import dataclasses
import tempfile
import unittest
from pathlib import Path

from adapters.claude import patches as claude_patches
from core import config
from tests.unit.test_compile_units import cfg


class AutoCompactTests(unittest.TestCase):
    def test_env_values_are_loaded_and_validated(self):
        with tempfile.TemporaryDirectory() as tmp:
            env = Path(tmp) / ".env"
            base = "HARNESS_HOME=C:\\ah\nINSTANCES=claude:default=C:\\h\\.claude\n"
            env.write_text(base, encoding="utf-8")
            loaded = config.load(env, {})
            self.assertEqual((loaded.claude_autocompact_pct, loaded.context_warn_tokens), (None, 150000))
            env.write_text(base + "CLAUDE_AUTOCOMPACT_PCT=70\nCONTEXT_WARN_TOKENS=120000\n", encoding="utf-8")
            loaded = config.load(env, {})
            self.assertEqual((loaded.claude_autocompact_pct, loaded.context_warn_tokens), (70, 120000))
            env.write_text(base + "CLAUDE_AUTOCOMPACT_PCT=150\n", encoding="utf-8")
            with self.assertRaises(ValueError):
                config.load(env, {})

    def test_claude_env_manages_the_override_only_when_set(self):
        self.assertNotIn("CLAUDE_AUTOCOMPACT_PCT_OVERRIDE", claude_patches.env(cfg()))
        managed = claude_patches.env(dataclasses.replace(cfg(), claude_autocompact_pct=70))
        self.assertEqual(managed["CLAUDE_AUTOCOMPACT_PCT_OVERRIDE"], "70")


if __name__ == "__main__":
    unittest.main()
