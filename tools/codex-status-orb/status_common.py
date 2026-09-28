from __future__ import annotations

import hashlib
import json
import logging
from logging.handlers import RotatingFileHandler
import math
import os
import subprocess
import sys
import time
from contextlib import contextmanager
from datetime import datetime
from pathlib import Path
from typing import Any, Iterable

APP_NAME = "Codex Status Orb"
TOOL_ROOT = Path(__file__).resolve().parent
DATA_ROOT = Path(os.environ.get("CODEX_STATUS_ORB_DATA", Path(os.environ.get("LOCALAPPDATA", Path.home())) / "CodexStatusOrb"))
STATE_DIR = DATA_ROOT / "sessions"
CONFIG_PATH = DATA_ROOT / "config.json"
LOG_PATH = DATA_ROOT / "status-orb.log"
HEALTH_PATH = DATA_ROOT / "health.json"
FOCUS_DIAGNOSTICS_PATH = DATA_ROOT / "focus-diagnostics.json"
COLORS = {
    "idle": "#64748B",
    "running": "#3B82F6",
    "working": "#3B82F6",
    "needs_input": "#F59E0B",
    "ready": "#22C55E",
    "blocked": "#EF4444",
}
PRIORITY = {"idle": 0, "running": 1, "working": 1, "ready": 2, "needs_input": 3, "blocked": 4}
DEFAULT_CONFIG = {"muted": False, "hidden": False, "position": None}


def ensure_dirs() -> None:
    STATE_DIR.mkdir(parents=True, exist_ok=True)


_LOGGER: logging.Logger | None = None


def get_logger() -> logging.Logger:
    global _LOGGER
    if _LOGGER is None:
        ensure_dirs()
        logger = logging.getLogger("codex_status_orb")
        logger.setLevel(logging.INFO)
        logger.propagate = False
        if not logger.handlers:
            handler = RotatingFileHandler(LOG_PATH, maxBytes=1_000_000, backupCount=3, encoding="utf-8")
            handler.setFormatter(logging.Formatter("%(asctime)s %(levelname)s %(message)s"))
            logger.addHandler(handler)
        _LOGGER = logger
    return _LOGGER


def log(message: str, level: int = logging.INFO) -> None:
    try:
        get_logger().log(level, message)
    except Exception:
        pass


def checkpoint(name: str, **detail: Any) -> None:
    fields = " ".join(f"{key}={json.dumps(value, ensure_ascii=True)}" for key, value in detail.items())
    log(f"checkpoint={name}{' ' + fields if fields else ''}")


def write_health(status: str, checkpoint_name: str, error_count: int = 0, last_error: str = "") -> None:
    save_json_atomic(HEALTH_PATH, {
        "status": status,
        "checkpoint": checkpoint_name,
        "pid": os.getpid(),
        "updated_at": time.time(),
        "error_count": error_count,
        "last_error": last_error,
    })


def load_json(path: Path, default: Any) -> Any:
    try:
        with path.open("r", encoding="utf-8") as stream:
            return json.load(stream)
    except Exception:
        return default


def save_json_atomic(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temp = path.with_name(f".{path.name}.{os.getpid()}.{time.time_ns()}.tmp")
    with temp.open("w", encoding="utf-8", newline="\n") as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2)
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temp, path)


def load_config() -> dict[str, Any]:
    result = dict(DEFAULT_CONFIG)
    loaded = load_json(CONFIG_PATH, {})
    if isinstance(loaded, dict):
        result.update(loaded)
    return result


def save_config(config: dict[str, Any]) -> None:
    save_json_atomic(CONFIG_PATH, config)


def safe_session_id(value: Any) -> str:
    raw = str(value or "unknown")
    return "".join(ch for ch in raw if ch.isalnum() or ch in "-_")[:160] or "unknown"


def state_path(session_id: str) -> Path:
    return STATE_DIR / f"{safe_session_id(session_id)}.json"


@contextmanager
def session_lock(session_id: str):
    import msvcrt

    ensure_dirs()
    path = STATE_DIR / f".{safe_session_id(session_id)}.lock"
    with path.open("a+b") as stream:
        stream.seek(0, os.SEEK_END)
        if stream.tell() == 0:
            stream.write(b"0")
            stream.flush()
        stream.seek(0)
        msvcrt.locking(stream.fileno(), msvcrt.LK_LOCK, 1)
        try:
            yield
        finally:
            stream.seek(0)
            msvcrt.locking(stream.fileno(), msvcrt.LK_UNLCK, 1)


def claim_hook_event(payload: dict[str, Any], window_seconds: float = 3.0) -> bool:
    """Claim one lifecycle event across duplicate active config layers."""
    session_id = safe_session_id(payload.get("session_id") or payload.get("sessionId"))
    canonical = {key: value for key, value in payload.items() if key not in {"hook_source", "hookSource"}}
    # Hook tool responses can contain lone UTF-16 surrogates from Windows consoles.
    # ASCII escaping keeps deduplication deterministic without rejecting the event.
    encoded = json.dumps(canonical, sort_keys=True, separators=(",", ":"), ensure_ascii=True, default=str).encode("ascii")
    fingerprint = hashlib.sha256(encoded).hexdigest()
    path = STATE_DIR / f".{session_id}.dedupe"
    now = time.time()
    with session_lock(session_id):
        previous = load_json(path, {})
        if (
            isinstance(previous, dict)
            and previous.get("fingerprint") == fingerprint
            and now - float(previous.get("updated_at", 0)) <= window_seconds
        ):
            return False
        save_json_atomic(path, {"fingerprint": fingerprint, "updated_at": now})
    return True


def write_session(event: dict[str, Any]) -> bool:
    session_id = safe_session_id(event.get("session_id"))
    event["session_id"] = session_id
    path = state_path(session_id)
    with session_lock(session_id):
        existing = load_json(path, {})
        if isinstance(existing, dict) and int(existing.get("event_ns", 0)) > int(event.get("event_ns", 0)):
            return False
        save_json_atomic(path, event)
    return True


def remove_session(session_id: str) -> None:
    with session_lock(session_id):
        state_path(session_id).unlink(missing_ok=True)


def read_sessions() -> list[dict[str, Any]]:
    ensure_dirs()
    sessions: list[dict[str, Any]] = []
    for path in STATE_DIR.glob("*.json"):
        value = load_json(path, None)
        if isinstance(value, dict) and value.get("session_id"):
            value["_path"] = str(path)
            sessions.append(value)
    return sorted(sessions, key=lambda item: int(item.get("event_ns", 0)), reverse=True)


def aggregate_status(sessions: Iterable[dict[str, Any]]) -> str:
    statuses = [str(item.get("status", "idle")) for item in sessions]
    if not statuses:
        return "idle"
    return max(statuses, key=lambda status: PRIORITY.get(status, 0))


def find_turn_abort_after(session: dict[str, Any]) -> dict[str, Any] | None:
    """Return a structured turn_aborted event newer than the orb state, if present."""
    transcript = Path(str(session.get("transcript_path") or ""))
    if not transcript.is_file():
        sid = safe_session_id(session.get("session_id"))
        transcript = Path()
        for codex_root in (Path.home() / ".codex", Path.home() / ".codex-cy", Path.home() / ".codex-feu"):
            session_root = codex_root / "sessions"
            if not session_root.is_dir():
                continue
            match = next(session_root.rglob(f"*{sid}.jsonl"), None)
            if match:
                transcript = match
                break
    if not transcript.is_file():
        return None
    try:
        with transcript.open("rb") as stream:
            stream.seek(0, os.SEEK_END)
            size = stream.tell()
            stream.seek(max(0, size - 262_144))
            lines = stream.read().decode("utf-8", errors="replace").splitlines()
        state_time = float(session.get("updated_at", 0))
        for line in reversed(lines):
            try:
                item = json.loads(line)
            except json.JSONDecodeError:
                continue
            payload = item.get("payload", {}) if isinstance(item, dict) else {}
            if item.get("type") == "event_msg" and payload.get("type") == "turn_aborted":
                completed_at = float(payload.get("completed_at", 0))
                return payload if completed_at > state_time else None
        return None
    except Exception as exc:
        log(f"checkpoint=abort_scan_error session={session.get('session_id')!r} error={exc!r}")
        return None


def find_task_complete_after(session: dict[str, Any]) -> dict[str, Any] | None:
    """Return a structured task_complete newer than the last recorded hook state."""
    transcript = Path(str(session.get("transcript_path") or ""))
    if not transcript.is_file():
        sid = safe_session_id(session.get("session_id"))
        transcript = Path()
        for codex_root in (Path.home() / ".codex", Path.home() / ".codex-cy", Path.home() / ".codex-feu"):
            session_root = codex_root / "sessions"
            if not session_root.is_dir():
                continue
            match = next(session_root.rglob(f"*{sid}.jsonl"), None)
            if match:
                transcript = match
                break
    if not transcript.is_file():
        return None
    try:
        with transcript.open("rb") as stream:
            stream.seek(0, os.SEEK_END)
            size = stream.tell()
            stream.seek(max(0, size - 262_144))
            lines = stream.read().decode("utf-8", errors="replace").splitlines()
        state_time = float(session.get("updated_at", 0))
        for line in reversed(lines):
            try:
                item = json.loads(line)
            except json.JSONDecodeError:
                continue
            payload = item.get("payload", {}) if isinstance(item, dict) else {}
            if item.get("type") == "event_msg" and payload.get("type") == "task_complete":
                completed_at = float(payload.get("completed_at", 0))
                return payload if completed_at > state_time else None
        return None
    except Exception as exc:
        log(f"checkpoint=task_complete_scan_error session={session.get('session_id')!r} error={exc!r}")
        return None


def find_user_input_state_after(session: dict[str, Any]) -> dict[str, Any] | None:
    """Return the newest structured request_user_input and whether it is unresolved."""
    transcript = Path(str(session.get("transcript_path") or ""))
    if not transcript.is_file():
        sid = safe_session_id(session.get("session_id"))
        transcript = Path()
        for codex_root in (Path.home() / ".codex", Path.home() / ".codex-cy", Path.home() / ".codex-feu"):
            session_root = codex_root / "sessions"
            if not session_root.is_dir():
                continue
            match = next(session_root.rglob(f"*{sid}.jsonl"), None)
            if match:
                transcript = match
                break
    if not transcript.is_file():
        return None
    try:
        with transcript.open("rb") as stream:
            stream.seek(0, os.SEEK_END)
            size = stream.tell()
            stream.seek(max(0, size - 262_144))
            lines = stream.read().decode("utf-8", errors="replace").splitlines()
        outputs: dict[str, float] = {}
        requests: list[dict[str, Any]] = []
        for line in lines:
            try:
                item = json.loads(line)
            except json.JSONDecodeError:
                continue
            payload = item.get("payload", {}) if isinstance(item, dict) else {}
            if item.get("type") != "response_item" or not isinstance(payload, dict):
                continue
            try:
                event_at = datetime.fromisoformat(str(item.get("timestamp") or "").replace("Z", "+00:00")).timestamp()
            except ValueError:
                event_at = 0.0
            call_id = str(payload.get("call_id") or "")
            if payload.get("type") == "function_call_output" and call_id:
                outputs[call_id] = event_at
            elif payload.get("type") == "function_call" and payload.get("name") == "request_user_input":
                arguments = payload.get("arguments") or {}
                if isinstance(arguments, str):
                    try:
                        arguments = json.loads(arguments)
                    except json.JSONDecodeError:
                        arguments = {}
                headers = [
                    str(question.get("header") or "").strip().lower()
                    for question in arguments.get("questions", [])
                    if isinstance(question, dict)
                ] if isinstance(arguments, dict) else []
                input_kind = "technology_selection" if any(header in {"technology", "tech choice", "reuse choice", "framework", "library"} for header in headers) else "general"
                if any(header in {"test status", "tech status", "tested"} for header in headers):
                    input_kind = "technology_status"
                requests.append({"call_id": call_id, "requested_at": event_at, "input_kind": input_kind})
        if not requests:
            return None
        newest = requests[-1]
        resolved_at = outputs.get(str(newest["call_id"]), 0.0)
        newest.update(pending=not bool(resolved_at), resolved_at=resolved_at)
        return newest
    except Exception as exc:
        log(f"checkpoint=user_input_scan_error session={session.get('session_id')!r} error={exc!r}")
        return None


def process_matches(pid: int, create_time: float | None = None) -> bool:
    try:
        import psutil

        proc = psutil.Process(int(pid))
        if not proc.is_running():
            return False
        if create_time and not math.isclose(proc.create_time(), float(create_time), abs_tol=1.0):
            return False
        return True
    except Exception:
        return False


def process_context() -> dict[str, Any]:
    result: dict[str, Any] = {
        "codex_pid": 0,
        "codex_create_time": 0.0,
        "shell_pid": 0,
        "terminal_pid": 0,
        "terminal_kind": "unknown",
        "wt_session": os.environ.get("WT_SESSION", ""),
    }
    try:
        import psutil

        proc = psutil.Process(os.getpid())
        ancestors = [proc] + proc.parents()
        codex_index = next((index for index, item in enumerate(ancestors) if item.name().lower() == "codex.exe"), None)
        if codex_index is None:
            return result
        codex_proc = ancestors[codex_index]
        result["codex_pid"] = codex_proc.pid
        result["codex_create_time"] = codex_proc.create_time()
        for parent in ancestors[codex_index + 1 :]:
            name = parent.name().lower()
            if not result["shell_pid"] and name in {"cmd.exe", "powershell.exe", "pwsh.exe"}:
                result["shell_pid"] = parent.pid
            if name in {"windowsterminal.exe", "openconsole.exe", "conhost.exe"}:
                result["terminal_pid"] = parent.pid
                result["terminal_kind"] = name.removesuffix(".exe")
                break
        if not result["terminal_pid"]:
            result["terminal_pid"] = result["shell_pid"]
            result["terminal_kind"] = "console"
    except Exception as exc:
        log(f"process-context-error {exc!r}")
    return result


def launch_orb() -> None:
    pythonw = TOOL_ROOT / ".venv" / "Scripts" / "pythonw.exe"
    executable = pythonw if pythonw.exists() else Path(sys.executable).with_name("pythonw.exe")
    if not executable.exists():
        executable = Path(sys.executable)
    flags = 0
    for name in ("CREATE_NO_WINDOW", "DETACHED_PROCESS", "CREATE_NEW_PROCESS_GROUP"):
        flags |= int(getattr(subprocess, name, 0))
    try:
        subprocess.Popen(
            [str(executable), str(TOOL_ROOT / "orb.py")],
            cwd=str(TOOL_ROOT),
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            close_fds=True,
            creationflags=flags,
        )
    except Exception as exc:
        log(f"orb-launch-error {exc!r}")


def focus_terminal(session: dict[str, Any]) -> bool:
    started_ns = time.perf_counter_ns()
    metrics: dict[str, Any] = {
        "session_id": str(session.get("session_id", "")),
        "terminal_pid": int(session.get("terminal_pid", 0)),
        "started_at": time.time(),
        "success": False,
    }
    try:
        import win32con
        import win32gui
        import win32process

        target_pids = {int(session.get("terminal_pid", 0)), int(session.get("shell_pid", 0))} - {0}
        matches: list[tuple[int, int]] = []

        def visitor(hwnd: int, _extra: Any) -> None:
            if not win32gui.IsWindowVisible(hwnd):
                return
            _, pid = win32process.GetWindowThreadProcessId(hwnd)
            if pid in target_pids:
                matches.append((hwnd, pid))

        win32gui.EnumWindows(visitor, None)
        if not matches:
            return False
        terminal_pid = int(session.get("terminal_pid", 0))
        hwnd, _matched_pid = next((item for item in matches if item[1] == terminal_pid), matches[0])
        metrics["was_minimized"] = bool(win32gui.IsIconic(hwnd))
        restore_started = time.perf_counter_ns()
        if metrics["was_minimized"]:
            win32gui.ShowWindow(hwnd, win32con.SW_RESTORE)
        win32gui.ShowWindow(hwnd, win32con.SW_SHOW)
        metrics["restore_ms"] = round((time.perf_counter_ns() - restore_started) / 1_000_000, 3)
        selected_tab = ""
        focus_method = "window"
        uia_started = time.perf_counter_ns()
        last_uia_error: Exception | None = None
        attempts = 0
        from pywinauto import Desktop

        for attempts in range(1, 4):
            try:
                window = Desktop(backend="uia").window(process=terminal_pid) if terminal_pid else Desktop(backend="uia").window(handle=hwnd)
                window.wait("exists visible enabled ready", timeout=0.7, retry_interval=0.05)
                tabs = [item for item in window.descendants() if item.element_info.control_type == "TabItem"]
                cwd_name = Path(str(session.get("cwd") or "")).name.casefold()
                want_running = str(session.get("status")) in {"running", "needs_input"}

                def tab_score(item: Any) -> tuple[int, int]:
                    title = item.window_text()
                    has_spinner = bool(title) and 0x2800 <= ord(title[0]) <= 0x28FF
                    cwd_match = bool(cwd_name) and cwd_name in title.casefold()
                    return (int(cwd_match) * 4 + int(has_spinner == want_running) * 2, -len(title))

                if tabs:
                    target = max(tabs, key=tab_score)
                    target.click_input()
                    selected_tab = target.window_text()
                    focus_method = "uia-tab-click"
                break
            except Exception as exc:
                last_uia_error = exc
                if attempts < 3:
                    time.sleep(0.05)
        metrics["uia_ms"] = round((time.perf_counter_ns() - uia_started) / 1_000_000, 3)
        metrics["uia_attempts"] = attempts
        if last_uia_error and not selected_tab:
            metrics["uia_error"] = repr(last_uia_error)
            log(f"checkpoint=terminal_tab_fallback attempts={attempts} error={last_uia_error!r}")
        foreground_started = time.perf_counter_ns()
        try:
            win32gui.BringWindowToTop(hwnd)
            win32gui.SetForegroundWindow(hwnd)
        except Exception as exc:
            metrics["foreground_error"] = repr(exc)
            log(f"checkpoint=foreground_lock_nonfatal error={exc!r}")
        metrics["foreground_ms"] = round((time.perf_counter_ns() - foreground_started) / 1_000_000, 3)
        metrics.update(
            success=True,
            method=focus_method,
            tab=selected_tab or "window-only",
            total_ms=round((time.perf_counter_ns() - started_ns) / 1_000_000, 3),
        )
        save_json_atomic(FOCUS_DIAGNOSTICS_PATH, metrics)
        checkpoint(
            "focus_completed",
            pid=int(session.get("terminal_pid", 0)) or next(iter(target_pids), 0),
            tab=selected_tab or "window-only",
            method=focus_method,
            was_minimized=metrics["was_minimized"],
            restore_ms=metrics["restore_ms"],
            uia_ms=metrics["uia_ms"],
            uia_attempts=attempts,
            foreground_ms=metrics["foreground_ms"],
            total_ms=metrics["total_ms"],
        )
        return True
    except Exception as exc:
        metrics.update(error=repr(exc), total_ms=round((time.perf_counter_ns() - started_ns) / 1_000_000, 3))
        save_json_atomic(FOCUS_DIAGNOSTICS_PATH, metrics)
        log(f"checkpoint=focus_failed total_ms={metrics['total_ms']} error={exc!r}")
        return False
