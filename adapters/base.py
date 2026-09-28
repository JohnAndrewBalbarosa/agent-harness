"""Adapter contract (spec §7): each agent's adapter turns native hook payloads into common events and
turns consumer results back into the agent's native hook stdout."""
from __future__ import annotations

import importlib
import json
import re
from dataclasses import dataclass
from types import MappingProxyType
from typing import Any, Callable, Mapping, Protocol, Sequence

from core.events import Event

ADAPTER_NAME = re.compile(r"^[a-z][a-z0-9_]*$")


@dataclass(frozen=True)
class ConsumerResult:
    name: str
    exit_code: int
    stdout: bytes


@dataclass(frozen=True)
class TurnUsage:
    input_tokens: int
    output_tokens: int
    cached_input_tokens: int
    reasoning_tokens: int | None
    api_calls: int
    turn_id: str | None


class Adapter(Protocol):
    name: str

    def instance(self, env: Mapping[str, str]) -> str: ...

    def normalize(self, native_event: str | None, payload: Mapping[str, Any],
                  env: Mapping[str, str] | None = None) -> Event | None: ...

    def respond(self, event: Event, results: Sequence[ConsumerResult]) -> str: ...

    def safe_default(self, native_event: str | None) -> str: ...


def load_adapter(name: str) -> Adapter:
    if not ADAPTER_NAME.match(name or ""):
        raise KeyError(name)
    try:
        module = importlib.import_module(f"adapters.{name}.adapter")
    except ModuleNotFoundError as error:
        raise KeyError(name) from error
    return module.ADAPTER


def successful_output(results: Sequence[ConsumerResult], name: str) -> str:
    for result in results:
        if result.name == name and result.exit_code == 0 and result.stdout.strip():
            return result.stdout.decode("utf-8", errors="replace").strip()
    return ""


def session_context_response(results: Sequence[ConsumerResult]) -> str:
    """SessionStart context: harness `context` consumer text (wrapped) first, else a ready bridge envelope."""
    context = successful_output(results, "context")
    if context:
        return json.dumps({"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": context}},
                          ensure_ascii=False)
    return successful_output(results, "herdr-bridge")


def count(value: Any) -> int:
    """Non-negative token count from an int or numeric string (OTLP JSON encodes int64 as strings)."""
    try:
        number = int(value)
    except (TypeError, ValueError):
        return 0
    return number if number >= 0 else 0


def subagent_data(payload: Mapping[str, Any]) -> dict[str, Any]:
    return {"subagent": {"id": payload.get("agent_id"), "type": payload.get("agent_type")}}


def tool_data(payload: Mapping[str, Any]) -> dict[str, Any]:
    return {"tool": {"name": payload.get("tool_name"), "input": payload.get("tool_input"),
                     "response": payload.get("tool_response")}}


Extractor = Callable[[Mapping[str, Any]], dict[str, Any]]

# Native events whose payloads are identical across Codex CLI and Claude Code (same snake_case fields).
SHARED_MAPPING: Mapping[str, tuple[str, Extractor]] = MappingProxyType({
    "SessionStart": ("session.start", lambda p: {"source": p.get("source")}),
    "SessionEnd": ("session.end", lambda p: {"reason": p.get("reason")}),
    "UserPromptSubmit": ("prompt.submit", lambda p: {"prompt": p.get("prompt")}),
    "PostToolUse": ("tool.completed", tool_data),
    "Stop": ("turn.stop", lambda p: {"last_message": p.get("last_assistant_message"),
                                     "stop_hook_active": bool(p.get("stop_hook_active"))}),
    "SubagentStart": ("subagent.start", subagent_data),
    "SubagentStop": ("subagent.stop", lambda p: {**subagent_data(p), "last_message": p.get("last_assistant_message")}),
})
