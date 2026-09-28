"""Claude Code background shells still running, from the session transcript (no platform hook exists for them).

Launch markers are accepted only at the start of a tool result, so output that merely echoes them (logs, greps)
is ignored; completions (`<task-id>`, any status incl. failed/stopped) only from real notification entries.
Agents are tracked by SubagentStart/SubagentStop hooks instead (core.tasks).
"""
from __future__ import annotations

import json
import re
from datetime import datetime, timedelta
from pathlib import Path

from core.jsonl import load_entries, parse_ts

MAX_AGE = timedelta(hours=3)
SHELL_LAUNCH = re.compile(
    r"\A(?:Command running in background with ID: (\w+)"
    r"|Command did not complete[^\n]*?moved to the background \(ID: (\w+)\))")
COMPLETION = re.compile(r"<task-id>(\w+)</task-id>")
NOTIFICATION_ENTRY_TYPES = ("queue-operation", "attachment")


def _tool_result_texts(entry: dict) -> list[str]:
    content = (entry.get("message") or {}).get("content")
    if entry.get("type") != "user" or not isinstance(content, list):
        return []
    texts = []
    for block in content:
        if not isinstance(block, dict) or block.get("type") != "tool_result":
            continue
        inner = block.get("content")
        if isinstance(inner, str):
            texts.append(inner)
        elif isinstance(inner, list):
            texts.append("\n".join(str(item.get("text", "")) for item in inner if isinstance(item, dict)))
    return texts


def _notification_text(entry: dict) -> str:
    if entry.get("type") in NOTIFICATION_ENTRY_TYPES:
        return json.dumps(entry, ensure_ascii=False)
    content = (entry.get("message") or {}).get("content")
    if entry.get("type") == "user" and isinstance(content, str) and content.lstrip().startswith("<task-notification>"):
        return content
    return ""


def scan(entries: list[dict], now: datetime) -> list[str]:
    running: dict[str, datetime | None] = {}
    for entry in entries:
        ts = parse_ts(entry.get("timestamp"))
        for text in _tool_result_texts(entry):
            match = SHELL_LAUNCH.match(text)
            if match:
                running[next(group for group in match.groups() if group)] = ts
        for task_id in COMPLETION.findall(_notification_text(entry)):
            running.pop(task_id, None)
    return [task_id for task_id, started in running.items() if started is None or now - started <= MAX_AGE]


def background_shells(transcript: Path, now: datetime) -> list[str]:
    return scan(load_entries(transcript), now)
