"""Tolerant JSONL reading for agent transcripts (skips blank and malformed lines)."""
from __future__ import annotations

import json
from datetime import datetime, timezone
from pathlib import Path


def load_entries(path: Path) -> list[dict]:
    entries: list[dict] = []
    try:
        with path.open("r", encoding="utf-8", errors="replace") as handle:
            for line in handle:
                try:
                    value = json.loads(line)
                except ValueError:
                    continue
                if isinstance(value, dict):
                    entries.append(value)
    except OSError:
        return []
    return entries


def parse_ts(value: object) -> datetime | None:
    try:
        return datetime.fromisoformat(str(value).replace("Z", "+00:00")).astimezone(timezone.utc)
    except ValueError:
        return None
