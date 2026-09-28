import tempfile
import unittest
from pathlib import Path, PureWindowsPath

from core import config

ENVIRON = {"USERPROFILE": r"C:\Users\Juan dela Cruz", "APPDATA": r"C:\Users\Juan dela Cruz\AppData\Roaming"}
EXAMPLE = r"""
# agent-harness configuration
HARNESS_HOME=%USERPROFILE%\.agent-harness
INSTANCES="codex:personal=%USERPROFILE%\.codex;codex:cy=%USERPROFILE%\.codex-cy;claude:default=%USERPROFILE%\.claude"
OTLP_PORT=4320
"""


def load(text=EXAMPLE, environ=None):
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / ".env"
        path.write_text(text, encoding="utf-8")
        return config.load(path, {**ENVIRON, **(environ or {})})


class ParseTests(unittest.TestCase):
    def test_parses_comments_quotes_and_blank_lines(self):
        values = config.parse_env_text('# c\n\nA=1\nB="two words"\nC=\'x\'\n')
        self.assertEqual(values, {"A": "1", "B": "two words", "C": "x"})

    def test_expands_percent_variables(self):
        self.assertEqual(config.expand(r"%USERPROFILE%\.x", ENVIRON), r"C:\Users\Juan dela Cruz\.x")
        self.assertEqual(config.expand("%UNSET%/y", ENVIRON), "%UNSET%/y")


class LoadTests(unittest.TestCase):
    def test_loads_instances_and_defaults(self):
        cfg = load()
        self.assertEqual(PureWindowsPath(cfg.harness_home), PureWindowsPath(r"C:\Users\Juan dela Cruz\.agent-harness"))
        self.assertEqual([(i.agent, i.name) for i in cfg.instances], [("codex", "personal"), ("codex", "cy"), ("claude", "default")])
        self.assertEqual((cfg.obs_endpoint, cfg.otlp_port, cfg.notify_rate, cfg.rulesync_version),
                         ("http://127.0.0.1:4319", 4320, (5, 60), "22.0.0"))
        self.assertIsNone(cfg.herdr_bin)
        self.assertTrue(str(cfg.python).endswith("python.exe"))

    def test_environment_overrides_file(self):
        self.assertEqual(load(environ={"OTLP_PORT": "5555"}).otlp_port, 5555)

    def test_missing_required_key_names_it(self):
        with self.assertRaises(ValueError) as ctx:
            load("OTLP_PORT=1\n")
        self.assertIn("HARNESS_HOME", str(ctx.exception))

    def test_bad_instance_entry_is_rejected(self):
        with self.assertRaises(ValueError) as ctx:
            load("HARNESS_HOME=C:/h\nINSTANCES=codex-without-home\n")
        self.assertIn("INSTANCES", str(ctx.exception))

    def test_bad_port_is_rejected(self):
        with self.assertRaises(ValueError):
            load(environ={"OTLP_PORT": "abc"})


class HookCommandTests(unittest.TestCase):
    def test_hook_commands_quote_paths_with_spaces(self):
        command = config.hook_command(load(), "claude")
        self.assertTrue(command.startswith('"C:'))
        self.assertIn(r"Juan dela Cruz\.agent-harness\core\router\harness-hook.cmd" + '" claude', command)

    def test_native_event_argument(self):
        self.assertTrue(config.hook_command(load(), "claude", "Stop").endswith('" claude Stop'))

    def test_bare_style_never_starts_with_a_quote(self):
        # Codex's Windows command runner rejects a command whose executable starts with a quote.
        cfg = load("HARNESS_HOME=C:/nospace/ah\nINSTANCES=codex:personal=C:/h/.codex\n")
        command = config.hook_command(cfg, "codex", style="bare")
        self.assertEqual(command, str(Path("C:/nospace/ah/core/router/harness-hook.cmd")) + " codex")

    def test_bare_style_with_spaces_uses_short_path_or_refuses(self):
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp) / "Juan dela Cruz" / ".agent-harness"
            (home / "core" / "router").mkdir(parents=True)
            (home / "core" / "router" / "harness-hook.cmd").write_text("@echo off\n", encoding="utf-8")
            cfg = load(f"HARNESS_HOME={home}\nINSTANCES=codex:personal=C:/h/.codex\n")
            try:
                command = config.hook_command(cfg, "codex", style="bare")
            except ValueError as error:
                self.assertIn("spaces", str(error))
                return
            executable = command.rsplit(" ", 1)[0]
            self.assertFalse(command.startswith('"'))
            self.assertNotIn(" ", executable)
            # compile recognizes harness-owned hook entries by this file name; an 8.3 name (HARNES~1.CMD) breaks that
            self.assertTrue(executable.endswith("\\harness-hook.cmd"), executable)

    def test_bare_style_refuses_unresolvable_path_with_spaces(self):
        cfg = load()  # C:\Users\Juan dela Cruz\... does not exist, so no 8.3 name
        with self.assertRaises(ValueError):
            config.hook_command(cfg, "codex", style="bare")


class ExampleFileTests(unittest.TestCase):
    def test_repo_env_example_loads(self):
        example = Path(__file__).resolve().parents[2] / ".env.example"
        cfg = config.load(example, ENVIRON)
        self.assertTrue({"codex", "claude"} <= {i.agent for i in cfg.instances})


class RuntimeEnvTests(unittest.TestCase):
    def runtime(self, text, environ=None):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / ".env"
            path.write_text(text, encoding="utf-8")
            return config.runtime_env(path, {**ENVIRON, **(environ or {})})

    def test_file_values_reach_consumers_expanded(self):
        env = self.runtime("OTLP_PORT=4399\nNOTIFY_RATE_LIMIT=2/30\nOBS_ENDPOINT=http://x:1\nHARNESS_PYTHON=%APPDATA%\py.exe\n")
        self.assertEqual((env["OTLP_PORT"], env["NOTIFY_RATE_LIMIT"], env["OBS_ENDPOINT"]), ("4399", "2/30", "http://x:1"))
        self.assertEqual(env["HARNESS_PYTHON"], r"C:\Users\Juan dela Cruz\AppData\Roaming\py.exe")
        self.assertEqual(env["USERPROFILE"], ENVIRON["USERPROFILE"])

    def test_process_environment_wins(self):
        self.assertEqual(self.runtime("OTLP_PORT=4399\n", {"OTLP_PORT": "5000"})["OTLP_PORT"], "5000")

    def test_herdr_bin_is_exported_under_the_bridge_name(self):
        self.assertEqual(self.runtime("HERDR_BIN=C:\h\herdr.exe\n")["HERDR_BIN_PATH"], r"C:\h\herdr.exe")
        self.assertEqual(self.runtime("HERDR_BIN=C:\h\herdr.exe\n", {"HERDR_BIN_PATH": "D:\o.exe"})["HERDR_BIN_PATH"], r"D:\o.exe")

    def test_missing_file_returns_environment(self):
        env = config.runtime_env(Path("does-not-exist.env"), {"A": "1"})
        self.assertEqual(env, {"A": "1"})

    def test_unknown_keys_are_not_exported(self):
        self.assertNotIn("SECRET", self.runtime("SECRET=x\n"))


if __name__ == "__main__":
    unittest.main()
