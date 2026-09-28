import tomllib
import unittest
from pathlib import Path

PLUGIN = Path(__file__).resolve().parents[2] / "herdr-plugin"


class ManifestTests(unittest.TestCase):
    def setUp(self):
        self.manifest = tomllib.loads((PLUGIN / "herdr-plugin.toml").read_text(encoding="utf-8"))

    def test_identity_and_platform(self):
        self.assertEqual((self.manifest["id"], self.manifest["min_herdr_version"]), ("agent-harness", "0.9.1"))
        self.assertIn("windows", self.manifest["platforms"])

    def test_agent_detection_runs_preflight(self):
        events = {e["on"]: e["command"] for e in self.manifest["events"]}
        self.assertEqual(events["pane.agent_detected"][-1], "agent-detected")

    def test_startup_ensures_services(self):
        self.assertEqual(self.manifest["startup"][0]["command"][-1], "services")

    def test_every_command_script_exists_in_the_plugin(self):
        for entry in self.manifest["events"] + self.manifest["startup"]:
            scripts = [part for part in entry["command"] if part.endswith(".cmd")]
            self.assertTrue(scripts and all((PLUGIN / s).exists() for s in scripts), entry)


if __name__ == "__main__":
    unittest.main()
