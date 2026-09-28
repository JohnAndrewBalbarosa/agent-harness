import json
import unittest
from pathlib import Path

from adapters.base import ConsumerResult, load_adapter

FIXTURES = Path(__file__).resolve().parents[2] / "adapters" / "claude" / "fixtures"


def fixture(name: str) -> dict:
    return json.loads((FIXTURES / f"{name}.json").read_text(encoding="utf-8"))


class NormalizeTests(unittest.TestCase):
    def setUp(self):
        self.adapter = load_adapter("claude")

    def normalize(self, name):
        payload = fixture(name)
        return self.adapter.normalize(payload["hook_event_name"], payload)

    def test_every_fixture_maps_to_its_common_event(self):
        expected = {
            "SessionStart": "session.start", "UserPromptSubmit": "prompt.submit", "PostToolUse": "tool.completed",
            "Stop": "turn.stop", "SubagentStart": "subagent.start", "SubagentStop": "subagent.stop",
            "SessionEnd": "session.end", "Notification": "attention.needed", "Notification.idle": "attention.needed",
        }
        for name, common in expected.items():
            with self.subTest(name=name):
                event = self.normalize(name)
                self.assertEqual((event.event, event.agent, event.instance), (common, "claude", "default"))

    def test_notification_kinds(self):
        self.assertEqual(self.normalize("Notification").data["attention"]["kind"], "permission")
        idle = self.normalize("Notification.idle").data["attention"]
        self.assertEqual((idle["kind"], idle["message"]), ("idle", "Claude is waiting for your input"))

    def test_other_notification_types_are_ignored(self):
        self.assertIsNone(self.normalize("Notification.other"))

    def test_prompt_keeps_unicode(self):
        self.assertEqual(self.normalize("UserPromptSubmit").data["prompt"], "gawin mo na ñ 🙂")

    def test_subagent_identity(self):
        self.assertEqual(dict(self.normalize("SubagentStart").data["subagent"]),
                         {"id": "ab2bf5fbb7190fd00", "type": "general-purpose"})

    def test_stop_carries_last_message(self):
        self.assertEqual(self.normalize("Stop").data["last_message"], "Tapos na.")

    def test_unknown_event_is_ignored(self):
        self.assertIsNone(self.adapter.normalize("PostToolBatch", {"hook_event_name": "PostToolBatch", "session_id": "s"}))

    def test_instance_override(self):
        self.assertEqual(self.adapter.instance({"AH_INSTANCE": "work"}), "work")
        self.assertEqual(self.adapter.instance({}), "default")


class RespondTests(unittest.TestCase):
    def setUp(self):
        self.adapter = load_adapter("claude")

    def test_session_start_context_envelope(self):
        event = self.adapter.normalize("SessionStart", fixture("SessionStart"))
        out = json.loads(self.adapter.respond(event, [ConsumerResult("context", 0, "Goal: ship".encode())]))
        self.assertEqual(out, {"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": "Goal: ship"}})

    def test_other_events_print_nothing(self):
        for name in ("Stop", "PostToolUse", "Notification"):
            with self.subTest(name=name):
                payload = fixture(name)
                event = self.adapter.normalize(payload["hook_event_name"], payload)
                self.assertEqual(self.adapter.respond(event, [ConsumerResult("context", 0, b"x")]), "")

    def test_safe_default_is_empty(self):
        for native in ("Stop", "SessionStart", "FutureEvent", None):
            with self.subTest(native=native):
                self.assertEqual(self.adapter.safe_default(native), "")


if __name__ == "__main__":
    unittest.main()
