"""Codex CLI adapter. Payload field names follow codex-rs/hooks/schema/generated/*.command.input.schema.json."""
from __future__ import annotations

import json
from pathlib import PureWindowsPath
from typing import Any, Mapping, Sequence

from adapters.base import SHARED_MAPPING, ConsumerResult, session_context_response
from core.events import Event, make_event

CONTINUE = json.dumps({"continue": True})
HOME_INSTANCES = {".codex": "personal", ".codex-cy": "cy", ".codex-feu": "feu"}
MAPPING = {
    **SHARED_MAPPING,
    "PermissionRequest": ("attention.needed", lambda p: {"attention": {
        "kind": "permission", "tool": p.get("tool_name"), "message": f"Permission requested: {p.get('tool_name')}"}}),
}


class CodexAdapter:
    name = "codex"

    def instance(self, env: Mapping[str, str]) -> str:
        if env.get("AH_INSTANCE"):
            return env["AH_INSTANCE"]
        home = env.get("CODEX_HOME")
        if not home:
            return "personal"
        leaf = PureWindowsPath(home).name
        return HOME_INSTANCES.get(leaf, leaf.removeprefix(".codex-") or "personal")

    def normalize(self, native_event: str | None, payload: Mapping[str, Any], env: Mapping[str, str] | None = None) -> Event | None:
        native = native_event or str(payload.get("hook_event_name") or "")
        if native not in MAPPING:
            return None
        common, extract = MAPPING[native]
        return make_event(common, self.name, self.instance(env or {}), payload, native, extract(payload))

    def respond(self, event: Event, results: Sequence[ConsumerResult]) -> str:
        if event.event == "session.start":
            return session_context_response(results)
        if event.event == "turn.stop":
            return CONTINUE
        return ""

    def safe_default(self, native_event: str | None) -> str:
        return CONTINUE if native_event == "Stop" else ""


ADAPTER = CodexAdapter()
