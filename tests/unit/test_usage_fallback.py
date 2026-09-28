import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from adapters.base import TurnUsage
from core.events import make_event, to_json
from core.usage import fallback

ROOT = Path(__file__).resolve().parents[2]


class BuildRecordTests(unittest.TestCase):
    def test_record_schema(self):
        event = make_event("turn.stop", "codex", "cy", {"session_id": "s1", "turn_id": "t1"}, "Stop")
        usage = TurnUsage(input_tokens=300, output_tokens=30, cached_input_tokens=180, reasoning_tokens=10, api_calls=2, turn_id="t1")
        record = fallback.build_record(event, usage, "project-1")
        self.assertEqual((record["source"], record["availability"], record["projectId"]), ("codex_hook", "exact", "project-1"))
        self.assertEqual((record["inputTokens"], record["outputTokens"], record["cachedInputTokens"], record["reasoningTokens"]),
                         (300, 30, 180, 10))
        self.assertEqual((record["nativeSession"], record["nativeTurn"], record["sourceEventId"]), ("s1", "t1", "s1:t1:Stop"))

    def test_no_calls_is_unavailable(self):
        event = make_event("turn.stop", "claude", "default", {"session_id": "s"}, "Stop")
        record = fallback.build_record(event, TurnUsage(0, 0, 0, None, 0, None), "p")
        self.assertEqual(record["availability"], "unavailable")

    def test_same_turn_gets_same_usage_id(self):
        event = make_event("turn.stop", "codex", "personal", {"session_id": "s", "turn_id": "t"}, "Stop")
        usage = TurnUsage(1, 1, 0, 0, 1, "t")
        self.assertEqual(fallback.build_record(event, usage, "p")["usageId"], fallback.build_record(event, usage, "p")["usageId"])


class ConsumerTests(unittest.TestCase):
    def test_consumer_writes_local_log_in_dry_run(self):
        with tempfile.TemporaryDirectory() as tmp:
            rollout = Path(tmp) / "rollout.jsonl"
            rollout.write_text(json.dumps({"type": "token_usage_record", "payload": {
                "turn_id": "t1", "response_id": "r1", "usage": {"input_tokens": 50, "cached_input_tokens": 20, "output_tokens": 5,
                                                              "reasoning_output_tokens": 1}}}) + "\n", encoding="utf-8")
            event = make_event("turn.stop", "codex", "personal",
                               {"session_id": "s1", "turn_id": "t1", "transcript_path": str(rollout)}, "Stop")
            env = {**os.environ, "PYTHONPATH": str(ROOT), "AH_USAGE_DIR": tmp, "AH_USAGE_DRY_RUN": "1"}
            done = subprocess.run([sys.executable, "-m", "core.usage.fallback"], input=to_json(event).encode(), cwd=str(ROOT), env=env, timeout=60)
            self.assertEqual(done.returncode, 0)
            logged = json.loads((Path(tmp) / "turn-usage.jsonl").read_text(encoding="utf-8").splitlines()[-1])
            self.assertEqual((logged["source"], logged["inputTokens"], logged["enqueued"]), ("codex_hook", 50, False))

    def test_consumer_ignores_non_stop_and_garbage(self):
        env = {**os.environ, "PYTHONPATH": str(ROOT), "AH_USAGE_DRY_RUN": "1"}
        for body in (b"garbage", to_json(make_event("prompt.submit", "codex", "personal", {"session_id": "s"}, "UserPromptSubmit")).encode()):
            done = subprocess.run([sys.executable, "-m", "core.usage.fallback"], input=body, cwd=str(ROOT), env=env, timeout=60)
            self.assertEqual(done.returncode, 0)


if __name__ == "__main__":
    unittest.main()
