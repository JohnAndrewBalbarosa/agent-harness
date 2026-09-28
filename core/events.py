"""Common event contract (`agent-harness.event/v1`).

Adapters translate each agent's native hook payload into an `Event`; consumers only ever see `Event`s
(spec §6, requirement R2). Events are immutable: `data` and `native_payload` are read-only mappings.
"""
from __future__ import annotations

import json
from dataclasses import dataclass, field
from datetime import datetime, timezone
from types import MappingProxyType
from typing import Any, Mapping

SCHEMA = "agent-harness.event/v1"
EVENT_TYPES = frozenset({
    "session.start", "session.end", "prompt.submit", "tool.completed",
    "turn.stop", "attention.needed", "subagent.start", "subagent.stop",
})
NATIVE_PAYLOAD_LIMIT_BYTES = 8 * 1024


def _frozen(mapping: Mapping[str, Any] | None) -> Mapping[str, Any]:
    return MappingProxyType(dict(mapping or {}))


@dataclass(frozen=True)
class Event:
    event: str
    agent: str
    instance: str
    session_id: str
    cwd: str | None
    transcript_path: str | None
    ts: str
    turn_id: str | None = None
    data: Mapping[str, Any] = field(default_factory=lambda: _frozen(None))
    native_event: str = ""
    native_payload: Mapping[str, Any] = field(default_factory=lambda: _frozen(None))


def _utc_now() -> str:
    now = datetime.now(timezone.utc)
    return now.strftime("%Y-%m-%dT%H:%M:%S.") + f"{now.microsecond // 1000:03d}Z"


def _validate_event_type(event: str) -> None:
    if event not in EVENT_TYPES:
        raise ValueError(f"unknown common event: {event!r}")


def _optional_str(value: Any) -> str | None:
    return str(value) if value not in (None, "") else None


def make_event(event: str, agent: str, instance: str, payload: Mapping[str, Any], native_event: str,
               data: Mapping[str, Any] | None = None) -> Event:
    """Build an Event from a native payload whose common keys use snake_case (Codex and Claude both do)."""
    _validate_event_type(event)
    event_data = dict(data or {})
    native = dict(payload)
    if len(json.dumps(native, ensure_ascii=False, default=str).encode("utf-8")) > NATIVE_PAYLOAD_LIMIT_BYTES:
        native = {}
        event_data["native_truncated"] = True
    return Event(
        event=event, agent=agent, instance=instance,
        session_id=str(payload.get("session_id") or "unknown"),
        cwd=_optional_str(payload.get("cwd")),
        transcript_path=_optional_str(payload.get("transcript_path")),
        ts=_utc_now(),
        turn_id=_optional_str(payload.get("turn_id")),
        data=_frozen(event_data), native_event=native_event, native_payload=_frozen(native),
    )


def to_json(event: Event) -> str:
    return json.dumps({
        "schema": SCHEMA, "event": event.event, "agent": event.agent, "instance": event.instance,
        "session_id": event.session_id, "turn_id": event.turn_id, "cwd": event.cwd,
        "transcript_path": event.transcript_path, "ts": event.ts, "data": dict(event.data),
        "native": {"event": event.native_event, "payload": dict(event.native_payload)},
    }, ensure_ascii=False, default=str)


def from_json(text: str) -> Event:
    doc = json.loads(text)
    if doc.get("schema") != SCHEMA:
        raise ValueError(f"unsupported schema: {doc.get('schema')!r}")
    _validate_event_type(doc.get("event", ""))
    native = doc.get("native") or {}
    return Event(
        event=doc["event"], agent=doc["agent"], instance=doc["instance"], session_id=doc["session_id"],
        cwd=doc.get("cwd"), transcript_path=doc.get("transcript_path"), ts=doc["ts"], turn_id=doc.get("turn_id"),
        data=_frozen(doc.get("data")), native_event=native.get("event", ""), native_payload=_frozen(native.get("payload")),
    )
