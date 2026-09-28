"""Enqueue `turn-usage.record` operations through the vendored observability client (tools/observability-client).

Project mapping: the obs spool's `context` table maps a native session to its registered project; unknown sessions
use the global agent project (same UUID the Codex runtime has always used).
"""
from __future__ import annotations

import os
import sqlite3
import sys
import uuid
from pathlib import Path
from typing import Iterable

ROOT = Path(__file__).resolve().parents[2]
OBS_DIR = ROOT / "tools" / "observability-client"
GLOBAL_PROJECT_ID = str(uuid.uuid5(uuid.NAMESPACE_URL, "codex-observability:global-agent-project"))


def spool_path() -> Path:
    return Path(os.environ.get("AH_OBS_SPOOL") or OBS_DIR / "var" / "spool.sqlite3")


def spool_project(native_session: str) -> str | None:
    try:
        conn = sqlite3.connect(f"file:{spool_path()}?mode=ro", uri=True, timeout=2)
        try:
            row = conn.execute("SELECT project_id FROM context WHERE native_session=?", (native_session,)).fetchone()
        finally:
            conn.close()
        return row[0] if row else None
    except sqlite3.Error:
        return None


def enqueue_usage(records: Iterable[dict]) -> int:
    records = list(records)
    if not records:
        return 0
    sys.path.insert(0, str(OBS_DIR))
    import obs  # vendored observability client
    import hook as obs_hook  # reuse its detached spool flush

    obs.enqueue([obs.operation("turn-usage.record", record) for record in records], flush_now=False)
    obs_hook.schedule_flush()
    return len(records)
