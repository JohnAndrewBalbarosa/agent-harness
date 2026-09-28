import hashlib
import json
import os
import shutil
import tempfile
import tomllib
import unittest
from pathlib import Path

from adapters.claude import patches as claude_patches
from compile import run as compile_run
from core.config import Config, Instance

OLD_ROUTER = r"C:\x\.codex-shared\tools\global-hook-router.cmd"


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


@unittest.skipUnless(shutil.which("npx"), "rulesync needs npx")
class CompileTempHomeTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        root = Path(self.dir.name)
        self.claude, self.codex, self.harness = root / "home" / ".claude", root / "home" / ".codex", root / "Juan dela Cruz" / "ah"
        for d in (self.claude, self.codex, self.harness / "policy"):
            d.mkdir(parents=True)
        (self.harness / "policy" / "AGENTS.md").write_text("# Policy\n", encoding="utf-8")
        # Installed before compile runs; Codex hooks need its 8.3 short name (the path has spaces).
        (self.harness / "core" / "router").mkdir(parents=True)
        (self.harness / "core" / "router" / "harness-hook.cmd").write_text("@exit /b 0\n", encoding="utf-8")
        (self.claude / "settings.json").write_text(json.dumps({"model": "opus", "env": {"KEEP": "1"}, "hooks": {
            "Stop": [{"hooks": [{"type": "command", "command": "user-own-hook"}]}]}}), encoding="utf-8")
        (self.claude / "CLAUDE.md").write_text("# Mine\n", encoding="utf-8")
        (self.codex / "hooks.json").write_text(json.dumps({"hooks": {"Stop": [
            {"hooks": [{"type": "command", "command": OLD_ROUTER}]}, {"hooks": [{"type": "command", "command": "codex-user-hook"}]}]}}), encoding="utf-8")
        (self.codex / "config.toml").write_text('model = "x"\n\n[features]\nhooks = true\n', encoding="utf-8")
        (self.codex / "AGENTS.md").write_text("old policy\n", encoding="utf-8")
        self.cfg = Config(harness_home=self.harness, python=Path("python.exe"),
                          instances=(Instance("codex", "personal", self.codex), Instance("claude", "default", self.claude)),
                          obs_endpoint="http://127.0.0.1:4319", otlp_port=4320, notify_rate=(5, 60), herdr_bin=None,
                          rulesync_version="22.0.0")

    def tearDown(self):
        self.dir.cleanup()

    def targets(self):
        return [self.claude / "settings.json", self.claude / "CLAUDE.md", self.codex / "hooks.json", self.codex / "config.toml", self.codex / "AGENTS.md"]

    def test_dry_run_writes_nothing(self):
        before = {p: digest(p) for p in self.targets()}
        actions = compile_run.compile_all(self.cfg, dry_run=True)
        self.assertTrue(actions)
        self.assertEqual({p: digest(p) for p in self.targets()}, before)
        self.assertFalse((self.harness / "var" / "backups").exists())

    def test_apply_merges_preserves_and_is_idempotent(self):
        compile_run.compile_all(self.cfg, dry_run=False)
        settings = json.loads((self.claude / "settings.json").read_text(encoding="utf-8"))
        self.assertEqual((settings["model"], settings["env"]["KEEP"], settings["env"]["OTEL_LOGS_EXPORTER"]), ("opus", "1", "otlp"))
        stop = [h["command"] for g in settings["hooks"]["Stop"] for h in g["hooks"]]
        self.assertIn("user-own-hook", stop)
        self.assertTrue(any("harness-hook.cmd" in c and c.startswith('"') and c.endswith(" claude") for c in stop))
        claude_md = (self.claude / "CLAUDE.md").read_text(encoding="utf-8")
        self.assertTrue(claude_md.startswith("# Mine"))
        self.assertIn(claude_patches.policy_import(self.cfg), claude_md)
        hooks = json.loads((self.codex / "hooks.json").read_text(encoding="utf-8"))
        codex_stop = [h["command"] for g in hooks["hooks"]["Stop"] for h in g["hooks"]]
        self.assertIn("codex-user-hook", codex_stop)
        self.assertNotIn(OLD_ROUTER, codex_stop)
        self.assertTrue(any(c.endswith(" codex") and not c.startswith('"') for c in codex_stop))
        toml = tomllib.loads((self.codex / "config.toml").read_text(encoding="utf-8"))
        self.assertEqual((toml["model"], toml["features"]["hooks"]), ("x", True))
        self.assertIn("otlp-http", toml["otel"]["exporter"])
        self.assertTrue(os.path.samefile(self.codex / "AGENTS.md", self.harness / "policy" / "AGENTS.md"))
        self.assertTrue(any((self.harness / "var" / "backups").rglob("settings.json")))
        self.assertFalse((self.claude / compile_run.LOCK_FILE).exists(), "rulesync lock leaked into the agent home")
        self.assertEqual(compile_run.compile_all(self.cfg, dry_run=True), [])

    def test_hard_linked_codex_hooks_stay_linked(self):
        other = self.codex.parent / ".codex-cy"
        other.mkdir()
        os.link(self.codex / "hooks.json", other / "hooks.json")
        (other / "config.toml").write_text('model = "x"\n', encoding="utf-8")
        cfg = Config(**{**self.cfg.__dict__, "instances": self.cfg.instances + (Instance("codex", "cy", other),)})
        compile_run.compile_all(cfg, dry_run=False)
        self.assertTrue(os.path.samefile(self.codex / "hooks.json", other / "hooks.json"))
        self.assertEqual(compile_run.compile_all(cfg, dry_run=True), [])

    def test_invalid_existing_json_aborts_before_writing(self):
        (self.claude / "settings.json").write_text("{broken", encoding="utf-8")
        before = {p: digest(p) for p in self.targets()}
        with self.assertRaises(compile_run.CompileError):
            compile_run.compile_all(self.cfg, dry_run=False)
        self.assertEqual({p: digest(p) for p in self.targets()}, before)

    def test_unmanaged_otel_table_is_a_conflict(self):
        (self.codex / "config.toml").write_text('[otel]\nexporter = "none"\n', encoding="utf-8")
        with self.assertRaises(compile_run.CompileError) as ctx:
            compile_run.compile_all(self.cfg, dry_run=True)
        self.assertIn("[otel]", str(ctx.exception))


if __name__ == "__main__":
    unittest.main()
