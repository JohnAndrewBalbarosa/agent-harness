"""Notify consumer: decides and delivers done / needs-you notifications for every agent (event JSON on stdin).

State lives under AH_NOTIFY_DIR (default <root>/var/notify): per-session pending flags and one global rate window,
read-modify-written under a cross-process lock. Every decision is appended to a bounded `notify-events.jsonl`
used by `core.notify.report` to cross-reference premature or missed notifications.
"""
from __future__ import annotations

import json
import os
import re
import sys
import time
from datetime import datetime, timezone
from pathlib import Path, PureWindowsPath
from typing import Any, Mapping

from core import display
from core.events import Event, from_json
from core.locking import file_lock
from core.notify.decide import Decision, NotifyState, decide
from core.notify.deliver import deliver
from core.tasks import state as tasks

ROOT = Path(__file__).resolve().parents[2]
LOG_MAX_RECORDS = 1000
DEFAULT_RATE = (5, 60)
_UNSAFE = re.compile(r"[^A-Za-z0-9_-]")


def _dir(env: Mapping[str, str]) -> Path:
    return Path(env.get("AH_NOTIFY_DIR") or ROOT / "var" / "notify")


def _rate(env: Mapping[str, str]) -> tuple[int, int]:
    try:
        limit, window = env.get("NOTIFY_RATE_LIMIT", "").split("/")
        return int(limit), int(window)
    except ValueError:
        return DEFAULT_RATE


def title(event: Event) -> str:
    return display.title(event.agent, event.instance)


def _message(decision: Decision, event: Event) -> str:
    project = PureWindowsPath(event.cwd).name if event.cwd else ""
    return f"{decision.message}\n[{project}]" if project else (decision.message or "")


def _herdr_bin(env: Mapping[str, str]) -> str | None:
    if env.get("HERDR_ENV") != "1" or not env.get("HERDR_PANE_ID"):
        return None
    candidate = env.get("HERDR_BIN_PATH") or str(Path(env.get("LOCALAPPDATA", "")) / "Programs" / "Herdr" / "bin" / "herdr.exe")
    return candidate if Path(candidate).exists() else None


def _read_json(path: Path, default: Any) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return default


def _write_json(path: Path, value: Any) -> None:
    temporary = path.with_suffix(".tmp")
    temporary.write_text(json.dumps(value), encoding="utf-8")
    os.replace(temporary, path)


def _windows_notification_state() -> str:
    if os.name != "nt":
        return "n/a"
    try:
        import ctypes
        state = ctypes.c_int(0)
        ctypes.windll.shell32.SHQueryUserNotificationState(ctypes.byref(state))
        return {1: "not-present", 2: "busy-fullscreen", 3: "d3d-fullscreen", 4: "presentation-mode",
                5: "accepts-notifications", 6: "quiet-time", 7: "app-fullscreen"}.get(state.value, f"unknown({state.value})")
    except (OSError, AttributeError):
        return "unknown"


def _append_log(path: Path, record: dict) -> None:
    lines = path.read_text(encoding="utf-8").splitlines() if path.exists() else []
    kept = [line for line in lines if line.strip()][-(LOG_MAX_RECORDS - 1):]
    kept.append(json.dumps(record, ensure_ascii=False))
    temporary = path.with_suffix(".tmp")
    temporary.write_text("\n".join(kept) + "\n", encoding="utf-8")
    os.replace(temporary, path)


def handle(event: Event, env: Mapping[str, str]) -> dict:
    started = time.perf_counter()
    directory = _dir(env)
    directory.mkdir(parents=True, exist_ok=True)
    session_path = directory / f"{_UNSAFE.sub('_', event.agent)}-{_UNSAFE.sub('_', event.session_id)[:120]}.json"
    now = datetime.now(timezone.utc)
    outstanding: list[str] = []
    if event.event in ("turn.stop", "attention.needed"):
        outstanding = tasks.outstanding(tasks.default_store(env), event.agent, event.session_id, event.transcript_path, now)
    with file_lock(directory / "notify-state"):
        state = NotifyState(bool(_read_json(session_path, {}).get("pending")),
                            tuple(int(t) for t in _read_json(directory / "_rate.json", []) if isinstance(t, (int, float))))
        decision, new_state = decide(event, state, outstanding, int(now.timestamp()), _rate(env))
        _write_json(session_path, {"pending": new_state.pending})
        _write_json(directory / "_rate.json", list(new_state.recent_toasts))
    message = _message(decision, event)
    result = None
    if decision.action == "toast":
        herdr = _herdr_bin(env)
        result = "dry-run" if env.get("AH_NOTIFY_DRY_RUN") == "1" else deliver(
            Decision(decision.action, decision.reason, decision.sound, message), title(event), herdr)
    record = {
        "tsUtc": now.strftime("%Y-%m-%dT%H:%M:%S.") + f"{now.microsecond // 1000:03d}Z",
        "agent": event.agent, "instance": event.instance, "event": event.event, "nativeEvent": event.native_event,
        "sessionId": event.session_id, "transcriptPath": event.transcript_path, "decision": decision.action,
        "reason": decision.reason, "sound": decision.sound, "title": title(event), "message": message if result else None,
        "toastResult": result, "outstanding": outstanding,
        "herdr": {"inHerdr": env.get("HERDR_ENV") == "1" and bool(env.get("HERDR_PANE_ID")), "paneId": env.get("HERDR_PANE_ID")},
        "windowsNotificationState": _windows_notification_state(),
        "durationMs": round((time.perf_counter() - started) * 1000, 1),
    }
    with file_lock(directory / "notify-events"):
        _append_log(directory / "notify-events.jsonl", record)
    return record


def main() -> int:
    try:
        handle(from_json(sys.stdin.buffer.read().decode("utf-8")), os.environ)
    except (ValueError, KeyError, OSError, UnicodeDecodeError) as error:
        sys.stderr.write(f"notify consumer: {type(error).__name__}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
