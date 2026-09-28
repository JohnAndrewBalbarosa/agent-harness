from __future__ import annotations

import json
import os
import sys
import time
from pathlib import Path
from typing import Any

from status_common import claim_hook_event, checkpoint, launch_orb, log, process_context, remove_session, safe_session_id, write_session

STATUS_BY_EVENT = {
    "SessionStart": "idle",
    "UserPromptSubmit": "running",
    "PermissionRequest": "needs_input",
    "PostToolUse": "running",
    "Stop": "ready",
}


def read_payload() -> dict[str, Any]:
    try:
        value = json.load(sys.stdin)
        return value if isinstance(value, dict) else {}
    except Exception:
        return {}


def neutral_output(event_name: str) -> None:
    if event_name == "Stop":
        sys.stdout.write('{"continue":true}\n')
        sys.stdout.flush()


def main() -> int:
    hook_received_ns = time.time_ns()
    payload = read_payload()
    event_name = str(payload.get("hook_event_name") or payload.get("hookEventName") or os.environ.get("CODEX_STATUS_EVENT") or "")
    session_id = safe_session_id(payload.get("session_id") or payload.get("sessionId"))
    try:
        if not claim_hook_event(payload):
            checkpoint("hook_duplicate_skipped", event=event_name, session=session_id)
        elif event_name == "SessionEnd":
            remove_session(session_id)
        elif event_name in STATUS_BY_EVENT:
            now_ns = time.time_ns()
            current = {
                "session_id": session_id,
                "event": event_name,
                "status": STATUS_BY_EVENT[event_name],
                "cwd": str(payload.get("cwd") or os.getcwd()),
                "transcript_path": str(payload.get("transcript_path") or payload.get("transcriptPath") or ""),
                "event_ns": now_ns,
                "updated_at": time.time(),
                "hook_received_ns": hook_received_ns,
                "herdr_managed": os.environ.get("HERDR_ENV") == "1" and bool(os.environ.get("HERDR_PANE_ID")),
                "herdr_pane_id": str(os.environ.get("HERDR_PANE_ID") or ""),
            }
            current.update(process_context())
            current["state_written_ns"] = time.time_ns()
            write_session(current)
            checkpoint("hook_processed", event=event_name, session=session_id, status=current["status"])
            if event_name == "SessionStart":
                launch_orb()  # with ORB_UI=0 the orb runs headless: same logic, no window
    except Exception as exc:
        log(f"hook-error event={event_name!r} session={session_id!r} error={exc!r}")
    finally:
        neutral_output(event_name)
        checkpoint(
            "hook_returning",
            event=event_name,
            session=session_id,
            cli_resume_ms=round((time.time_ns() - hook_received_ns) / 1_000_000, 3),
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
