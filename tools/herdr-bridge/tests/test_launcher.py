from __future__ import annotations

import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from launcher import parse_args, powershell_command


class LauncherTests(unittest.TestCase):
    def test_codex_arguments_remain_after_boundary(self) -> None:
        args = parse_args(["-Profile", "cy", "-Project", r"C:\work tree", "--", "--profile", "review", "a b", "quote'case"])
        self.assertEqual(args.codex_arguments, ["--profile", "review", "a b", "quote'case"])

    def test_powershell_command_excludes_codex_arguments(self) -> None:
        args = parse_args(["-Profile", "feu", "-Project", r"C:\work", "--", "prompt", "secret-like-value"])
        command = powershell_command(args)
        self.assertNotIn("prompt", command)
        self.assertNotIn("secret-like-value", command)
        self.assertEqual(command[-4:], ["-Profile", "feu", "-Project", r"C:\work"])


if __name__ == "__main__":
    unittest.main()
