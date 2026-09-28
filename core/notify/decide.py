"""Pure notification decision engine (spec §9, ported from ~/.claude/scripts/notify-gate.ps1).

prompt.submit marks the session pending; turn.stop toasts "done" once the prompt is finished and no background
work (subagents, background shells) is still running; attention.needed toasts "request" (idle prompts wait for
background work). A global rate limit caps toasts per window.
"""
from __future__ import annotations

from dataclasses import dataclass, replace
from typing import Literal, Sequence

from core.events import Event

DONE_MESSAGE = "Tapos na ang prompt!"
ATTENTION_MESSAGE = "Kailangan ng input mo!"


@dataclass(frozen=True)
class NotifyState:
    pending: bool
    recent_toasts: tuple[int, ...]


@dataclass(frozen=True)
class Decision:
    action: Literal["toast", "defer", "skip", "mark"]
    reason: str
    sound: Literal["done", "request"] | None = None
    message: str | None = None


def _within_rate(recent: tuple[int, ...], now_s: int, rate: tuple[int, int]) -> tuple[bool, tuple[int, ...]]:
    limit, window_s = rate
    kept = tuple(t for t in recent if now_s - window_s <= t <= now_s)
    if len(kept) >= limit:
        return False, kept
    return True, kept + (now_s,)


def _toast(state: NotifyState, now_s: int, rate: tuple[int, int], sound: Literal["done", "request"],
           message: str, pending: bool) -> tuple[Decision, NotifyState]:
    allowed, recent = _within_rate(state.recent_toasts, now_s, rate)
    new_state = NotifyState(pending=pending, recent_toasts=recent)
    if not allowed:
        return Decision("skip", "rate-limited"), new_state
    return Decision("toast", sound, sound, message), new_state


def decide(event: Event, state: NotifyState, outstanding_tasks: Sequence[str], now_s: int,
           rate: tuple[int, int]) -> tuple[Decision, NotifyState]:
    if event.event == "prompt.submit":
        return Decision("mark", "pending"), replace(state, pending=True)
    if event.event == "turn.stop":
        if event.data.get("stop_hook_active"):
            return Decision("skip", "stop-hook-active"), state
        if not state.pending:
            return Decision("skip", "no-pending-prompt"), state
        if outstanding_tasks:
            return Decision("defer", "background-tasks"), state
        return _toast(state, now_s, rate, "done", DONE_MESSAGE, pending=False)
    if event.event == "attention.needed":
        attention = event.data.get("attention") or {}
        if attention.get("kind") == "idle" and outstanding_tasks:
            return Decision("defer", "background-tasks"), state
        return _toast(state, now_s, rate, "request", attention.get("message") or ATTENTION_MESSAGE, pending=state.pending)
    return Decision("skip", "unhandled-event"), state
