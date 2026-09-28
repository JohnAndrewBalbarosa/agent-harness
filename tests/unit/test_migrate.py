import tomllib
import unittest

from scripts import migrate_codex_shared as migrate

SETTINGS = {
    "model": "opus",
    "hooks": {
        "Stop": [
            {"hooks": [{"type": "command", "command": 'powershell -File "C:\\u\\.claude\\scripts\\notify-gate.ps1" -Event Stop'}]},
            {"hooks": [{"type": "command", "command": 'powershell -File "C:\\u\\.claude\\hooks\\obs-hook.ps1"'}]},
            {"hooks": [{"type": "command", "command": "user-own-stop"}]},
        ],
        "SessionStart": [
            {"hooks": [{"type": "command", "command": "python C:/u/.claude/hooks/claude_otel_receiver.py ensure"}]},
        ],
        "SubagentStart": [{"matcher": "", "hooks": [
            {"type": "command", "command": "python C:/u/.claude/scripts/background_tasks.py hook"},
            {"type": "command", "command": "keep-me"}]}],
    },
}


class RetireShimsTests(unittest.TestCase):
    def test_removes_only_legacy_shim_hooks(self):
        out, removed = migrate.retire_claude_shims(SETTINGS)
        self.assertEqual(removed, 4)
        self.assertEqual([h["command"] for g in out["hooks"]["Stop"] for h in g["hooks"]], ["user-own-stop"])
        self.assertNotIn("SessionStart", out["hooks"])
        self.assertEqual([h["command"] for g in out["hooks"]["SubagentStart"] for h in g["hooks"]], ["keep-me"])
        self.assertEqual(out["model"], "opus")

    def test_does_not_mutate_input_and_is_idempotent(self):
        before = repr(SETTINGS)
        once, _ = migrate.retire_claude_shims(SETTINGS)
        self.assertEqual(repr(SETTINGS), before)
        self.assertEqual(migrate.retire_claude_shims(once), (once, 0))


class CommonOtelTests(unittest.TestCase):
    ENDPOINT = "http://127.0.0.1:4320/v1/logs"

    def test_appends_the_exporter_table(self):
        out = migrate.add_common_otel('model = "x"\n\n[features]\nhooks = true\n', self.ENDPOINT)
        parsed = tomllib.loads(out)
        self.assertEqual(parsed["otel"]["exporter"]["otlp-http"], {"endpoint": self.ENDPOINT, "protocol": "json"})
        self.assertTrue(parsed["features"]["hooks"])

    def test_is_idempotent(self):
        once = migrate.add_common_otel('model = "x"\n', self.ENDPOINT)
        self.assertEqual(migrate.add_common_otel(once, self.ENDPOINT), once)

    def test_refuses_a_different_existing_otel(self):
        with self.assertRaises(ValueError):
            migrate.add_common_otel('[otel]\nexporter = "none"\n', self.ENDPOINT)


class LegacyReceiverTests(unittest.TestCase):
    def test_stop_query_only_matches_python_and_never_itself(self):
        script = migrate.STOP_LEGACY_RECEIVER
        self.assertIn("Name like 'python%'", script)
        self.assertIn("$PID", script)


class PolicyImportTests(unittest.TestCase):
    def test_drops_the_legacy_import_line_only(self):
        text = "# Mine\n\n@~/.codex-shared/AGENTS.md\n\nrest\n"
        self.assertEqual(migrate.drop_legacy_import(text), "# Mine\n\n\nrest\n")
        self.assertEqual(migrate.drop_legacy_import("no import\n"), "no import\n")


if __name__ == "__main__":
    unittest.main()
