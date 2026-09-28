import dataclasses
import tempfile
import unittest
from pathlib import Path

from adapters.claude import patches as claude_patches
from compile import patch, render
from core import config
from tests.unit.test_compile_units import MANIFESTS, cfg


def with_rtk(value="rtk"):
    return dataclasses.replace(cfg(r"C:\nospace\ah"), rtk_bin=value)


class RtkConfigTests(unittest.TestCase):
    def test_disabled_by_default_and_read_from_env(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = Path(tmp) / ".env"
            env.write_text("HARNESS_HOME=C:\\ah\nINSTANCES=claude:default=C:\\h\\.claude\n", encoding="utf-8")
            self.assertIsNone(config.load(env, {}).rtk_bin)
            env.write_text(env.read_text(encoding="utf-8") + "RTK_BIN=rtk\n", encoding="utf-8")
            self.assertEqual(config.load(env, {}).rtk_bin, "rtk")


class RtkRenderTests(unittest.TestCase):
    def test_bash_pre_tool_use_hook_per_agent_when_enabled(self):
        from unittest import mock
        with mock.patch.object(render.shutil, "which", return_value="C:/bin/rtk.exe"):
            doc = render.hooks_source(with_rtk(), MANIFESTS)
        self.assertEqual(doc["claudecode"]["hooks"]["preToolUse"],
                         [{"matcher": "Bash", "command": "rtk hook claude", "timeout": 10}])
        self.assertEqual(doc["codexcli"]["hooks"]["preToolUse"],
                         [{"matcher": "Bash", "command": "rtk hook codex", "timeout": 10}])

    def test_no_hook_when_bare_rtk_does_not_resolve_on_path(self):
        # rtk rewrites commands to a bare `rtk <cmd>`; without it on PATH every Bash call would fail (exit 127).
        from unittest import mock
        with mock.patch.object(render.shutil, "which", return_value=None):
            doc = render.hooks_source(with_rtk(), MANIFESTS)
        self.assertNotIn("preToolUse", doc["claudecode"]["hooks"])

    def test_no_pre_tool_use_hook_when_disabled(self):
        doc = render.hooks_source(cfg(r"C:\nospace\ah"), MANIFESTS)
        self.assertNotIn("preToolUse", doc["claudecode"]["hooks"])
        self.assertNotIn("preToolUse", doc["codexcli"]["hooks"])


class RtkOwnershipTests(unittest.TestCase):
    def test_rtk_entries_are_harness_owned_in_codex_hooks(self):
        existing = {"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "rtk hook codex"}]},
                                             {"hooks": [{"type": "command", "command": "user-pre"}]}]}}
        merged = patch.merge_codex_hooks(existing, {"hooks": {}})
        self.assertEqual([h["command"] for g in merged["hooks"]["PreToolUse"] for h in g["hooks"]], ["user-pre"])


class RtkClaudeNoteTests(unittest.TestCase):
    def test_claude_import_block_adds_the_rtk_note_only_when_enabled(self):
        home = Path("C:/Users/x")
        on = dataclasses.replace(cfg("C:/Users/x/.agent-harness"), rtk_bin="rtk")
        self.assertIn("@~/.agent-harness/policy/RTK.md", claude_patches.policy_import(on, home))
        self.assertNotIn("RTK.md", claude_patches.policy_import(cfg("C:/Users/x/.agent-harness"), home))


if __name__ == "__main__":
    unittest.main()
