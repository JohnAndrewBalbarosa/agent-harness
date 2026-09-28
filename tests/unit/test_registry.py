import json
import tempfile
import unittest
from datetime import datetime, timedelta, timezone
from pathlib import Path

from core import registry

NOW = datetime(2026, 9, 28, 12, 0, tzinfo=timezone.utc)


class Fixture:
    def __init__(self, tmp: Path):
        self.root = tmp
        self.home = tmp / "home" / ".claude"
        self.home.mkdir(parents=True)
        self.env_file = tmp / ".env"
        self.env_file.write_text(f"HARNESS_HOME={tmp}\nINSTANCES=claude:default={self.home}\n", encoding="utf-8")
        self.log_dir = tmp / "logs"
        self.compiled = []

    def configure(self):
        (self.home / "settings.json").write_text(json.dumps({"hooks": {"Stop": [{"hooks": [
            {"type": "command", "command": f'"{self.root}\\core\\router\\harness-hook.cmd" claude'}]}]}}), encoding="utf-8")

    def run(self, agent="claude", init=False, which=lambda name: f"C:/bin/{name}.exe", failures=None):
        router_log = self.log_dir / "hook-router.lifecycle.jsonl"
        self.log_dir.mkdir(exist_ok=True)
        router_log.write_text("".join(json.dumps(r) + "\n" for r in (failures or [])), encoding="utf-8")
        return registry.preflight(agent, env_file=self.env_file, environ={}, init=init, which=which,
                                  compile_instance=lambda cfg: (self.compiled.append(cfg), self.configure()),
                                  log_dir=self.log_dir, now=NOW)


class PreflightTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.fx = Fixture(Path(self.dir.name))

    def tearDown(self):
        self.dir.cleanup()

    def steps(self):
        lines = (self.fx.log_dir / "preflight.lifecycle.jsonl").read_text(encoding="utf-8").splitlines()
        return [(json.loads(l)["operation"], json.loads(l)["outcome"]) for l in lines]

    def test_unsupported_agent(self):
        verdict = self.fx.run("gemini")
        self.assertEqual((verdict.status, verdict.code), ("unsupported", 10))
        self.assertIn("not supported", verdict.message)
        self.assertEqual(self.steps()[-1], ("preflight.verdict", "failed"))

    def test_not_installed(self):
        verdict = self.fx.run(which=lambda name: None)
        self.assertEqual((verdict.status, verdict.code), ("not_installed", 11))

    def test_not_configured_without_init(self):
        verdict = self.fx.run()
        self.assertEqual((verdict.status, verdict.code), ("not_configured", 13))
        self.assertEqual(self.fx.compiled, [])

    def test_init_configures_then_ready(self):
        verdict = self.fx.run(init=True)
        self.assertEqual((verdict.status, verdict.code), ("ready", 0))
        self.assertEqual(len(self.fx.compiled), 1)
        self.assertIn(("step.configured", "initialized"), self.steps())

    def test_not_subscribed_and_init_subscribes_with_default_home(self):
        self.fx.env_file.write_text(f"HARNESS_HOME={self.fx.root}\nINSTANCES=codex:personal=C:/h/.codex\n", encoding="utf-8")
        self.assertEqual(self.fx.run().status, "not_subscribed")
        self.fx.run(init=True)
        self.assertIn("claude:default=", self.fx.env_file.read_text(encoding="utf-8"))
        self.assertIn(("step.subscribed", "initialized"), self.steps())

    def test_supported_but_broken_is_reported_with_reason(self):
        self.fx.configure()
        failed = {"timestamp": (NOW - timedelta(hours=1)).isoformat(), "component": "router", "operation": "hook.dispatch",
                  "outcome": "failed", "details": {"agent": "claude", "error": "KeyError"}}
        verdict = self.fx.run(failures=[failed])
        self.assertEqual((verdict.status, verdict.code), ("broken", 14))
        self.assertIn("KeyError", verdict.message)

    def test_old_failures_do_not_count(self):
        self.fx.configure()
        old = {"timestamp": (NOW - timedelta(days=3)).isoformat(), "component": "router", "operation": "hook.dispatch",
               "outcome": "failed", "details": {"agent": "claude", "error": "KeyError"}}
        self.assertEqual(self.fx.run(failures=[old]).status, "ready")


class RoutesTests(unittest.TestCase):
    def test_recognizes_quoted_and_bare_harness_commands_for_the_agent_only(self):
        self.assertTrue(registry._routes('"C:\\ah\\core\\router\\harness-hook.cmd" claude', "claude"))
        self.assertTrue(registry._routes("C:\\JUANDE~1\\ah\\core\\router\\harness-hook.cmd codex", "codex"))
        self.assertFalse(registry._routes("C:\\ah\\core\\router\\harness-hook.cmd codex", "claude"))
        self.assertFalse(registry._routes("rtk hook claude", "claude"))


class RegistryTests(unittest.TestCase):
    def test_lists_supported_agents_with_subscription(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = Path(tmp) / ".env"
            env.write_text(f"HARNESS_HOME={tmp}\nINSTANCES=codex:cy=C:/h/.codex-cy\n", encoding="utf-8")
            rows = {r["agent"]: r for r in registry.agents(env_file=env, environ={}, which=lambda n: None)}
        self.assertEqual(sorted(rows), ["claude", "codex"])
        self.assertEqual(rows["codex"]["instances"], ["cy"])
        self.assertEqual(rows["claude"]["instances"], [])

    def test_add_instance_is_pure_and_idempotent(self):
        text = "HARNESS_HOME=C:/ah\nINSTANCES=codex:cy=C:/h/.codex-cy\n"
        once = registry.add_instance(text, "claude", "default", "C:/h/.claude")
        self.assertIn("INSTANCES=codex:cy=C:/h/.codex-cy;claude:default=C:/h/.claude", once)
        self.assertEqual(registry.add_instance(once, "claude", "default", "C:/h/.claude"), once)


if __name__ == "__main__":
    unittest.main()
