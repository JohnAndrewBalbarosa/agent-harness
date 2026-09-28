import contextlib
import io
import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from core import cli


class CliTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        root = Path(self.dir.name)
        self.env = root / ".env"
        self.env.write_text(f"HARNESS_HOME={root}\nINSTANCES=codex:cy=C:/nowhere/.codex-cy\n", encoding="utf-8")
        self.args = ["--env-file", str(self.env), "--log-dir", str(root / "logs")]

    def tearDown(self):
        self.dir.cleanup()

    def run_cli(self, *argv):
        out = io.StringIO()
        with contextlib.redirect_stdout(out), mock.patch.object(cli.shutil, "which", return_value="C:/bin/x.exe"):
            code = cli.main([*argv, *self.args])
        return code, out.getvalue()

    def test_preflight_prints_one_line_and_returns_the_verdict_code(self):
        code, out = self.run_cli("preflight", "gemini")
        self.assertEqual(code, 10)
        self.assertEqual(out.strip(), "agent-harness: gemini is not supported (no adapter); running without the harness")

    def test_agents_lists_support_and_subscriptions_as_json(self):
        code, out = self.run_cli("agents", "--json")
        rows = {r["agent"]: r for r in json.loads(out)}
        self.assertEqual((code, rows["codex"]["instances"], rows["claude"]["instances"]), (0, ["cy"], []))

    def test_bad_configuration_prints_a_message_not_a_traceback(self):
        self.env.write_text("OTLP_PORT=1\n", encoding="utf-8")
        code, out = self.run_cli("preflight", "claude")
        self.assertEqual(code, 15)
        self.assertIn("agent-harness: configuration error", out)

    def test_logs_summarizes_a_bounded_tail(self):
        logs = Path(self.dir.name) / "logs"
        logs.mkdir()
        rows = [{"timestamp": f"t{i}", "operation": "hook.dispatch", "outcome": "succeeded", "details": {}} for i in range(500)]
        rows.append({"timestamp": "t-last", "operation": "hook.dispatch", "outcome": "failed", "details": {"error": "KeyError"}})
        (logs / "hook-router.lifecycle.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows), encoding="utf-8")
        code, out = self.run_cli("logs", "hook-router", "--tail", "100")
        self.assertEqual(code, 0)
        self.assertIn("hook.dispatch succeeded 99", out)
        self.assertIn("KeyError", out)
        self.assertLessEqual(len(out.splitlines()), 20)

    def test_logs_without_component_lists_available_logs(self):
        (Path(self.dir.name) / "logs").mkdir()
        (Path(self.dir.name) / "logs" / "services.lifecycle.jsonl").write_text("", encoding="utf-8")
        code, out = self.run_cli("logs")
        self.assertEqual((code, out.split()), (0, ["services"]))

    def test_unknown_command_is_a_usage_error(self):
        code, _ = self.run_cli("nope")
        self.assertEqual(code, 2)


if __name__ == "__main__":
    unittest.main()
