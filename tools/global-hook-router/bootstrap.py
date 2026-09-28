from __future__ import annotations

import json
import msvcrt
import os
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

USER = Path.home()
SHARED = USER / ".codex-shared"
LOG_PATH = SHARED / "var" / "logs" / "bootstrap.lifecycle.jsonl"
LOCK_PATH = SHARED / "var" / "bootstrap.lock"
ORB_ROOT = SHARED / "tools" / "codex-status-orb"
OBS_ROOT = SHARED / "tools" / "observability-hub"


def emit(severity: str, operation: str, outcome: str, **details: object) -> None:
    LOG_PATH.parent.mkdir(parents=True, exist_ok=True)
    record = {"timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "severity": severity,
              "component": "global-bootstrap", "operation": operation, "outcome": outcome, "details": details}
    with LOG_PATH.open("a", encoding="utf-8") as stream:
        stream.write(json.dumps(record, separators=(",", ":"), ensure_ascii=False) + "\n")


def hub_healthy() -> bool:
    try:
        with urllib.request.urlopen("http://127.0.0.1:4319/health", timeout=2) as response:
            return response.status == 200
    except Exception:
        return False


def ensure_orb() -> None:
    health_path = Path(os.environ.get("LOCALAPPDATA", USER)) / "CodexStatusOrb" / "health.json"
    try:
        health = json.loads(health_path.read_text(encoding="utf-8"))
        if health.get("status") == "healthy" and time.time() - float(health.get("updated_at", 0)) < 15:
            emit("info", "status-orb.health", "already_running")
            return
    except (OSError, ValueError, TypeError):
        pass
    pythonw = ORB_ROOT / ".venv" / "Scripts" / "pythonw.exe"
    flags = sum(int(getattr(subprocess, name, 0)) for name in ("CREATE_NO_WINDOW", "DETACHED_PROCESS", "CREATE_NEW_PROCESS_GROUP"))
    subprocess.Popen([str(pythonw), str(ORB_ROOT / "orb.py")], cwd=str(ORB_ROOT), stdin=subprocess.DEVNULL,
                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, close_fds=True, creationflags=flags)
    emit("info", "status-orb.start", "requested")


def ensure_console_window_monitor() -> None:
    pythonw = Path(sys.executable).with_name("pythonw.exe")
    monitor = OBS_ROOT / "console_window_monitor.py"
    if not pythonw.is_file() or not monitor.is_file():
        emit("error", "console-monitor.start", "failed", reason="launcher_missing")
        return
    flags = sum(int(getattr(subprocess, name, 0)) for name in
                ("CREATE_NO_WINDOW", "DETACHED_PROCESS", "CREATE_NEW_PROCESS_GROUP"))
    try:
        process = subprocess.Popen([str(pythonw), str(monitor)], cwd=str(OBS_ROOT),
                                   stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                   stderr=subprocess.DEVNULL, close_fds=True, creationflags=flags)
        emit("info", "console-monitor.start", "requested", processId=process.pid, windowMode="hidden")
    except OSError as error:
        emit("error", "console-monitor.start", "failed", errorType=type(error).__name__)


def ensure_observability() -> None:
    if hub_healthy():
        emit("info", "observability.health", "already_running")
        return
    started = time.perf_counter()
    result = subprocess.run(
        ["powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", str(OBS_ROOT / "start.ps1")],
        cwd=str(OBS_ROOT), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=150, check=False,
        creationflags=int(getattr(subprocess, "CREATE_NO_WINDOW", 0)),
    )
    deadline = time.monotonic() + 12
    while not hub_healthy() and time.monotonic() < deadline:
        time.sleep(1)
    healthy = hub_healthy()
    emit("info" if healthy else "error", "observability.start", "ready" if healthy else "failed",
         exitCode=result.returncode, durationMs=round((time.perf_counter() - started) * 1000, 3))
    if healthy:
        flush = subprocess.run([str(OBS_ROOT / "obs.cmd"), "flush"], cwd=str(OBS_ROOT), stdout=subprocess.DEVNULL,
                               stderr=subprocess.DEVNULL, timeout=60, check=False,
                               creationflags=int(getattr(subprocess, "CREATE_NO_WINDOW", 0)))
        emit("info" if flush.returncode == 0 else "error", "observability.spool.flush",
             "succeeded" if flush.returncode == 0 else "failed", exitCode=flush.returncode)


def main() -> int:
    LOCK_PATH.parent.mkdir(parents=True, exist_ok=True)
    descriptor = os.open(LOCK_PATH, os.O_RDWR | os.O_CREAT, 0o600)
    try:
        if os.path.getsize(LOCK_PATH) == 0:
            os.write(descriptor, b"0")
        os.lseek(descriptor, 0, os.SEEK_SET)
        try:
            msvcrt.locking(descriptor, msvcrt.LK_NBLCK, 1)
        except OSError:
            emit("info", "bootstrap.lock", "already_running")
            return 0
        profile = os.environ.get("CODEX_SHARED_PROFILE", "personal")
        emit("info", "bootstrap", "started", profile=profile)
        sync = subprocess.run([sys.executable, str(SHARED / "config" / "sync_config.py"), "sync", "--profile", profile],
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=10, check=False,
                              creationflags=int(getattr(subprocess, "CREATE_NO_WINDOW", 0)))
        emit("info" if sync.returncode == 0 else "error", "config.sync", "succeeded" if sync.returncode == 0 else "failed",
             profile=profile, exitCode=sync.returncode)
        ensure_orb()
        ensure_observability()
        ensure_console_window_monitor()
        emit("info", "bootstrap", "completed", profile=profile)
        return 0
    finally:
        try:
            os.lseek(descriptor, 0, os.SEEK_SET)
            msvcrt.locking(descriptor, msvcrt.LK_UNLCK, 1)
        except OSError:
            pass
        os.close(descriptor)


if __name__ == "__main__":
    raise SystemExit(main())
