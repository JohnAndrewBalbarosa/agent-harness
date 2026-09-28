"""Context consumer: session-start context from the observability hub, for every agent (spec §9 context).

Reuses herdr-bridge's `build_context` (loaded under a unique module name: the observability client also ships a
module called `hook`). Prints bounded plain text; the agent's adapter wraps it into its native SessionStart output.
An unreachable hub yields no output.
"""
from __future__ import annotations

import importlib.util
import os
import sys
import uuid
from pathlib import Path
from typing import Callable, Mapping

from core.events import Event, from_json

ROOT = Path(__file__).resolve().parents[2]
BRIDGE_DIR = ROOT / "tools" / "herdr-bridge"
OBS_DIR = ROOT / "tools" / "observability-client"
GLOBAL_PROJECT_ID = str(uuid.uuid5(uuid.NAMESPACE_URL, "codex-observability:global-agent-project"))


def limit_for(source: str | None) -> int:
    return 3000 if source == "compact" else 6000


def _build_context() -> Callable[[str, str, str], str]:
    sys.path.insert(0, str(BRIDGE_DIR))
    spec = importlib.util.spec_from_file_location("herdr_bridge_hook", BRIDGE_DIR / "hook.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.build_context


def _project_id(cwd: str | None) -> str:
    if not cwd:
        return GLOBAL_PROJECT_ID
    sys.path.insert(0, str(OBS_DIR))
    import obs  # vendored observability client
    found = obs.project_config(Path(cwd))
    return str(found[1]["project"]["id"]) if found else GLOBAL_PROJECT_ID


def handle(event: Event, env: Mapping[str, str]) -> str:
    if event.event != "session.start":
        return ""
    if env.get("OBS_ENDPOINT"):
        os.environ["OBS_ENDPOINT"] = env["OBS_ENDPOINT"]
    source = str(event.data.get("source") or "")
    text = _build_context()(_project_id(event.cwd), event.instance, source)
    return text[:limit_for(source)]


def main() -> int:
    try:
        text = handle(from_json(sys.stdin.buffer.read().decode("utf-8")), os.environ)
        if text:
            sys.stdout.buffer.write(text.encode("utf-8"))
    except Exception as error:  # context is best-effort; never block session start
        sys.stderr.write(f"context consumer: {type(error).__name__}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
