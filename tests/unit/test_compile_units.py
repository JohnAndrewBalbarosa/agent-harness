import unittest
from pathlib import Path

from compile import patch, render
from core.config import Config, Instance


def cfg(home=r"C:\Users\Juan dela Cruz\.agent-harness"):
    return Config(harness_home=Path(home), python=Path("python.exe"),
                  instances=(Instance("codex", "personal", Path("C:/h/.codex")), Instance("claude", "default", Path("C:/h/.claude"))),
                  obs_endpoint="http://127.0.0.1:4319", otlp_port=4320, notify_rate=(5, 60), herdr_bin=None, rulesync_version="22.0.0")


MANIFESTS = {
    "codex": {"agent": "codex", "rulesync_target": "codexcli", "hook_command_style": "bare",
              "events": ["SessionStart", "UserPromptSubmit", "Stop", "PermissionRequest", "SessionEnd"]},
    "claude": {"agent": "claude", "rulesync_target": "claudecode", "events": ["SessionStart", "UserPromptSubmit", "Stop", "Notification", "SubagentStart"]},
}


class RenderTests(unittest.TestCase):
    def test_hooks_source_per_agent_with_canonical_names_and_quoted_command(self):
        doc = render.hooks_source(cfg(), {"claude": MANIFESTS["claude"]})
        self.assertEqual(doc["hooks"], {})
        claude = doc["claudecode"]["hooks"]
        self.assertEqual(sorted(claude), sorted(["sessionStart", "beforeSubmitPrompt", "stop", "notification", "subagentStart"]))
        self.assertEqual(claude["stop"][0]["command"],
                         r'"C:\Users\Juan dela Cruz\.agent-harness\core\router\harness-hook.cmd" claude')
        codex = render.hooks_source(cfg(r"C:\nospace\ah"), MANIFESTS)["codexcli"]["hooks"]
        self.assertEqual(codex["permissionRequest"][0]["timeout"], 600)
        self.assertEqual(codex["sessionEnd"][0]["timeout"], 10)
        self.assertEqual(codex["stop"][0]["command"], r"C:\nospace\ah\core\router\harness-hook.cmd codex")

    def test_codex_hooks_refuse_unresolvable_paths_with_spaces(self):
        with self.assertRaises(ValueError):
            render.hooks_source(cfg(), MANIFESTS)

    def test_unknown_native_events_are_skipped(self):
        doc = render.hooks_source(cfg(), {"claude": {**MANIFESTS["claude"], "events": ["Stop", "FutureEvent"]}})
        self.assertEqual(list(doc["claudecode"]["hooks"]), ["stop"])

    def test_rulesync_config_is_safe_for_real_homes(self):
        conf = render.rulesync_config(["claudecode"])
        self.assertEqual((conf["delete"], conf["preserveUnownedHooks"], conf["global"], conf["features"]), (False, True, True, ["hooks"]))


class SkillTests(unittest.TestCase):
    def test_skill_names_the_installed_cli_and_its_commands(self):
        text = render.skill(cfg(r"C:\Users\Juan dela Cruz\.agent-harness"))
        self.assertTrue(text.startswith("---\nname: agent-harness\ndescription: "))
        self.assertIn(r'"C:\Users\Juan dela Cruz\.agent-harness\core\harness.cmd" usage', text)
        for command in ("usage", "logs", "agents", "preflight"):
            self.assertIn(f" {command}", text)


class MergeEnvTests(unittest.TestCase):
    def test_sets_managed_keys_and_keeps_others(self):
        merged = patch.merge_env({"model": "opus", "env": {"KEEP": "1"}}, {"A": "x"}, previously_managed=set())
        self.assertEqual(merged, {"model": "opus", "env": {"KEEP": "1", "A": "x"}})

    def test_removes_previously_managed_keys_no_longer_wanted(self):
        merged = patch.merge_env({"env": {"KEEP": "1", "OLD": "y"}}, {"A": "x"}, previously_managed={"OLD"})
        self.assertEqual(merged["env"], {"KEEP": "1", "A": "x"})

    def test_does_not_mutate_input(self):
        original = {"env": {"KEEP": "1"}}
        patch.merge_env(original, {"A": "x"}, set())
        self.assertEqual(original, {"env": {"KEEP": "1"}})


class MergeBlockTests(unittest.TestCase):
    BEGIN, END = "<!-- agent-harness:begin -->", "<!-- agent-harness:end -->"

    def test_appends_when_missing(self):
        out = patch.merge_block("# Mine\n", "@policy", self.BEGIN, self.END)
        self.assertEqual(out, f"# Mine\n\n{self.BEGIN}\n@policy\n{self.END}\n")

    def test_replaces_existing_block_only(self):
        text = f"top\n{self.BEGIN}\nold\n{self.END}\nbottom\n"
        self.assertEqual(patch.merge_block(text, "new", self.BEGIN, self.END), f"top\n{self.BEGIN}\nnew\n{self.END}\nbottom\n")

    def test_is_idempotent(self):
        once = patch.merge_block("x\n", "b", self.BEGIN, self.END)
        self.assertEqual(patch.merge_block(once, "b", self.BEGIN, self.END), once)


class MergeCodexHooksTests(unittest.TestCase):
    def test_replaces_harness_owned_entries_and_keeps_user_hooks(self):
        existing = {"hooks": {
            "Stop": [{"hooks": [{"type": "command", "command": r"C:\x\.codex-shared\tools\global-hook-router.cmd"}]},
                     {"hooks": [{"type": "command", "command": "my-own-stop-hook"}]}],
            "PreToolUse": [{"hooks": [{"type": "command", "command": "user-pre"}]}],
        }}
        generated = {"hooks": {"Stop": [{"hooks": [{"type": "command", "command": '"C:\\h\\harness-hook.cmd" codex'}]}]}}
        merged = patch.merge_codex_hooks(existing, generated)
        stop_commands = [h["command"] for group in merged["hooks"]["Stop"] for h in group["hooks"]]
        self.assertEqual(stop_commands, ['"C:\\h\\harness-hook.cmd" codex', "my-own-stop-hook"])
        self.assertEqual(merged["hooks"]["PreToolUse"], existing["hooks"]["PreToolUse"])

    def test_events_no_longer_generated_lose_only_harness_entries(self):
        existing = {"hooks": {"SubagentStop": [{"hooks": [{"type": "command", "command": "x\\global-hook-router.cmd"}]}]}}
        self.assertEqual(patch.merge_codex_hooks(existing, {"hooks": {}})["hooks"], {})


class ApplyGuardTests(unittest.TestCase):
    def test_file_changed_after_planning_aborts_without_writing(self):
        import tempfile
        from compile import run
        with tempfile.TemporaryDirectory() as tmp:
            first, second = Path(tmp) / "a.json", Path(tmp) / "b.json"
            first.write_text("old-a", encoding="utf-8")
            second.write_text("old-b", encoding="utf-8")
            actions = [run.Write(first, "old-a", "new-a"), run.Write(second, "old-b", "new-b")]
            second.write_text("edited by the agent meanwhile", encoding="utf-8")
            with self.assertRaises(run.CompileError):
                run.apply(cfg(tmp), actions)
            self.assertEqual((first.read_text(encoding="utf-8"), second.read_text(encoding="utf-8")),
                             ("old-a", "edited by the agent meanwhile"))


class HardLinkTests(unittest.TestCase):
    def test_apply_keeps_hard_link_groups(self):
        import os
        import tempfile
        from compile import run
        with tempfile.TemporaryDirectory() as tmp:
            shared, profile = Path(tmp) / "shared.json", Path(tmp) / "profile.json"
            shared.write_text("old", encoding="utf-8")
            os.link(shared, profile)
            run.apply(cfg(tmp), [run.Write(profile, "old", "new")])
            self.assertTrue(os.path.samefile(shared, profile))
            self.assertEqual(shared.read_text(encoding="utf-8"), "new")

    def test_writes_to_one_file_through_several_links_are_planned_once(self):
        import os
        import tempfile
        from compile import run
        with tempfile.TemporaryDirectory() as tmp:
            a, b = Path(tmp) / "a.json", Path(tmp) / "b.json"
            a.write_text("old", encoding="utf-8")
            os.link(a, b)
            actions = run.dedupe([run.Write(a, "old", "new"), run.Write(b, "old", "new")])
            self.assertEqual(actions, [run.Write(a, "old", "new")])

    def test_conflicting_writes_to_one_file_are_refused(self):
        import os
        import tempfile
        from compile import run
        with tempfile.TemporaryDirectory() as tmp:
            a, b = Path(tmp) / "a.json", Path(tmp) / "b.json"
            a.write_text("old", encoding="utf-8")
            os.link(a, b)
            with self.assertRaises(run.CompileError):
                run.dedupe([run.Write(a, "old", "x"), run.Write(b, "old", "y")])


class CodexConfigTests(unittest.TestCase):
    def test_adds_the_managed_block(self):
        from compile import run
        out = run.codex_config('model = "x"\n', cfg())
        self.assertIn("otlp-http", out)
        self.assertIn("agent-harness managed", out)

    def test_equal_otel_from_a_generated_config_is_accepted_unchanged(self):
        from compile import run
        text = ('model = "x"\n\n[otel.exporter.otlp-http]\n'
                'endpoint = "http://127.0.0.1:4320/v1/logs"\nprotocol = "json"\n')
        self.assertEqual(run.codex_config(text, cfg()), text)

    def test_different_unmanaged_otel_is_a_conflict(self):
        from compile import run
        with self.assertRaises(run.CompileError):
            run.codex_config('[otel]\nexporter = "none"\n', cfg())


if __name__ == "__main__":
    unittest.main()
