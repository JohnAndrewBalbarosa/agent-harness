"""Claude Code turn token usage from the session transcript (fallback for OTel; carries thinking tokens).

A turn starts at the last user entry that is not meta and not a tool result (real prompts and background-task
notifications). One API response is split across several transcript lines with identical usage, so responses are
deduplicated by `message.id`.
"""
from __future__ import annotations

from pathlib import Path

from adapters.base import TurnUsage, count
from core.jsonl import load_entries

USAGE_FIELDS = ("input_tokens", "output_tokens", "cache_creation_input_tokens", "cache_read_input_tokens")


def is_turn_start(entry: dict) -> bool:
    if entry.get("type") != "user" or entry.get("isMeta"):
        return False
    content = (entry.get("message") or {}).get("content")
    if isinstance(content, str):
        return True
    if isinstance(content, list):
        return not any(isinstance(block, dict) and block.get("type") == "tool_result" for block in content)
    return False


def sum_usage(entries: list[dict], turn_id: str | None) -> TurnUsage:
    totals = {field: 0 for field in USAGE_FIELDS}
    thinking, seen = None, set()
    for entry in entries:
        message = entry.get("message") or {}
        usage = message.get("usage")
        if entry.get("type") != "assistant" or not isinstance(usage, dict):
            continue
        key = message.get("id") or entry.get("requestId") or entry.get("uuid")
        if key in seen:
            continue
        seen.add(key)
        for field in USAGE_FIELDS:
            totals[field] += count(usage.get(field))
        details = usage.get("output_tokens_details")
        if isinstance(details, dict) and details.get("thinking_tokens") is not None:
            thinking = (thinking or 0) + count(details["thinking_tokens"])
    cache_read = totals["cache_read_input_tokens"]
    return TurnUsage(
        input_tokens=totals["input_tokens"] + cache_read + totals["cache_creation_input_tokens"],
        output_tokens=totals["output_tokens"], cached_input_tokens=cache_read, reasoning_tokens=thinking,
        api_calls=len(seen), turn_id=turn_id)


def usage(path: Path, turn_id: str | None) -> TurnUsage:
    """Usage of the latest turn (Claude's Stop payload has no turn id, so `turn_id` is ignored)."""
    entries = load_entries(path)
    start = max((i for i, entry in enumerate(entries) if is_turn_start(entry)), default=None)
    turn_entries = entries if start is None else entries[start + 1:]
    return sum_usage(turn_entries, None if start is None else entries[start].get("uuid"))


def context_tokens(path: Path) -> int | None:
    """Prompt size of the latest API request (input + cache read + cache write): what the next request re-sends."""
    for entry in reversed(load_entries(path)):
        message = entry.get("message") if entry.get("type") == "assistant" else None
        usage = message.get("usage") if isinstance(message, dict) else None
        if isinstance(usage, dict):
            return count(usage.get("input_tokens")) + count(usage.get("cache_read_input_tokens")) + \
                count(usage.get("cache_creation_input_tokens"))
    return None
