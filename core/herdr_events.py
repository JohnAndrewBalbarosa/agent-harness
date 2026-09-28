"""Herdr plugin event handlers (spec 2026-09-28 herdr-plugin §4.5a).

`pane.agent_detected`: every agent Herdr detects in a pane gets a preflight with --init (subscribe + compile when
it is supported but not set up). The verdict is logged, and shown as a Herdr notification: failures once per pane
and status (with the request sound), `ready` only the first time a pane shows that agent.
Entry point: `python -m core.herdr_events agent-detected` with HERDR_PLUGIN_EVENT_JSON set by Herdr.
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Callable

from core import lifecycle, registry
from core.locking import file_lock

ROOT = Path(__file__).resolve().parents[1]
Preflight = Callable[[str], "registry.Verdict | None"]
Notify = Callable[[str, str, str], None]
HIDDEN = int(getattr(subprocess, "CREATE_NO_WINDOW", 0))


def _event(raw: str) -> dict | None:
    try:
        value = json.loads(raw)
    except (TypeError, ValueError):
        return None
    if isinstance(value, dict) and isinstance(value.get("data"), dict):
        value = value["data"]
    return value if isinstance(value, dict) else None


def on_agent_detected(raw: str, preflight: Preflight, notify: Notify, state_path: Path, log_path: Path) -> str:
    event = _event(raw)
    agent = str((event or {}).get("agent") or "").strip().lower()
    if not event or not agent or event.get("released"):
        lifecycle.emit(log_path, "herdr-plugin", "agent_detected.skipped", "observed", reason="no agent or released")
        return "ignored"
    pane = str(event.get("pane_id") or "")
    verdict = preflight(agent)
    if verdict is None:
        return "ignored"
    lifecycle.emit(log_path, "herdr-plugin", "agent_detected.preflight", "succeeded" if verdict.status == "ready" else "failed",
                   agent=agent, pane=pane, status=verdict.status, message=verdict.message)
    state_path.parent.mkdir(parents=True, exist_ok=True)
    with file_lock(state_path):
        try:
            seen = json.loads(state_path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            seen = {}
        key = f"{pane}:{agent}"
        if seen.get(key) == verdict.status:
            return "repeat"
        state_path.write_text(json.dumps({**seen, key: verdict.status}), encoding="utf-8")
    notify("agent-harness", verdict.message, "none" if verdict.status == "ready" else "request")
    return verdict.status


def _herdr_notify(title: str, body: str, sound: str) -> None:
    herdr = os.environ.get("HERDR_BIN_PATH") or shutil.which("herdr") or "herdr"
    subprocess.run([herdr, "notification", "show", title, "--body", body, "--sound", sound],
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=10, creationflags=HIDDEN, check=False)


def _preflight(agent: str) -> registry.Verdict:
    def compile_instance(cfg) -> None:
        from compile.run import compile_all
        compile_all(cfg, dry_run=False)

    return registry.preflight(agent, env_file=ROOT / ".env", environ=os.environ, init=True, which=shutil.which,
                              compile_instance=compile_instance, log_dir=ROOT / "var" / "logs")


def main(argv: list[str]) -> int:
    log_path = ROOT / "var" / "logs" / "herdr-plugin.lifecycle.jsonl"
    if argv[:1] != ["agent-detected"]:
        return 0
    try:
        on_agent_detected(os.environ.get("HERDR_PLUGIN_EVENT_JSON", ""), _preflight, _herdr_notify,
                          ROOT / "var" / "herdr" / "detected.json", log_path)
    except Exception as error:  # a plugin hook must never break Herdr; the failure is logged
        lifecycle.emit(log_path, "herdr-plugin", "agent_detected.error", "failed", error=f"{type(error).__name__}: {error}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
