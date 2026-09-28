"""Adapter conformance kit: every adapter MUST pass this suite (spec §12). New agents add their name to ADAPTERS."""
import unittest
from pathlib import Path

from adapters.base import load_adapter
from core.events import from_json, make_event, to_json
from tests.conformance.kit import conformance_violations

ADAPTERS = ("codex", "claude")
ROOT = Path(__file__).resolve().parents[2]


class ConformanceTests(unittest.TestCase):
    def test_every_adapter_conforms(self):
        for name in ADAPTERS:
            with self.subTest(adapter=name):
                self.assertEqual(conformance_violations(name, load_adapter(name), ROOT / "adapters" / name), [])


class BrokenAdapter:
    """Violates the contract on purpose: wrong agent name, raising respond, non-str safe_default."""
    name = "codex"

    def instance(self, env):
        return "x"

    def normalize(self, native_event, payload, env=None):
        return make_event("turn.stop", "someone-else", "x", payload, native_event or "")

    def respond(self, event, results):
        raise RuntimeError("boom")

    def safe_default(self, native_event):
        return None


class KitSelfTests(unittest.TestCase):
    def test_kit_reports_a_broken_adapter(self):
        violations = conformance_violations("codex", BrokenAdapter(), ROOT / "adapters" / "codex")
        joined = "\n".join(violations)
        self.assertIn("agent", joined)
        self.assertIn("respond raised", joined)
        self.assertIn("safe_default", joined)

    def test_kit_reports_missing_manifest_keys(self):
        violations = conformance_violations("codex", load_adapter("codex"), ROOT / "adapters" / "does-not-exist")
        self.assertTrue(any("manifest" in v for v in violations))

    def test_events_round_trip_helper(self):
        event = make_event("turn.stop", "codex", "personal", {"session_id": "s"}, "Stop")
        self.assertEqual(from_json(to_json(event)), event)


if __name__ == "__main__":
    unittest.main()
