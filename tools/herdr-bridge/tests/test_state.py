import importlib.util
import sys
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("herdr_state", ROOT / "state.py")
state = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = state
spec.loader.exec_module(state)


class StateTests(unittest.TestCase):
    def test_hook_mapping_preserves_custom_semantics(self):
        self.assertEqual(state.decision_for_hook("SessionStart", {}).canonical, "idle")
        self.assertEqual(state.decision_for_hook("UserPromptSubmit", {}).canonical, "working")
        self.assertEqual(state.decision_for_hook("PermissionRequest", {}).canonical, "needs_input")
        self.assertEqual(state.decision_for_hook("Stop", {}).canonical, "ready")

    def test_needs_input_ties_use_oldest_waiting(self):
        older = {"session_id": "older", "canonical_state": "needs_input", "state_entered_at": 10}
        newer = {"session_id": "newer", "canonical_state": "needs_input", "state_entered_at": 20}
        self.assertEqual(state.highest_priority([newer, older])["session_id"], "older")

    def test_other_ties_use_newest_transition(self):
        older = {"session_id": "older", "canonical_state": "ready", "state_entered_at": 10}
        newer = {"session_id": "newer", "canonical_state": "ready", "state_entered_at": 20}
        self.assertEqual(state.highest_priority([older, newer])["session_id"], "newer")


if __name__ == "__main__":
    unittest.main()
