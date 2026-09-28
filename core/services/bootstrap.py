"""Session-start service bootstrap: keep the usage receiver, status orb, console-window monitor and the optional
observability hub running.

Replaces the Codex-only `global-hook-router/bootstrap.py`. The `services` consumer detaches this module so the agent's
SessionStart hook never waits on Docker. One bootstrap at a time (non-blocking lock); every step is logged.
"""
from __future__ import annotations

import contextlib
import json
import os
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Iterator, Sequence
from urllib.request import urlopen

from core import lifecycle
from core.config import runtime_env

ROOT = Path(__file__).resolve().parents[2]
ORB_HEALTH_MAX_AGE_S = 15
HUB_START_TIMEOUT_S = 150
DETACHED = sum(int(getattr(subprocess, n, 0)) for n in ("CREATE_NO_WINDOW", "DETACHED_PROCESS", "CREATE_NEW_PROCESS_GROUP"))

Launch = Callable[[Sequence[str | Path], Path], None]
Healthy = Callable[[str], bool]


@dataclass(frozen=True)
class Layout:
    root: Path
    orb_health: Path
    obs_endpoint: str
    hub_dir: Path | None = None  # observability hub checkout (start.ps1); default <root>/hub
    orb_ui: bool = True  # ORB_UI=0: the orb hook keeps session state, but no orb window is started

    @property
    def log(self) -> Path:
        return self.root / "var" / "logs" / "services.lifecycle.jsonl"


def default_layout(env: dict[str, str]) -> Layout:
    local = Path(env.get("LOCALAPPDATA") or Path.home())
    return Layout(ROOT, local / "CodexStatusOrb" / "health.json", env.get("OBS_ENDPOINT") or "http://127.0.0.1:4319",
                  Path(env["HUB_DIR"]) if env.get("HUB_DIR") else None, str(env.get("ORB_UI", "")).strip() != "0")


def launch_detached(command: Sequence[str | Path], cwd: Path) -> None:
    subprocess.Popen([str(part) for part in command], cwd=str(cwd), stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                     stderr=subprocess.DEVNULL, close_fds=True, creationflags=DETACHED)


def http_healthy(url: str) -> bool:
    try:
        with urlopen(url, timeout=2) as response:
            return response.status == 200
    except OSError:
        return False


@contextlib.contextmanager
def single_instance(root: Path) -> Iterator[bool]:
    lock = root / "var" / "services.lock"
    lock.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(lock, os.O_RDWR | os.O_CREAT, 0o600)
    acquired = False
    try:
        if os.name == "nt":
            import msvcrt
            try:
                msvcrt.locking(fd, msvcrt.LK_NBLCK, 1)
                acquired = True
            except OSError:
                pass
        else:
            import fcntl
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                acquired = True
            except OSError:
                pass
        yield acquired
    finally:
        if acquired and os.name == "nt":
            import msvcrt
            os.lseek(fd, 0, os.SEEK_SET)
            msvcrt.locking(fd, msvcrt.LK_UNLCK, 1)
        os.close(fd)


def _orb_healthy(path: Path) -> bool:
    try:
        health = json.loads(path.read_text(encoding="utf-8"))
        return health.get("status") == "healthy" and time.time() - float(health.get("updated_at", 0)) < ORB_HEALTH_MAX_AGE_S
    except (OSError, ValueError, TypeError, AttributeError):
        return False


def ensure_orb(layout: Layout, launch: Launch) -> str:
    orb = layout.root / "tools" / "codex-status-orb"
    pythonw = orb / ".venv" / "Scripts" / "pythonw.exe"
    if not pythonw.exists():
        return "not_installed"
    if not layout.orb_ui:
        return "ui_disabled"
    if _orb_healthy(layout.orb_health):
        return "already_running"
    launch([pythonw, orb / "orb.py"], orb)
    return "requested"


def ensure_hub(layout: Layout, launch: Launch, healthy: Healthy) -> str:
    start = (layout.hub_dir or layout.root / "hub") / "start.ps1"
    if not start.exists():
        return "not_installed"
    if healthy(layout.obs_endpoint.rstrip("/") + "/health"):
        return "already_running"
    launch(["powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", start], start.parent)
    return "requested"


def ensure_console_monitor(layout: Layout, launch: Launch) -> str:
    """The monitor holds a named mutex, so launching it while one runs is a no-op."""
    monitor = layout.root / "tools" / "observability-client" / "console_window_monitor.py"
    if not monitor.exists():
        return "not_installed"
    pythonw = Path(sys.executable).with_name("pythonw.exe")
    launch([pythonw if pythonw.exists() else sys.executable, monitor], monitor.parent)
    return "requested"


def run(layout: Layout, launch: Launch = launch_detached, healthy: Healthy = http_healthy,
        ensure_receiver: Callable[[], None] | None = None) -> None:
    if ensure_receiver is None:
        from core.usage import receiver
        ensure_receiver = receiver.ensure
    steps: list[tuple[str, Callable[[], str | None]]] = [
        ("receiver.ensure", ensure_receiver),
        ("orb.ensure", lambda: ensure_orb(layout, launch)),
        ("hub.ensure", lambda: ensure_hub(layout, launch, healthy)),
        ("console-monitor.ensure", lambda: ensure_console_monitor(layout, launch)),
    ]
    for operation, step in steps:
        try:
            result = step()
            lifecycle.emit(layout.log, "services", operation, "succeeded", result=result or "ensured")
        except Exception as error:  # one broken service must not stop the others
            lifecycle.emit(layout.log, "services", operation, "failed", error=type(error).__name__)


def main() -> int:
    env = runtime_env(ROOT / ".env", os.environ)
    layout = default_layout(env)
    with single_instance(layout.root) as acquired:
        if not acquired:
            lifecycle.emit(layout.log, "services", "bootstrap", "skipped", reason="already_running")
            return 0
        os.environ.update({k: v for k, v in env.items() if k in ("OTLP_PORT", "OBS_ENDPOINT")})
        run(layout)
    return 0


if __name__ == "__main__":
    sys.exit(main())
