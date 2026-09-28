"""turn.stop consumer: warn when a session's context gets expensive.

Every request re-sends the whole context, so a long session costs more per step (measured 2026-09-28: 97-98% of all
input tokens were cache re-reads of long sessions). When the latest request's prompt reaches CONTEXT_WARN_TOKENS
(default 150000), notify once per 50K band per session: compact or start a new session.
"""
from __future__ import annotations

import importlib
import json
import os
import re
import sys
from pathlib import Path
from typing import Callable, Mapping

from core import lifecycle
from core.events import Event, from_json
from core.locking import file_lock

ROOT = Path(__file__).resolve().parents[2]
BAND = 50_000
DEFAULT_THRESHOLD = 150_000
_UNSAFE = re.compile(r"[^A-Za-z0-9_-]")


def _threshold(env: Mapping[str, str]) -> int:
    try:
        return int(env.get("CONTEXT_WARN_TOKENS") or DEFAULT_THRESHOLD)
    except ValueError:
        return DEFAULT_THRESHOLD


def _message(agent: str, tokens: int) -> str:
    return (f"{agent}: {round(tokens / 1000)}K tokens per request. "
            "Mag-/compact o magsimula ng bagong session para bumaba ang gastos bawat hakbang.")


def handle(event: Event, env: Mapping[str, str], notify: Callable[[str], None]) -> str:
    log = Path(env.get("AH_LOG_DIR") or ROOT / "var" / "logs") / "context-guard.lifecycle.jsonl"
    if event.event != "turn.stop" or not event.transcript_path:
        return "ignored"
    try:
        transcript = importlib.import_module(f"adapters.{event.agent}.transcript")
        tokens = transcript.context_tokens(Path(event.transcript_path))
    except (ModuleNotFoundError, AttributeError):
        return "unsupported"
    threshold = _threshold(env)
    if tokens is None or tokens < threshold:
        return "below"
    band = tokens // BAND
    state_dir = Path(env.get("AH_CONTEXT_GUARD_DIR") or ROOT / "var" / "context-guard")
    state_dir.mkdir(parents=True, exist_ok=True)
    state = state_dir / f"{_UNSAFE.sub('_', event.agent)}-{_UNSAFE.sub('_', event.session_id)[:120]}.json"
    with file_lock(state):
        try:
            warned = int(json.loads(state.read_text(encoding="utf-8")).get("band", -1))
        except (OSError, ValueError, AttributeError):
            warned = -1
        if band <= warned:
            return "already_warned"
        state.write_text(json.dumps({"band": band, "tokens": tokens}), encoding="utf-8")
    notify(_message(event.agent, tokens))
    lifecycle.emit(log, "context-guard", "context.warned", "succeeded", agent=event.agent, instance=event.instance,
                   session=event.session_id, tokens=tokens, threshold=threshold)
    return "warned"


def _notify(message: str) -> None:
    from core.notify import consumer as notify_consumer
    from core.notify.decide import Decision
    from core.notify.deliver import deliver
    herdr = notify_consumer._herdr_bin(os.environ)
    deliver(Decision("toast", "context-budget", "request", message), "agent-harness: context", herdr)


def main() -> int:
    try:
        event = from_json(sys.stdin.buffer.read().decode("utf-8"))
        handle(event, os.environ, _notify)
    except Exception as error:  # consumers never fail the hook; the router logs the exit code
        print(f"context guard: {type(error).__name__}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
