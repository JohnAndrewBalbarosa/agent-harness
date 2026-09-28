"""Bounded, line-atomic JSONL lifecycle log shared by the router and consumers.

Windows' CRT emulates O_APPEND as seek-then-write, so concurrent hooks can interleave or overwrite lines.
Every write therefore holds a cross-process lock on a sidecar `<log>.lock` file. The file rotates at
MAX_BYTES keeping BACKUPS old copies. Logging never raises: a hook must not fail because of its log (R3).
"""
from __future__ import annotations

import json
import os
import threading
from datetime import datetime, timezone
from pathlib import Path

from core.locking import file_lock

MAX_BYTES = 1_048_576
BACKUPS = 2
_THREAD_LOCK = threading.Lock()


def _rotate(path: Path) -> None:
    if not path.exists() or path.stat().st_size < MAX_BYTES:
        return
    for index in range(BACKUPS, 1, -1):
        older = path.with_name(f"{path.name}.{index - 1}")
        if older.exists():
            os.replace(older, path.with_name(f"{path.name}.{index}"))
    os.replace(path, path.with_name(f"{path.name}.1"))


def _timestamp() -> str:
    now = datetime.now(timezone.utc)
    return now.strftime("%Y-%m-%dT%H:%M:%S.") + f"{now.microsecond // 1000:03d}Z"


def emit(path: Path, component: str, operation: str, outcome: str, **details: object) -> None:
    record = {"timestamp": _timestamp(), "component": component, "operation": operation,
              "outcome": outcome, "details": details}
    line = (json.dumps(record, ensure_ascii=False, separators=(",", ":"), default=str) + "\n").encode("utf-8")
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        with _THREAD_LOCK, file_lock(path):
            _rotate(path)
            with open(path, "ab") as stream:
                stream.write(line)
    except (OSError, ValueError):
        pass
