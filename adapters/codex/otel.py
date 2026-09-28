"""Codex CLI OpenTelemetry `codex.sse_event` (kind `response.completed`) → turn-usage.record data.

Observed with codex 0.157.1 (`service.name` "codex_exec"): counts arrive as a mix of ints and numeric strings;
identifiers are `conversation.id` + `event.timestamp` (no request or turn id), so per-turn totals come from the
rollout fallback (`token_usage_record.turn_id`). Personal fields (user.email, user.account_id) are never read.
"""
from __future__ import annotations

import uuid
from typing import Any, Mapping

from adapters.base import count


def map(attrs: Mapping[str, Any]) -> dict | None:
    if attrs.get("event.name") != "codex.sse_event" or attrs.get("event.kind") != "response.completed":
        return None
    session = str(attrs.get("conversation.id") or "unknown")[:240]
    source_event = f"{session}:{attrs.get('event.timestamp')}:{attrs.get('input_token_count')}:{attrs.get('output_token_count')}"
    return {
        "usageId": str(uuid.uuid5(uuid.NAMESPACE_URL, f"codex-otel:{source_event}")),
        "nativeSession": session,
        "nativeTurn": attrs.get("turn.id"),
        "promptId": None,
        "inputTokens": count(attrs.get("input_token_count")),
        "outputTokens": count(attrs.get("output_token_count")),
        "cachedInputTokens": count(attrs.get("cached_token_count")),
        "reasoningTokens": count(attrs.get("reasoning_token_count")),
        "availability": "exact",
        "source": "codex_otel",
        "sourceEventId": source_event,
    }
