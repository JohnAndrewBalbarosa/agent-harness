import json
import tempfile
import unittest
from pathlib import Path

from adapters.base import TurnUsage
from adapters.claude import transcript as claude_tx
from adapters.codex import transcript as codex_tx


def write_jsonl(entries):
    tmp = tempfile.NamedTemporaryFile("w", suffix=".jsonl", delete=False, encoding="utf-8")
    tmp.write("\n".join(json.dumps(e) for e in entries) + "\nnot json\n")
    tmp.close()
    return Path(tmp.name)


def assistant(msg_id, ts, inp=0, out=0, create=0, read=0, thinking=None):
    usage = {"input_tokens": inp, "output_tokens": out, "cache_creation_input_tokens": create, "cache_read_input_tokens": read}
    if thinking is not None:
        usage["output_tokens_details"] = {"thinking_tokens": thinking}
    return {"type": "assistant", "timestamp": ts, "message": {"id": msg_id, "usage": usage}}


def user(content, ts, uuid="u", meta=False):
    entry = {"type": "user", "timestamp": ts, "uuid": uuid, "message": {"content": content}}
    if meta:
        entry["isMeta"] = True
    return entry


def record(turn, response, inp, cached, out, reasoning, ts="2026-09-27T20:00:00Z"):
    usage = {"input_tokens": inp, "cached_input_tokens": cached, "cache_write_input_tokens": 0, "output_tokens": out,
             "reasoning_output_tokens": reasoning, "total_tokens": inp + out}
    return {"timestamp": ts, "type": "token_usage_record", "payload": {"turn_id": turn, "response_id": response, "usage": usage}}


class ClaudeTranscriptTests(unittest.TestCase):
    def tearDown(self):
        if hasattr(self, "path"):
            self.path.unlink(missing_ok=True)

    def test_counts_only_after_last_real_prompt_and_dedupes(self):
        self.path = write_jsonl([
            user("first", "2026-09-28T00:00:00Z", uuid="p1"), assistant("old", "2026-09-28T00:00:01Z", inp=999, out=999),
            user("second", "2026-09-28T00:01:00Z", uuid="p2"),
            assistant("a1", "2026-09-28T00:01:01Z", inp=10, out=1, read=100, create=5, thinking=3),
            assistant("a1", "2026-09-28T00:01:01Z", inp=10, out=1, read=100, create=5, thinking=3),
            user([{"type": "tool_result", "tool_use_id": "t", "content": "ok"}], "2026-09-28T00:01:02Z"),
            user("reminder", "2026-09-28T00:01:02Z", meta=True),
            assistant("a2", "2026-09-28T00:01:03Z", inp=20, out=2, read=50),
        ])
        self.assertEqual(claude_tx.usage(self.path, None),
                         TurnUsage(input_tokens=30 + 150 + 5, output_tokens=3, cached_input_tokens=150, reasoning_tokens=3,
                                   api_calls=2, turn_id="p2"))

    def test_task_notification_starts_a_turn(self):
        self.path = write_jsonl([user("go", "2026-09-28T00:00:00Z", uuid="p1"), assistant("a", "2026-09-28T00:00:01Z", inp=5),
                                 user("<task-notification>\n<task-id>x</task-id>", "2026-09-28T00:05:00Z", uuid="n1"),
                                 assistant("b", "2026-09-28T00:05:01Z", inp=7)])
        self.assertEqual((claude_tx.usage(self.path, None).turn_id, claude_tx.usage(self.path, None).input_tokens), ("n1", 7))

    def test_missing_transcript_is_empty_usage(self):
        usage = claude_tx.usage(Path("C:/nope/missing.jsonl"), None)
        self.assertEqual((usage.api_calls, usage.input_tokens), (0, 0))


class CodexTranscriptTests(unittest.TestCase):
    def tearDown(self):
        if hasattr(self, "path"):
            self.path.unlink(missing_ok=True)

    def test_sums_turn_records_and_dedupes_responses(self):
        self.path = write_jsonl([
            record("t-old", "r0", 999, 0, 999, 0),
            record("t1", "r1", 100, 60, 10, 4), record("t1", "r1", 100, 60, 10, 4), record("t1", "r2", 200, 120, 20, 6),
            {"type": "event_msg", "payload": {"type": "token_count", "info": {"last_token_usage": {"input_tokens": 5}}}},
        ])
        self.assertEqual(codex_tx.usage(self.path, "t1"),
                         TurnUsage(input_tokens=300, output_tokens=30, cached_input_tokens=180, reasoning_tokens=10, api_calls=2, turn_id="t1"))

    def test_without_turn_id_uses_latest_turn(self):
        self.path = write_jsonl([record("t1", "r1", 1, 0, 1, 0), record("t2", "r2", 7, 0, 3, 1)])
        usage = codex_tx.usage(self.path, None)
        self.assertEqual((usage.turn_id, usage.input_tokens), ("t2", 7))

    def test_unknown_turn_is_empty(self):
        self.path = write_jsonl([record("t1", "r1", 1, 0, 1, 0)])
        self.assertEqual(codex_tx.usage(self.path, "zzz").api_calls, 0)


if __name__ == "__main__":
    unittest.main()
