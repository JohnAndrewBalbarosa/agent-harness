import json
import os
import subprocess
import sys
import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path

from core.events import make_event, to_json
from core.notify import consumer
from core.tasks import state as tasks

ROOT = Path(__file__).resolve().parents[2]


def ev(event_type, agent="claude", instance="default", session="s1", **data):
    native = {"prompt.submit": "UserPromptSubmit", "turn.stop": "Stop", "attention.needed": "Notification"}[event_type]
    return make_event(event_type, agent, instance, {"session_id": session, "cwd": "C:/work/demo"}, native, data)


class ConsumerTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.tmp = Path(self.dir.name)
        self.env = {"AH_NOTIFY_DIR": str(self.tmp / "notify"), "AH_TASKS_DIR": str(self.tmp / "tasks"), "AH_NOTIFY_DRY_RUN": "1"}

    def tearDown(self):
        self.dir.cleanup()

    def handle(self, event):
        return consumer.handle(event, self.env)

    def records(self):
        path = self.tmp / "notify" / "notify-events.jsonl"
        return [json.loads(l) for l in path.read_text(encoding="utf-8").splitlines()]

    def test_prompt_then_stop_toasts_done(self):
        self.handle(ev("prompt.submit", prompt="x"))
        self.handle(ev("turn.stop"))
        mark, toast = self.records()
        self.assertEqual((mark["decision"], toast["decision"], toast["sound"]), ("mark", "toast", "done"))
        self.assertEqual(toast["toastResult"], "dry-run")
        self.assertEqual(toast["title"], "Claude Code")
        self.assertIn("[demo]", toast["message"])

    def test_second_stop_is_skipped(self):
        for event in (ev("prompt.submit"), ev("turn.stop"), ev("turn.stop")):
            self.handle(event)
        self.assertEqual(self.records()[-1]["reason"], "no-pending-prompt")

    def test_stop_defers_while_subagent_runs(self):
        start = make_event("subagent.start", "claude", "default", {"session_id": "s1"}, "SubagentStart",
                           {"subagent": {"id": "ag1", "type": "x"}})
        tasks.record(self.tmp / "tasks", start, datetime.now(timezone.utc))
        self.handle(ev("prompt.submit"))
        self.handle(ev("turn.stop"))
        last = self.records()[-1]
        self.assertEqual((last["decision"], last["outstanding"]), ("defer", ["ag1"]))

    def test_sessions_are_isolated(self):
        self.handle(ev("prompt.submit", session="a"))
        self.handle(ev("turn.stop", session="b"))
        self.assertEqual(self.records()[-1]["reason"], "no-pending-prompt")

    def test_codex_instance_title(self):
        self.handle(ev("prompt.submit", agent="codex", instance="cy"))
        self.handle(ev("turn.stop", agent="codex", instance="cy"))
        self.assertEqual(self.records()[-1]["title"], "Codex (cy)")

    def test_log_is_bounded(self):
        log = self.tmp / "notify" / "notify-events.jsonl"
        log.parent.mkdir(parents=True)
        log.write_text("\n".join(json.dumps({"n": i}) for i in range(consumer.LOG_MAX_RECORDS)) + "\n", encoding="utf-8")
        self.handle(ev("prompt.submit"))
        lines = log.read_text(encoding="utf-8").splitlines()
        self.assertEqual(len(lines), consumer.LOG_MAX_RECORDS)
        self.assertEqual(json.loads(lines[-1])["decision"], "mark")

    def test_main_reads_stdin_and_ignores_garbage(self):
        env = {**os.environ, **self.env, "PYTHONPATH": str(ROOT)}
        ok = subprocess.run([sys.executable, "-m", "core.notify.consumer"], input=to_json(ev("prompt.submit")).encode(),
                            cwd=str(ROOT), env=env, timeout=60)
        bad = subprocess.run([sys.executable, "-m", "core.notify.consumer"], input=b"nope", cwd=str(ROOT), env=env, timeout=60)
        self.assertEqual((ok.returncode, bad.returncode), (0, 0))
        self.assertEqual(self.records()[-1]["decision"], "mark")


if __name__ == "__main__":
    unittest.main()
