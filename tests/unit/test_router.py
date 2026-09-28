import json
import sys
import tempfile
import time
import unittest
from pathlib import Path

from core.router import harness_hook as router
from core.router.consumers import Consumer

FAKES = Path(__file__).resolve().parent / "fakes"


def fake(name, script, stdin="event", timeout_s=10, events=None):
    return Consumer(name=name, events=frozenset(events or {"session.start", "prompt.submit", "turn.stop", "tool.completed"}),
                    command=(sys.executable, str(FAKES / script)), stdin=stdin, timeout_s=timeout_s)


class RouterTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.tmp = Path(self.dir.name)
        self.env = {"AH_LOG_DIR": str(self.tmp)}

    def tearDown(self):
        self.dir.cleanup()

    def run_router(self, agent, native, payload, consumers, env=None):
        stdin = payload if isinstance(payload, bytes) else json.dumps(payload, ensure_ascii=False).encode("utf-8")
        return router.run(agent, native, stdin, {**self.env, **(env or {})}, registry=lambda event, env: consumers)

    def log_records(self):
        path = self.tmp / "hook-router.lifecycle.jsonl"
        return [json.loads(l) for l in path.read_text(encoding="utf-8").splitlines()] if path.exists() else []

    def test_malformed_stdin_returns_safe_default(self):
        out, code = self.run_router("codex", "Stop", b"{not json", [])
        self.assertEqual((json.loads(out), code), ({"continue": True}, 0))
        self.assertTrue(any(r["operation"] == "hook.payload.invalid" for r in self.log_records()))

    def test_empty_stdin_returns_safe_default(self):
        self.assertEqual(json.loads(self.run_router("codex", "Stop", b"", [])[0]), {"continue": True})

    def test_utf8_payload_round_trip(self):
        out_file = self.tmp / "seen.json"
        payload = {"hook_event_name": "UserPromptSubmit", "session_id": "s", "prompt": "ñá 🙂"}
        self.run_router("claude", None, payload, [fake("echo", "echo_event.py")], {"FAKE_OUT": str(out_file)})
        received = json.loads(out_file.read_text(encoding="utf-8"))
        self.assertEqual((received["event"], received["data"]["prompt"]), ("prompt.submit", "ñá 🙂"))

    def test_native_stdin_consumer_gets_raw_payload(self):
        out_file = self.tmp / "raw.json"
        payload = {"hook_event_name": "Stop", "session_id": "s", "extra": "kept"}
        self.run_router("codex", "Stop", payload, [fake("legacy", "echo_event.py", stdin="native")], {"FAKE_OUT": str(out_file)})
        self.assertEqual(json.loads(out_file.read_text(encoding="utf-8"))["extra"], "kept")

    def test_consumer_timeout_does_not_block(self):
        started = time.perf_counter()
        out, code = self.run_router("codex", "Stop", {"hook_event_name": "Stop", "session_id": "s"}, [fake("slow", "sleeper.py", timeout_s=1)])
        self.assertLess(time.perf_counter() - started, 8)
        self.assertEqual((json.loads(out), code), ({"continue": True}, 0))
        self.assertTrue(any(r["operation"] == "consumer.timeout" for r in self.log_records()))

    def test_consumer_crash_is_isolated(self):
        out_file = self.tmp / "ok.json"
        consumers = [fake("crash", "crasher.py"), fake("echo", "echo_event.py")]
        out, code = self.run_router("codex", "Stop", {"hook_event_name": "Stop", "session_id": "s"}, consumers, {"FAKE_OUT": str(out_file)})
        self.assertTrue(out_file.exists())
        self.assertEqual(code, 0)
        dispatch = [r for r in self.log_records() if r["operation"] == "hook.dispatch"][-1]
        self.assertEqual(dispatch["details"]["failures"], {"crash": 3})

    def test_context_output_reaches_agent(self):
        payload = {"hook_event_name": "SessionStart", "session_id": "s", "source": "startup"}
        out, _ = self.run_router("claude", "SessionStart", payload, [fake("context", "printer.py")], {"FAKE_PRINT": "Goal: X"})
        self.assertEqual(json.loads(out)["hookSpecificOutput"]["additionalContext"], "Goal: X")

    def test_unknown_native_event_gets_safe_default_without_consumers(self):
        out_file = self.tmp / "never.json"
        out, _ = self.run_router("codex", "FutureEvent", {"session_id": "s"}, [fake("echo", "echo_event.py")], {"FAKE_OUT": str(out_file)})
        self.assertEqual(out, "")
        self.assertFalse(out_file.exists())

    def test_only_subscribed_consumers_run(self):
        out_file = self.tmp / "sub.json"
        consumer = fake("echo", "echo_event.py", events={"subagent.start"})
        self.run_router("codex", "Stop", {"hook_event_name": "Stop", "session_id": "s"}, [consumer], {"FAKE_OUT": str(out_file)})
        self.assertFalse(out_file.exists())

    def test_untested_version_is_logged_not_blocked(self):
        payload = {"hook_event_name": "SessionStart", "session_id": "s", "source": "startup", "version": "9.9.9"}
        out, code = self.run_router("claude", "SessionStart", payload, [])
        self.assertEqual(code, 0)
        record = [r for r in self.log_records() if r["operation"] == "agent.version.untested"][0]
        self.assertEqual((record["details"]["agent"], record["details"]["version"]), ("claude", "9.9.9"))

    def test_dispatch_is_logged(self):
        self.run_router("claude", "Stop", {"hook_event_name": "Stop", "session_id": "s"}, [])
        dispatch = [r for r in self.log_records() if r["operation"] == "hook.dispatch"][0]
        self.assertEqual((dispatch["details"]["agent"], dispatch["details"]["event"]), ("claude", "turn.stop"))


class MainTests(unittest.TestCase):
    def test_unknown_agent_exits_zero(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(router.run("nope", "Stop", b"{}", {"AH_LOG_DIR": tmp})[1], 0)

    def test_main_without_agent_exits_zero(self):
        self.assertEqual(router.main([]), 0)


    def test_main_merges_the_harness_env_file(self):
        from unittest import mock
        seen = {}
        with tempfile.TemporaryDirectory() as tmp:
            env_file = Path(tmp) / ".env"
            env_file.write_text("NOTIFY_RATE_LIMIT=1/9\n", encoding="utf-8")
            with mock.patch.object(router, "run", lambda a, n, s, env: (seen.update(env), ("", 0))[1]), \
                 mock.patch.object(router, "ENV_FILE", env_file), mock.patch.object(router.sys, "stdin", None):
                router.main(["claude", "Stop"])
        self.assertEqual(seen.get("NOTIFY_RATE_LIMIT"), "1/9")


if __name__ == "__main__":
    unittest.main()
