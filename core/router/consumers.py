"""Consumer registry: which consumers run for which common event, and how they are launched.

`stdin="event"` consumers receive the common event JSON; `stdin="native"` consumers (vendored legacy tools that
already understand the shared snake_case payload) receive the agent's raw payload.
"""
from __future__ import annotations

import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Literal, Mapping

from core.events import EVENT_TYPES, Event

ROOT = Path(__file__).resolve().parents[2]
TOOLS = ROOT / "tools"
DEFAULT_TIMEOUT_S = 25
PERMISSION_TIMEOUT_S = 590  # the status orb may wait on the user while a permission prompt is open


@dataclass(frozen=True)
class Consumer:
    name: str
    events: frozenset[str]
    command: tuple[str, ...]
    stdin: Literal["event", "native"]
    timeout_s: int


def _module(module: str) -> tuple[str, ...]:
    return (sys.executable, "-m", module)


def _harness_consumers() -> list[Consumer]:
    return [
        # native: the common event may drop oversized payloads, and obs needs full tool inputs/responses
        Consumer("obs", EVENT_TYPES - {"subagent.start", "subagent.stop", "attention.needed"},
                 _module("core.consumers.obs"), "native", DEFAULT_TIMEOUT_S),
        Consumer("notify", frozenset({"prompt.submit", "turn.stop", "attention.needed"}),
                 _module("core.notify.consumer"), "event", DEFAULT_TIMEOUT_S),
        Consumer("tasks", frozenset({"subagent.start", "subagent.stop"}), _module("core.tasks.consumer"), "event", 10),
        Consumer("usage-fallback", frozenset({"turn.stop"}), _module("core.usage.fallback"), "event", DEFAULT_TIMEOUT_S),
        Consumer("context-guard", frozenset({"turn.stop"}), _module("core.usage.context_guard"), "event", DEFAULT_TIMEOUT_S),
        Consumer("context", frozenset({"session.start"}), _module("core.context.consumer"), "event", 20),
        Consumer("services", frozenset({"session.start"}), _module("core.services.consumer"), "event", 10),
    ]


def _orb(event: Event) -> list[Consumer]:
    python = TOOLS / "codex-status-orb" / ".venv" / "Scripts" / "python.exe"
    if not python.exists():
        return []
    timeout = PERMISSION_TIMEOUT_S if event.native_event == "PermissionRequest" else DEFAULT_TIMEOUT_S
    return [Consumer("orb", EVENT_TYPES - {"subagent.start", "subagent.stop"},
                     (str(python), str(TOOLS / "codex-status-orb" / "hook.py")), "native", timeout)]


def _herdr_bridge(env: Mapping[str, str]) -> list[Consumer]:
    if env.get("HERDR_ENV") != "1" or not env.get("HERDR_PANE_ID"):
        return []
    return [Consumer("herdr-bridge", EVENT_TYPES - {"subagent.start", "subagent.stop"},
                     (sys.executable, str(TOOLS / "herdr-bridge" / "hook.py")), "native", DEFAULT_TIMEOUT_S)]


def consumers_for(event: Event, env: Mapping[str, str]) -> list[Consumer]:
    candidates = _harness_consumers() + _orb(event) + _herdr_bridge(env)
    return [consumer for consumer in candidates if event.event in consumer.events]
