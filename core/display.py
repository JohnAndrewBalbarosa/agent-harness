"""Human-facing agent names shared by notifications and observability."""
from __future__ import annotations

DISPLAY_NAMES = {"claude": "Claude Code", "codex": "Codex"}
DEFAULT_INSTANCES = frozenset({"default", "personal"})


def title(agent: str, instance: str) -> str:
    name = DISPLAY_NAMES.get(agent, agent)
    return name if instance in DEFAULT_INSTANCES else f"{name} ({instance})"
