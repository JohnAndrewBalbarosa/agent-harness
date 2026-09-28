import json
import unittest

from core import events as ev


class MakeEventTests(unittest.TestCase):
    def test_fills_common_fields_from_payload(self):
        e = ev.make_event("turn.stop", "claude", "default",
                          {"session_id": "s1", "cwd": "C:/p", "transcript_path": "C:/t.jsonl"}, "Stop")
        self.assertEqual((e.session_id, e.cwd, e.transcript_path, e.native_event), ("s1", "C:/p", "C:/t.jsonl", "Stop"))
        self.assertTrue(e.ts.endswith("Z"))

    def test_turn_id_is_taken_when_present(self):
        e = ev.make_event("prompt.submit", "codex", "personal", {"session_id": "s", "turn_id": "t9"}, "UserPromptSubmit")
        self.assertEqual(e.turn_id, "t9")

    def test_missing_session_id_becomes_unknown(self):
        self.assertEqual(ev.make_event("session.end", "codex", "personal", {}, "SessionEnd").session_id, "unknown")

    def test_unknown_event_rejected(self):
        with self.assertRaises(ValueError):
            ev.make_event("bogus", "codex", "personal", {}, "X")

    def test_large_native_payload_is_dropped(self):
        e = ev.make_event("tool.completed", "codex", "personal", {"session_id": "s", "blob": "x" * 20000}, "PostToolUse")
        self.assertEqual(dict(e.native_payload), {})
        self.assertTrue(e.data["native_truncated"])

    def test_small_native_payload_is_kept(self):
        e = ev.make_event("tool.completed", "codex", "personal", {"session_id": "s", "tool_name": "Bash"}, "PostToolUse")
        self.assertEqual(e.native_payload["tool_name"], "Bash")

    def test_event_is_immutable(self):
        e = ev.make_event("turn.stop", "codex", "personal", {"session_id": "s"}, "Stop", {"x": 1})
        with self.assertRaises(Exception):
            e.agent = "other"  # type: ignore[misc]
        with self.assertRaises(TypeError):
            e.data["x"] = 2  # type: ignore[index]


class SerializationTests(unittest.TestCase):
    def test_round_trip_preserves_unicode(self):
        e = ev.make_event("prompt.submit", "codex", "personal", {"session_id": "s"}, "UserPromptSubmit", {"prompt": "ñá 🙂"})
        self.assertEqual(ev.from_json(ev.to_json(e)), e)
        encoded = ev.to_json(e)
        self.assertIn("ñá 🙂", encoded)
        self.assertEqual(json.loads(encoded)["schema"], ev.SCHEMA)

    def test_from_json_rejects_other_schema(self):
        doc = json.loads(ev.to_json(ev.make_event("turn.stop", "codex", "personal", {"session_id": "s"}, "Stop")))
        doc["schema"] = "something/v9"
        with self.assertRaises(ValueError):
            ev.from_json(json.dumps(doc))

    def test_from_json_rejects_unknown_event(self):
        doc = json.loads(ev.to_json(ev.make_event("turn.stop", "codex", "personal", {"session_id": "s"}, "Stop")))
        doc["event"] = "nope"
        with self.assertRaises(ValueError):
            ev.from_json(json.dumps(doc))


if __name__ == "__main__":
    unittest.main()
