import json
import sqlite3
import tempfile
import unittest
from pathlib import Path

from core.usage import report


def jsonl(path: Path, rows) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("".join(json.dumps(r) + "\n" for r in rows), encoding="utf-8")
    return path


class ClaudeReportTests(unittest.TestCase):
    def test_dedupes_by_message_id_and_splits_cache_and_tools(self):
        with tempfile.TemporaryDirectory() as tmp:
            projects = Path(tmp) / "projects"
            usage = {"input_tokens": 1, "cache_read_input_tokens": 900, "cache_creation_input_tokens": 99, "output_tokens": 10}
            jsonl(projects / "C--Users-x-Desktop-Projects-app" / "s.jsonl", [
                {"type": "assistant", "timestamp": "2026-09-27T01:00:00Z", "message": {"id": "m1", "model": "claude-x", "usage": usage,
                 "content": [{"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": "a.py"}}]}},
                {"type": "assistant", "timestamp": "2026-09-27T01:00:00Z", "message": {"id": "m1", "model": "claude-x", "usage": usage}},
                {"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t1", "content": "x" * 50}]}},
            ])
            result = report.claude(projects)
        self.assertEqual(result["requests"], 1)
        self.assertEqual(result["totals"], {"input": 1, "cache_read": 900, "cache_create": 99, "output": 10})
        self.assertEqual(result["by_project"], {"app": 1010})
        self.assertEqual(result["tool_result_bytes"]["Read"]["calls"], 1)


class CodexReportTests(unittest.TestCase):
    def test_final_cumulative_totals_per_thread(self):
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp) / ".codex"
            rollout = jsonl(home / "sessions" / "r.jsonl", [
                {"type": "event_msg", "payload": {"type": "token_count", "info": {
                    "total_token_usage": {"input_tokens": 100, "cached_input_tokens": 80, "output_tokens": 5, "reasoning_output_tokens": 2, "total_tokens": 105},
                    "last_token_usage": {"input_tokens": 100}}}},
                {"type": "event_msg", "payload": {"type": "token_count", "info": {
                    "total_token_usage": {"input_tokens": 300, "cached_input_tokens": 250, "output_tokens": 9, "reasoning_output_tokens": 3, "total_tokens": 309},
                    "last_token_usage": {"input_tokens": 200}}}},
            ])
            db = sqlite3.connect(home / "state_5.sqlite")
            db.execute("create table threads (rollout_path text, cwd text, tokens_used integer, created_at integer)")
            db.execute("insert into threads values (?, ?, ?, ?)", (str(rollout), "C:\\x\\rentcircle", 309, 1790000000))
            db.commit()
            db.close()
            result = report.codex({"personal": home})
        self.assertEqual(result["totals"], {"uncached": 50, "cached": 250, "output": 9, "reasoning": 3, "requests": 2})
        self.assertEqual(result["by_project"], {"rentcircle": 300})
        self.assertEqual(result["max_context"], 200)


class RenderTests(unittest.TestCase):
    def test_text_report_is_bounded_and_labelled(self):
        text = report.render({"claude": {"requests": 1, "totals": {"input": 1, "cache_read": 9, "cache_create": 0, "output": 1},
                                         "by_project": {f"p{i}": i for i in range(30)}, "tool_result_bytes": {}},
                              "codex": None})
        self.assertLessEqual(len(text.splitlines()), 40)
        self.assertIn("cache_read", text)


if __name__ == "__main__":
    unittest.main()
