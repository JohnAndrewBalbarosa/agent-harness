"""Claude Code adapter. Payload field names follow https://code.claude.com/docs/en/hooks."""
from __future__ import annotations

from typing import Any, Mapping, Sequence

from adapters.base import SHARED_MAPPING, ConsumerResult, session_context_response
from core.events import Event, make_event

ATTENTION_KINDS = {"permission_prompt": "permission", "idle_prompt": "idle"}


class ClaudeAdapter:
    name = "claude"

    def instance(self, env: Mapping[str, str]) -> str:
        return env.get("AH_INSTANCE") or "default"

    def normalize(self, native_event: str | None, payload: Mapping[str, Any], env: Mapping[str, str] | None = None) -> Event | None:
        native = native_event or str(payload.get("hook_event_name") or "")
        instance = self.instance(env or {})
        if native == "Notification":
            kind = ATTENTION_KINDS.get(str(payload.get("notification_type") or ""))
            if kind is None:
                return None
            return make_event("attention.needed", self.name, instance, payload, native,
                              {"attention": {"kind": kind, "message": payload.get("message")}})
        if native not in SHARED_MAPPING:
            return None
        common, extract = SHARED_MAPPING[native]
        return make_event(common, self.name, instance, payload, native, extract(payload))

    def respond(self, event: Event, results: Sequence[ConsumerResult]) -> str:
        return session_context_response(results) if event.event == "session.start" else ""

    def safe_default(self, native_event: str | None) -> str:
        return ""


ADAPTER = ClaudeAdapter()
