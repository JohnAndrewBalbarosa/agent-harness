"""Codex CLI turn token usage from the rollout file (per-turn source; OTel has no turn id).

Uses `token_usage_record` entries (one per model response, with `turn_id` and `response_id`), deduplicated by
`response_id`. `event_msg/token_count` is not used: it can repeat an identical snapshot, which would double count.
OpenAI `input_tokens` already include cached tokens.
"""
from __future__ import annotations

from pathlib import Path

from adapters.base import TurnUsage, count
from core.jsonl import load_entries


def usage(path: Path, turn_id: str | None) -> TurnUsage:
    records = [e.get("payload") or {} for e in load_entries(path) if e.get("type") == "token_usage_record"]
    target = turn_id or next((r.get("turn_id") for r in reversed(records) if r.get("turn_id")), None)
    totals = {"input": 0, "cached": 0, "output": 0, "reasoning": 0}
    seen: set[str] = set()
    for record in records:
        response = str(record.get("response_id") or "")
        if record.get("turn_id") != target or response in seen:
            continue
        seen.add(response)
        tokens = record.get("usage") or {}
        totals["input"] += count(tokens.get("input_tokens"))
        totals["cached"] += count(tokens.get("cached_input_tokens"))
        totals["output"] += count(tokens.get("output_tokens"))
        totals["reasoning"] += count(tokens.get("reasoning_output_tokens"))
    return TurnUsage(input_tokens=totals["input"], output_tokens=totals["output"], cached_input_tokens=totals["cached"],
                     reasoning_tokens=totals["reasoning"], api_calls=len(seen), turn_id=target)
