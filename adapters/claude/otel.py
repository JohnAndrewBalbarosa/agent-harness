"""Claude Code OpenTelemetry `api_request` → turn-usage.record data (whitelisted fields only; spec R6).

One row per API request; `nativeTurn` is the OTel `prompt.id`, so per-turn totals are SUM(...) GROUP BY native_turn.
Reasoning tokens are not reported by OTel (the transcript fallback has them).
"""
from __future__ import annotations

import uuid
from typing import Any, Mapping

from adapters.base import count

API_REQUEST_EVENTS = ("api_request", "claude_code.api_request")


def map(attrs: Mapping[str, Any]) -> dict | None:
    if attrs.get("event.name") not in API_REQUEST_EVENTS:
        return None
    request_id = str(attrs.get("request_id") or attrs.get("client_request_id") or "")
    cache_read = count(attrs.get("cache_read_tokens"))
    return {
        "usageId": str(uuid.uuid5(uuid.NAMESPACE_URL, f"claude-otel:{request_id}")),
        "nativeSession": str(attrs.get("session.id") or "unknown")[:240],
        "nativeTurn": attrs.get("prompt.id"),
        "promptId": None,
        "inputTokens": count(attrs.get("input_tokens")) + cache_read + count(attrs.get("cache_creation_tokens")),
        "outputTokens": count(attrs.get("output_tokens")),
        "cachedInputTokens": cache_read,
        "reasoningTokens": None,
        "availability": "exact",
        "source": "claude_otel",
        "sourceEventId": request_id,
    }
