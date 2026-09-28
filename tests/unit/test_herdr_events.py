import json
import tempfile
import unittest
from pathlib import Path

from core import herdr_events
from core.registry import Verdict


class AgentDetectedTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.state = Path(self.dir.name) / "detected.json"
        self.log = Path(self.dir.name) / "herdr-plugin.lifecycle.jsonl"
        self.notified, self.checked = [], []

    def tearDown(self):
        self.dir.cleanup()

    def handle(self, payload, verdict=None):
        verdict = verdict or Verdict("claude", "ready", "agent-harness: claude ready (default)")

        def preflight(agent):
            self.checked.append(agent)
            return verdict

        return herdr_events.on_agent_detected(json.dumps(payload), preflight=preflight,
                                              notify=lambda title, body, sound: self.notified.append((title, body, sound)),
                                              state_path=self.state, log_path=self.log)

    def test_ready_is_announced_once_per_pane(self):
        event = {"type": "pane_agent_detected", "agent": "claude", "pane_id": "p1", "workspace_id": "w"}
        self.handle(event)
        self.handle(event)
        self.assertEqual(self.checked, ["claude", "claude"])
        self.assertEqual(self.notified, [("agent-harness", "agent-harness: claude ready (default)", "none")])

    def test_failures_are_always_shown_with_a_request_sound_but_not_repeated_per_pane(self):
        broken = Verdict("codex", "broken", "agent-harness: codex is set up but not working: x")
        event = {"type": "pane_agent_detected", "agent": "codex", "pane_id": "p2", "workspace_id": "w"}
        self.handle(event, broken)
        self.handle(event, broken)
        self.assertEqual(self.notified, [("agent-harness", broken.message, "request")])

    def test_unsupported_agent_is_reported(self):
        verdict = Verdict("gemini", "unsupported", "agent-harness: gemini is not supported (no adapter); running without the harness")
        self.handle({"type": "pane_agent_detected", "agent": "gemini", "pane_id": "p3", "workspace_id": "w"}, verdict)
        self.assertEqual(self.notified[0][1], verdict.message)

    def test_released_or_missing_agent_is_ignored(self):
        self.handle({"type": "pane_agent_detected", "agent": None, "pane_id": "p", "workspace_id": "w"})
        self.handle({"type": "pane_agent_detected", "agent": "claude", "released": True, "pane_id": "p", "workspace_id": "w"})
        self.assertEqual((self.checked, self.notified), ([], []))

    def test_wrapped_payload_and_garbage_are_handled(self):
        self.handle({"event": "pane.agent_detected", "data": {"agent": "claude", "pane_id": "p9", "workspace_id": "w"}})
        self.assertEqual(self.checked, ["claude"])
        self.assertEqual(herdr_events.on_agent_detected("not json", preflight=lambda a: None, notify=lambda *a: None,
                                                        state_path=self.state, log_path=self.log), "ignored")

    def test_every_detection_is_logged(self):
        self.handle({"type": "pane_agent_detected", "agent": "claude", "pane_id": "p1", "workspace_id": "w"})
        ops = [json.loads(line)["operation"] for line in self.log.read_text(encoding="utf-8").splitlines()]
        self.assertIn("agent_detected.preflight", ops)


if __name__ == "__main__":
    unittest.main()
