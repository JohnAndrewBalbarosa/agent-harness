"""turn.stop consumer: per-turn token usage from the agent's transcript → turn-usage.record (`source='<agent>_hook'`).

Complements OTel (`<agent>_otel`): it carries reasoning tokens and exact per-turn totals for agents whose OTel
events have no turn id. A bounded local copy is kept in <root>/var/usage/turn-usage.jsonl (AH_USAGE_DIR).
"""
from __future__ import annotations

import importlib
import json
import os
import sys
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Mapping

from adapters.base import TurnUsage
from core.events import Event, from_json
from core.locking import file_lock
from core.usage.obs_client import GLOBAL_PROJECT_ID, ROOT, enqueue_usage, spool_project

LOG_MAX_RECORDS = 500


def build_record(event: Event, usage: TurnUsage, project_id: str) -> dict:
    turn = usage.turn_id or event.turn_id
    return {
        "usageId": str(uuid.uuid5(uuid.NAMESPACE_URL, f"{event.agent}-hook:{event.session_id}:{turn}")),
        "projectId": project_id, "nativeSession": event.session_id, "nativeTurn": turn, "promptId": None,
        "inputTokens": usage.input_tokens, "outputTokens": usage.output_tokens,
        "cachedInputTokens": usage.cached_input_tokens, "reasoningTokens": usage.reasoning_tokens,
        "availability": "exact" if usage.api_calls > 0 else "unavailable",
        "source": f"{event.agent}_hook", "sourceEventId": f"{event.session_id}:{turn}:{event.native_event}",
    }


def _append_local(directory: Path, record: dict) -> None:
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / "turn-usage.jsonl"
    with file_lock(path):
        lines = [l for l in path.read_text(encoding="utf-8").splitlines() if l.strip()] if path.exists() else []
        lines = lines[-(LOG_MAX_RECORDS - 1):] + [json.dumps(record, ensure_ascii=False)]
        temporary = path.with_suffix(".tmp")
        temporary.write_text("\n".join(lines) + "\n", encoding="utf-8")
        os.replace(temporary, path)


def handle(event: Event, env: Mapping[str, str]) -> dict | None:
    if event.event != "turn.stop" or not event.transcript_path:
        return None
    transcript = importlib.import_module(f"adapters.{event.agent}.transcript")
    usage = transcript.usage(Path(event.transcript_path), event.turn_id)
    record = build_record(event, usage, spool_project(event.session_id) or GLOBAL_PROJECT_ID)
    enqueued = env.get("AH_USAGE_DRY_RUN") != "1" and enqueue_usage([record]) == 1
    _append_local(Path(env.get("AH_USAGE_DIR") or ROOT / "var" / "usage"),
                  {**record, "recordedAt": datetime.now(timezone.utc).isoformat(), "apiCalls": usage.api_calls,
                   "agent": event.agent, "instance": event.instance, "enqueued": enqueued})
    return record


def main() -> int:
    try:
        handle(from_json(sys.stdin.buffer.read().decode("utf-8")), os.environ)
    except (ValueError, KeyError, OSError, UnicodeDecodeError, ModuleNotFoundError) as error:
        sys.stderr.write(f"usage fallback: {type(error).__name__}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
