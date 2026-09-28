"""obs consumer: forwards the agent's raw hook payload to the vendored observability hook with agent identity.

Receives the NATIVE payload (router stdin="native"): the common event may drop oversized payloads, and the obs
hook needs tool inputs/responses. Identity comes from AH_AGENT / AH_INSTANCE set by the router. Codex instances
keep their legacy instance keys so existing observability history stays continuous (spec R9).
"""
from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

from core.display import title

ROOT = Path(__file__).resolve().parents[2]
OBS_HOOK = ROOT / "tools" / "observability-client" / "hook.py"
TIMEOUT_S = 20


def instance_key(agent: str, instance: str) -> str:
    if agent == "codex":
        return instance
    return agent if instance in ("default", "") else f"{agent}-{instance}"


def main() -> int:
    agent = os.environ.get("AH_AGENT", "")
    instance = os.environ.get("AH_INSTANCE", "default")
    env = {**os.environ, "CODEX_OBS_INSTANCE": instance_key(agent, instance), "CODEX_OBS_HOME": str(ROOT / "policy"),
           "AH_AGENT_DISPLAY": title(agent, instance), "AH_POLICY_FILE": "AGENTS.md", "PYTHONUTF8": "1"}
    hook = os.environ.get("AH_OBS_HOOK") or str(OBS_HOOK)
    try:
        subprocess.run([sys.executable, hook], input=sys.stdin.buffer.read(), env=env, stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL, timeout=TIMEOUT_S,
                       creationflags=int(getattr(subprocess, "CREATE_NO_WINDOW", 0)))
    except (OSError, subprocess.SubprocessError) as error:
        sys.stderr.write(f"obs consumer: {type(error).__name__}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
