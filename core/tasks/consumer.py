"""Tasks consumer: records subagent.start / subagent.stop common events (event JSON on stdin)."""
from __future__ import annotations

import os
import sys
from datetime import datetime, timezone

from core.events import from_json
from core.tasks.state import default_store, record


def main() -> int:
    try:
        event = from_json(sys.stdin.buffer.read().decode("utf-8"))
        record(default_store(os.environ), event, datetime.now(timezone.utc))
    except (ValueError, KeyError, OSError, UnicodeDecodeError) as error:
        sys.stderr.write(f"tasks consumer: {type(error).__name__}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
