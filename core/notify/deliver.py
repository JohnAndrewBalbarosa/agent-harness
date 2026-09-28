"""Notification delivery backends. Never raises; returns a result string for the notify log.

Inside Herdr: Herdr's notification API shows its in-app toast and plays the configured done/request bell, and the
Windows pop-up (kept in Notification Center) is silent. Outside Herdr, or when Herdr does not show it: a Windows
pop-up with sound.
"""
from __future__ import annotations

import json
import subprocess
from pathlib import Path
from typing import Callable

from core.notify.decide import Decision

TOAST_SCRIPT = Path(__file__).resolve().with_name("toast.ps1")
TIMEOUT_S = 15
Runner = Callable[..., subprocess.CompletedProcess]
_HIDDEN = int(getattr(subprocess, "CREATE_NO_WINDOW", 0))


def _windows(title: str, message: str, silent: bool, run: Runner, script: Path) -> str:
    args = ["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", str(script), "-Title", title, "-Message", message]
    if silent:
        args.append("-Silent")
    try:
        done = run(args, capture_output=True, timeout=TIMEOUT_S, creationflags=_HIDDEN)
    except (OSError, subprocess.SubprocessError) as error:
        return f"windows:error({type(error).__name__})"
    if done.returncode != 0:
        return f"windows:error(exit {done.returncode})"
    return "windows:handed(silent)" if silent else "windows:handed"


def _herdr(herdr_bin: str, title: str, message: str, sound: str, run: Runner) -> tuple[str, bool]:
    try:
        done = run([herdr_bin, "notification", "show", title, "--body", message, "--sound", sound],
                   capture_output=True, timeout=TIMEOUT_S, creationflags=_HIDDEN)
        result = json.loads(done.stdout or b"{}").get("result") or {}
    except (OSError, subprocess.SubprocessError, ValueError) as error:
        return f"herdr:error({type(error).__name__})", False
    if result.get("shown"):
        return f"herdr:{result.get('reason', 'shown')}", True
    return f"herdr:not-shown({result.get('reason', 'unknown')})", False


def deliver(decision: Decision, title: str, herdr_bin: str | None, run: Runner = subprocess.run,
            toast_script: Path = TOAST_SCRIPT) -> str:
    message = decision.message or ""
    if not herdr_bin:
        return _windows(title, message, False, run, toast_script)
    herdr_result, bell_played = _herdr(herdr_bin, title, message, decision.sound or "done", run)
    return f"{herdr_result} + {_windows(title, message, bell_played, run, toast_script)}"
