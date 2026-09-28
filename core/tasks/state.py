"""Background-work tracking per agent session (spec §9 tasks consumer).

Subagents come from `subagent.start` / `subagent.stop` common events (platform hooks, authoritative). Background
shells have no hook, so an adapter MAY provide `adapters.<agent>.background.background_shells(transcript, now)`.
Entries older than MAX_AGE are ignored so a lost stop event cannot suppress notifications forever.
"""
from __future__ import annotations

import importlib
import json
import os
import re
from datetime import datetime, timedelta
from pathlib import Path
from typing import Mapping

from core.events import Event
from core.locking import file_lock

MAX_AGE = timedelta(hours=3)
ROOT = Path(__file__).resolve().parents[2]
_UNSAFE = re.compile(r"[^A-Za-z0-9_-]")


def default_store(env: Mapping[str, str]) -> Path:
    return Path(env.get("AH_TASKS_DIR") or ROOT / "var" / "tasks")


def apply_event(state: Mapping[str, str], event: Event, now: datetime) -> dict[str, str]:
    agent_id = str((event.data.get("subagent") or {}).get("id") or "")
    if not agent_id or event.event not in ("subagent.start", "subagent.stop"):
        return dict(state)
    if event.event == "subagent.start":
        return {**state, agent_id: now.isoformat()}
    return {key: value for key, value in state.items() if key != agent_id}


def _parse(value: str) -> datetime | None:
    try:
        return datetime.fromisoformat(value)
    except (TypeError, ValueError):
        return None


def fresh_agents(state: Mapping[str, str], now: datetime) -> list[str]:
    return [agent_id for agent_id, started in state.items()
            if (ts := _parse(started)) is not None and now - ts <= MAX_AGE]


def _path(store: Path, agent: str, session_id: str) -> Path:
    return store / f"{_UNSAFE.sub('_', agent)}-{_UNSAFE.sub('_', session_id)[:120]}.json"


def _load(path: Path) -> dict[str, str]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    return value if isinstance(value, dict) else {}


def record(store: Path, event: Event, now: datetime) -> None:
    path = _path(store, event.agent, event.session_id)
    store.mkdir(parents=True, exist_ok=True)
    with file_lock(path):
        updated = apply_event(_load(path), event, now)
        temporary = path.with_suffix(".tmp")
        temporary.write_text(json.dumps(updated), encoding="utf-8")
        os.replace(temporary, path)


def _background_shells(agent: str, transcript_path: str | None, now: datetime) -> list[str]:
    if not transcript_path:
        return []
    try:
        module = importlib.import_module(f"adapters.{agent}.background")
    except ModuleNotFoundError:
        return []
    try:
        return list(module.background_shells(Path(transcript_path), now))
    except (OSError, ValueError):
        return []


def outstanding(store: Path, agent: str, session_id: str, transcript_path: str | None, now: datetime) -> list[str]:
    path = _path(store, agent, session_id)
    if not path.exists():
        return _background_shells(agent, transcript_path, now)
    # Read under the writer's lock: on Windows an open reader makes os.replace fail with PermissionError.
    with file_lock(path):
        agents = fresh_agents(_load(path), now)
    return agents + _background_shells(agent, transcript_path, now)
