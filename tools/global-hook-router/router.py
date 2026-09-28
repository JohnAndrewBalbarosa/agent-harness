from __future__ import annotations

import concurrent.futures
import json
import os
import subprocess
import sys
import time
from pathlib import Path

USER = Path.home()
SHARED = USER / ".codex-shared"
TOOL_ROOT = SHARED / "tools"
LOG_PATH = SHARED / "var" / "logs" / "hook-router.lifecycle.jsonl"
ORB_EVENTS = {"PermissionRequest", "PostToolUse", "SessionEnd", "SessionStart", "Stop", "UserPromptSubmit"}
OBS_EVENTS = {"PostToolUse", "SessionEnd", "SessionStart", "Stop", "UserPromptSubmit"}
NOTIFY_EVENTS = {"PermissionRequest", "Stop", "SubagentStop"}
HERDR_BRIDGE_EVENTS = {"PermissionRequest", "PostToolUse", "SessionEnd", "SessionStart", "Stop", "UserPromptSubmit"}


def notification_authority(environment: dict[str, str]) -> str:
    return "herdr" if environment.get("HERDR_ENV") == "1" and environment.get("HERDR_PANE_ID") else "legacy-fallback"


def emit(severity: str, operation: str, outcome: str, **details: object) -> None:
    LOG_PATH.parent.mkdir(parents=True, exist_ok=True)
    record = {
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "severity": severity,
        "component": "global-hook-router",
        "operation": operation,
        "outcome": outcome,
        "details": details,
    }
    encoded = (json.dumps(record, separators=(",", ":"), ensure_ascii=False) + "\n").encode()
    descriptor = os.open(LOG_PATH, os.O_APPEND | os.O_CREAT | os.O_WRONLY, 0o600)
    try:
        os.write(descriptor, encoded)
    finally:
        os.close(descriptor)


def active_profile() -> tuple[str, Path]:
    home = Path(os.environ.get("CODEX_HOME") or USER / ".codex").resolve()
    return {".codex": "personal", ".codex-cy": "cy", ".codex-feu": "feu"}.get(home.name, home.name), home


def launch_bootstrap(profile: str, home: Path) -> None:
    environment = os.environ.copy()
    environment.update(CODEX_SHARED_PROFILE=profile, CODEX_HOME=str(home))
    flags = sum(int(getattr(subprocess, name, 0)) for name in ("CREATE_NO_WINDOW", "DETACHED_PROCESS", "CREATE_NEW_PROCESS_GROUP"))
    subprocess.Popen(
        [sys.executable, str(Path(__file__).with_name("bootstrap.py"))],
        cwd=str(SHARED), env=environment, stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, close_fds=True, creationflags=flags,
    )


def invoke(name: str, command: list[str], payload: bytes, environment: dict[str, str], timeout: int) -> tuple[str, int, bytes]:
    started = time.perf_counter()
    flags = int(getattr(subprocess, "CREATE_NO_WINDOW", 0))
    try:
        process = subprocess.Popen(
            command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            env=environment, close_fds=True, creationflags=flags,
        )
        emit("info", "helper.process.started", "started", handler=name, processId=process.pid, windowMode="hidden")
        stdout, _ = process.communicate(input=payload, timeout=timeout)
        emit(
            "info" if process.returncode == 0 else "error", "helper.process.exited",
            "succeeded" if process.returncode == 0 else "failed", handler=name, processId=process.pid,
            exitCode=process.returncode, durationMs=round((time.perf_counter() - started) * 1000, 3), windowMode="hidden",
        )
        return name, process.returncode, stdout[:8192]
    except subprocess.TimeoutExpired:
        process.kill()
        process.communicate()
        emit("error", "helper.process.failed", "timeout", handler=name, processId=process.pid,
             durationMs=round((time.perf_counter() - started) * 1000, 3), windowMode="hidden")
        return name, 124, b""
    except OSError as exc:
        emit("error", "helper.process.failed", "launch_failed", handler=name,
             errorType=type(exc).__name__, durationMs=round((time.perf_counter() - started) * 1000, 3), windowMode="hidden")
        return name, 127, b""


def main() -> int:
    started = time.perf_counter()
    payload = sys.stdin.buffer.read()
    try:
        value = json.loads(payload or b"{}")
    except json.JSONDecodeError:
        value = {}
    event = str(value.get("hook_event_name") or value.get("hookEventName") or "")
    profile, home = active_profile()
    environment = os.environ.copy()
    environment.update(CODEX_OBS_INSTANCE=profile, CODEX_OBS_HOME=str(home), CODEX_HOME=str(home))
    children: list[tuple[str, list[str]]] = []
    if event in ORB_EVENTS:
        python = TOOL_ROOT / "codex-status-orb" / ".venv" / "Scripts" / "python.exe"
        children.append(("status-orb", [str(python), str(TOOL_ROOT / "codex-status-orb" / "hook.py")]))
    if event in OBS_EVENTS:
        children.append(("observability", [sys.executable, str(TOOL_ROOT / "observability-hub" / "hook.py")]))
    if event in HERDR_BRIDGE_EVENTS:
        children.append(("herdr-bridge", [sys.executable, str(TOOL_ROOT / "herdr-bridge" / "hook.py")]))
    notify_authority = notification_authority(environment)
    if event in NOTIFY_EVENTS and notify_authority == "legacy-fallback":
        children.append(("notification", [sys.executable, str(USER / ".codex" / "hooks" / "codex-cli-notify" / "scripts" / "codex_cli_notify.py"),
                                          "--config", str(USER / ".codex" / "codex-cli-notify.json")]))
    if event == "SessionStart":
        launch_bootstrap(profile, home)
    timeout = 590 if event == "PermissionRequest" else (8 if event == "SessionEnd" else 25)
    results: list[tuple[str, int, bytes]] = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, len(children))) as pool:
        futures = [pool.submit(invoke, name, command, payload, environment, timeout) for name, command in children]
        results = [future.result() for future in futures]
    failed = {name: code for name, code, _ in results if code != 0}
    emit("error" if failed else "info", "hook.dispatch", "failed" if failed else "succeeded",
         profile=profile, event=event, handlers=[name for name, _, _ in results], failures=failed,
         notificationAuthority=notify_authority,
         durationMs=round((time.perf_counter() - started) * 1000, 3))
    bridge_output = next((output for name, code, output in results if name == "herdr-bridge" and code == 0 and output.strip()), b"")
    notifier_output = next((output for name, code, output in results if name == "notification" and code == 0 and output.strip()), b"")
    if bridge_output:
        # The bridge emits a complete Codex hook envelope only when it has bounded
        # model context to inject. No notification event currently overlaps that path.
        sys.stdout.buffer.write(bridge_output)
    elif notifier_output:
        sys.stdout.buffer.write(notifier_output)
    elif event == "Stop":
        sys.stdout.write('{"continue":true}\n')
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
