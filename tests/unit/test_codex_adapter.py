import json
import unittest
from pathlib import Path

from adapters.base import ConsumerResult, load_adapter

FIXTURES = Path(__file__).resolve().parents[2] / "adapters" / "codex" / "fixtures"


def fixture(name: str) -> dict:
    return json.loads((FIXTURES / f"{name}.json").read_text(encoding="utf-8"))


class NormalizeTests(unittest.TestCase):
    def setUp(self):
        self.adapter = load_adapter("codex")

    def normalize(self, name):
        return self.adapter.normalize(name, fixture(name))

    def test_every_fixture_maps_to_its_common_event(self):
        expected = {
            "SessionStart": "session.start", "UserPromptSubmit": "prompt.submit", "PostToolUse": "tool.completed",
            "Stop": "turn.stop", "SubagentStart": "subagent.start", "SubagentStop": "subagent.stop",
            "SessionEnd": "session.end", "PermissionRequest": "attention.needed",
        }
        for native, common in expected.items():
            with self.subTest(native=native):
                event = self.normalize(native)
                self.assertEqual((event.event, event.agent, event.native_event), (common, "codex", native))
                self.assertEqual(event.session_id, "sess-codex-1")

    def test_prompt_and_turn_id(self):
        event = self.normalize("UserPromptSubmit")
        self.assertEqual((event.data["prompt"], event.turn_id), ("ayusin ang bug ñ", "turn-1"))

    def test_tool_details(self):
        tool = self.normalize("PostToolUse").data["tool"]
        self.assertEqual((tool["name"], tool["input"], tool["response"]), ("shell", {"command": ["echo", "hi"]}, {"output": "hi"}))

    def test_permission_request_is_permission_attention(self):
        attention = self.normalize("PermissionRequest").data["attention"]
        self.assertEqual((attention["kind"], attention["tool"]), ("permission", "shell"))

    def test_subagent_identity(self):
        self.assertEqual(dict(self.normalize("SubagentStop").data["subagent"]), {"id": "agent-7", "type": "worker"})

    def test_stop_carries_last_message(self):
        data = self.normalize("Stop").data
        self.assertEqual((data["last_message"], data["stop_hook_active"]), ("Tapos na.", False))

    def test_native_event_falls_back_to_payload(self):
        self.assertEqual(self.adapter.normalize(None, fixture("Stop")).event, "turn.stop")

    def test_unknown_event_is_ignored(self):
        self.assertIsNone(self.adapter.normalize("FutureEvent", {"hook_event_name": "FutureEvent", "session_id": "s"}))


class InstanceTests(unittest.TestCase):
    def test_instance_from_codex_home(self):
        adapter = load_adapter("codex")
        cases = {r"C:\Users\x\.codex": "personal", r"C:\Users\x\.codex-cy": "cy", r"C:\Users\x\.codex-feu": "feu",
                 r"C:\Users\x\.codex-work": "work"}
        for home, instance in cases.items():
            with self.subTest(home=home):
                self.assertEqual(adapter.instance({"CODEX_HOME": home}), instance)
        self.assertEqual(adapter.instance({}), "personal")

    def test_explicit_instance_override(self):
        self.assertEqual(load_adapter("codex").instance({"AH_INSTANCE": "lab", "CODEX_HOME": r"C:\x\.codex"}), "lab")


class RespondTests(unittest.TestCase):
    def setUp(self):
        self.adapter = load_adapter("codex")

    def test_stop_without_output_continues(self):
        event = self.adapter.normalize("Stop", fixture("Stop"))
        self.assertEqual(json.loads(self.adapter.respond(event, [])), {"continue": True})

    def test_context_output_is_wrapped_for_session_start(self):
        event = self.adapter.normalize("SessionStart", fixture("SessionStart"))
        out = json.loads(self.adapter.respond(event, [ConsumerResult("context", 0, "Vision: X".encode())]))
        self.assertEqual(out["hookSpecificOutput"], {"hookEventName": "SessionStart", "additionalContext": "Vision: X"})

    def test_bridge_envelope_passes_through(self):
        event = self.adapter.normalize("SessionStart", fixture("SessionStart"))
        envelope = b'{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"from bridge"}}'
        self.assertEqual(self.adapter.respond(event, [ConsumerResult("herdr-bridge", 0, envelope)]), envelope.decode())

    def test_failed_consumer_output_is_ignored(self):
        event = self.adapter.normalize("SessionStart", fixture("SessionStart"))
        self.assertEqual(self.adapter.respond(event, [ConsumerResult("context", 1, b"partial")]), "")

    def test_safe_defaults(self):
        self.assertEqual(json.loads(self.adapter.safe_default("Stop")), {"continue": True})
        self.assertEqual(self.adapter.safe_default("PostToolUse"), "")
        self.assertEqual(self.adapter.safe_default("FutureEvent"), "")


class LoaderTests(unittest.TestCase):
    def test_unknown_adapter_raises_key_error(self):
        with self.assertRaises(KeyError):
            load_adapter("does-not-exist")

    def test_adapter_names_cannot_escape_package(self):
        with self.assertRaises(KeyError):
            load_adapter("..core")


if __name__ == "__main__":
    unittest.main()
