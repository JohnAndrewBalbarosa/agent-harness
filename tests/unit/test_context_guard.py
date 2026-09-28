import json
import tempfile
import unittest
from pathlib import Path

from adapters.claude import transcript as claude_transcript
from adapters.codex import transcript as codex_transcript
from core.events import make_event
from core.usage import context_guard


def write_jsonl(path: Path, rows) -> Path:
    path.write_text("".join(json.dumps(r) + "\n" for r in rows), encoding="utf-8")
    return path


class ContextTokensTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.tmp = Path(self.dir.name)

    def tearDown(self):
        self.dir.cleanup()

    def test_claude_last_request_context(self):
        path = write_jsonl(self.tmp / "c.jsonl", [
            {"type": "assistant", "message": {"id": "m1", "usage": {"input_tokens": 5, "cache_read_input_tokens": 1000, "cache_creation_input_tokens": 10, "output_tokens": 3}}},
            {"type": "user", "message": {"content": "x"}},
            {"type": "assistant", "message": {"id": "m2", "usage": {"input_tokens": 7, "cache_read_input_tokens": 180000, "cache_creation_input_tokens": 500, "output_tokens": 9}}},
        ])
        self.assertEqual(claude_transcript.context_tokens(path), 180507)

    def test_codex_last_request_context(self):
        path = write_jsonl(self.tmp / "x.jsonl", [
            {"type": "event_msg", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 90000}}}},
            {"type": "event_msg", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 210000}}}},
            {"type": "event_msg", "payload": {"type": "token_count", "info": None}},
        ])
        self.assertEqual(codex_transcript.context_tokens(path), 210000)

    def test_missing_transcript_is_none(self):
        self.assertIsNone(claude_transcript.context_tokens(self.tmp / "nope.jsonl"))


class GuardTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.tmp = Path(self.dir.name)
        self.sent = []

    def tearDown(self):
        self.dir.cleanup()

    def stop(self, context):
        path = write_jsonl(self.tmp / "t.jsonl", [{"type": "assistant", "message": {"id": f"m{context}", "usage": {
            "input_tokens": 0, "cache_read_input_tokens": context, "cache_creation_input_tokens": 0, "output_tokens": 1}}}])
        event = make_event("turn.stop", "claude", "default", {"session_id": "s1", "transcript_path": str(path)}, "Stop")
        return context_guard.handle(event, {"CONTEXT_WARN_TOKENS": "150000", "AH_CONTEXT_GUARD_DIR": str(self.tmp / "state"),
                                            "AH_LOG_DIR": str(self.tmp / "logs")},
                                    notify=lambda message: self.sent.append(message))

    def test_below_threshold_is_quiet(self):
        self.assertEqual(self.stop(90000), "below")
        self.assertEqual(self.sent, [])

    def test_warns_once_per_50k_band(self):
        self.assertEqual(self.stop(160000), "warned")
        self.assertEqual(self.stop(170000), "already_warned")
        self.assertEqual(self.stop(205000), "warned")
        self.assertEqual(len(self.sent), 2)
        self.assertIn("205K", self.sent[-1])
        log = (self.tmp / "logs" / "context-guard.lifecycle.jsonl").read_text(encoding="utf-8")
        self.assertIn("context.warned", log)


if __name__ == "__main__":
    unittest.main()
