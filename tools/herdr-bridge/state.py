from __future__ import annotations

import time
from dataclasses import dataclass
from typing import Any


PRIORITY = {"idle": 0, "working": 1, "ready": 2, "needs_input": 3, "blocked": 4}
PROJECTION = {
    "idle": "idle",
    "working": "working",
    "needs_input": "blocked",
    "ready": "idle",
    "blocked": "blocked",
}


@dataclass(frozen=True)
class StateDecision:
    canonical: str
    projected: str
    reason: str
    authority: str
    signal: str


def decision_for_hook(event: str, payload: dict[str, Any]) -> StateDecision | None:
    if event == "SessionStart":
        return StateDecision("idle", "idle", "session_started", "codex_hook", event)
    if event == "UserPromptSubmit":
        return StateDecision("working", "working", "prompt_submitted", "codex_hook", event)
    if event == "PermissionRequest":
        reason = str(payload.get("permission_reason") or payload.get("reason") or "permission")[:160]
        return StateDecision("needs_input", "blocked", reason, "codex_hook", event)
    if event == "PostToolUse":
        return StateDecision("working", "working", "tool_completed", "codex_hook", event)
    if event == "Stop":
        return StateDecision("ready", "idle", "turn_stopped", "codex_hook", event)
    return None


def ranking_key(session: dict[str, Any], now: float | None = None) -> tuple[int, float, str]:
    current = now if now is not None else time.time()
    state = str(session.get("canonical_state") or session.get("status") or "idle")
    entered = float(session.get("state_entered_at") or session.get("updated_at") or current)
    # needs_input deliberately uses oldest-first; all other equal states use newest-first.
    time_key = -entered if state == "needs_input" else entered
    return PRIORITY.get(state, -1), time_key, str(session.get("session_id") or "")


def highest_priority(sessions: list[dict[str, Any]]) -> dict[str, Any] | None:
    if not sessions:
        return None
    return max(sessions, key=ranking_key)
