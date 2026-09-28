import json
import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path

from adapters.claude import background as bg

NOW = datetime(2026, 9, 28, 12, 0, 0, tzinfo=timezone.utc)


def tool_result(text, ts="2026-09-28T11:50:00Z"):
    return {"type": "user", "timestamp": ts, "message": {"content": [{"type": "tool_result", "tool_use_id": "t", "content": text}]}}


def notification(task_id, status="completed", entry_type="user", ts="2026-09-28T11:55:00Z"):
    text = f"<task-notification>\n<task-id>{task_id}</task-id>\n<status>{status}</status>\n</task-notification>"
    if entry_type == "user":
        return {"type": "user", "timestamp": ts, "message": {"content": text}}
    return {"type": entry_type, "timestamp": ts, "content": text}


def bash_background(shell_id, ts="2026-09-28T11:50:00Z"):
    return tool_result(f"Command running in background with ID: {shell_id}. Output is being written to ...", ts)


def bash_timeout(shell_id, ts="2026-09-28T11:50:00Z"):
    return tool_result(f"Command did not complete within its 120s timeout and was moved to the background (ID: {shell_id}). Output ...", ts)


class ScanTests(unittest.TestCase):
    def test_background_shells_are_tracked(self):
        self.assertEqual(bg.scan([bash_background("b1x"), bash_timeout("b2y")], NOW), ["b1x", "b2y"])

    def test_notifications_complete_shells_in_any_entry_type(self):
        entries = [bash_background("b1"), bash_timeout("b2"),
                   notification("b1", "stopped", entry_type="queue-operation"), notification("b2", "failed", entry_type="attachment")]
        self.assertEqual(bg.scan(entries, NOW), [])

    def test_agent_launches_are_left_to_subagent_hooks(self):
        agent = tool_result("Async agent launched successfully.\nagentId: abc123 (internal ID)")
        self.assertEqual(bg.scan([agent], NOW), [])

    def test_echoed_markers_are_ignored(self):
        echoed = tool_result("120 LAUNCH 'moved to the background (ID: b85)'\nCommand running in background with ID: zz")
        self.assertEqual(bg.scan([echoed], NOW), [])

    def test_echoed_notification_does_not_complete(self):
        echoed = tool_result("grep hit: <task-notification>\n<task-id>b1</task-id>")
        self.assertEqual(bg.scan([bash_background("b1"), echoed], NOW), ["b1"])

    def test_stale_shells_are_ignored(self):
        self.assertEqual(bg.scan([bash_background("old", ts="2026-09-28T08:00:00Z"), bash_background("new")], NOW), ["new"])


class FileTests(unittest.TestCase):
    def test_background_shells_reads_transcript_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "t.jsonl"
            path.write_text("\n".join([json.dumps(bash_background("s9")), "not json", ""]), encoding="utf-8")
            self.assertEqual(bg.background_shells(path, NOW), ["s9"])

    def test_missing_transcript_is_empty(self):
        self.assertEqual(bg.background_shells(Path("C:/does/not/exist.jsonl"), NOW), [])


if __name__ == "__main__":
    unittest.main()
