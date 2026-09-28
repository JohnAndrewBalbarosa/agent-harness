import json
import tempfile
import time
import unittest
from pathlib import Path

from core.events import make_event
from core.router.consumers import consumers_for
from core.services import bootstrap


class Recorder:
    def __init__(self):
        self.launched = []

    def __call__(self, command, cwd):
        self.launched.append((tuple(str(c) for c in command), str(cwd)))


class RegistryTests(unittest.TestCase):
    def test_services_consumer_runs_on_session_start_only(self):
        start = make_event("session.start", "claude", "default", {"session_id": "s"}, "SessionStart")
        stop = make_event("turn.stop", "claude", "default", {"session_id": "s"}, "Stop")
        self.assertIn("services", [c.name for c in consumers_for(start, {})])
        self.assertNotIn("services", [c.name for c in consumers_for(stop, {})])


class BootstrapTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.root = Path(self.dir.name)
        self.orb_health = self.root / "orb-health.json"
        self.calls = []
        self.launch = Recorder()
        self.layout = bootstrap.Layout(root=self.root, orb_health=self.orb_health, obs_endpoint="http://127.0.0.1:1")

    def tearDown(self):
        self.dir.cleanup()

    def run_bootstrap(self, healthy=lambda url: False):
        return bootstrap.run(self.layout, launch=self.launch, healthy=healthy,
                             ensure_receiver=lambda: self.calls.append("receiver"))

    def test_receiver_is_always_ensured(self):
        self.run_bootstrap()
        self.assertEqual(self.calls, ["receiver"])

    def test_orb_starts_only_when_installed_and_not_healthy(self):
        self.run_bootstrap()
        self.assertEqual(self.launch.launched, [])
        pythonw = self.root / "tools" / "codex-status-orb" / ".venv" / "Scripts" / "pythonw.exe"
        pythonw.parent.mkdir(parents=True)
        pythonw.write_bytes(b"")
        self.run_bootstrap()
        self.assertEqual(len(self.launch.launched), 1)
        self.assertTrue(self.launch.launched[0][0][-1].endswith("orb.py"))
        self.orb_health.write_text(json.dumps({"status": "healthy", "updated_at": time.time()}), encoding="utf-8")
        self.run_bootstrap()
        self.assertEqual(len(self.launch.launched), 1)

    def test_hub_starts_only_when_installed_and_down(self):
        start = self.root / "hub" / "start.ps1"
        start.parent.mkdir(parents=True)
        start.write_text("", encoding="utf-8")
        self.run_bootstrap(healthy=lambda url: True)
        self.assertEqual(self.launch.launched, [])
        self.run_bootstrap(healthy=lambda url: False)
        self.assertTrue(any("start.ps1" in part for part in self.launch.launched[0][0]))

    def test_hub_outside_the_harness_comes_from_hub_dir(self):
        hub = self.root / "elsewhere" / "observability-hub"
        hub.mkdir(parents=True)
        (hub / "start.ps1").write_text("", encoding="utf-8")
        layout = bootstrap.default_layout({"HUB_DIR": str(hub), "LOCALAPPDATA": str(self.root)})
        self.assertEqual(layout.hub_dir, hub)
        self.assertEqual(bootstrap.ensure_hub(layout, self.launch, lambda url: False), "requested")
        self.assertIn(str(hub / "start.ps1"), self.launch.launched[0][0])

    def test_console_monitor_is_launched_when_present(self):
        self.run_bootstrap()
        self.assertEqual(self.launch.launched, [])
        monitor = self.root / "tools" / "observability-client" / "console_window_monitor.py"
        monitor.parent.mkdir(parents=True)
        monitor.write_text("", encoding="utf-8")
        self.run_bootstrap()
        self.assertEqual(self.launch.launched[0][0][-1], str(monitor))

    def test_second_bootstrap_is_a_no_op_while_one_runs(self):
        with bootstrap.single_instance(self.root) as first:
            self.assertTrue(first)
            with bootstrap.single_instance(self.root) as second:
                self.assertFalse(second)

    def test_every_step_is_logged(self):
        self.run_bootstrap()
        log = self.root / "var" / "logs" / "services.lifecycle.jsonl"
        operations = [json.loads(line)["operation"] for line in log.read_text(encoding="utf-8").splitlines()]
        self.assertIn("receiver.ensure", operations)
        self.assertIn("orb.ensure", operations)


if __name__ == "__main__":
    unittest.main()
