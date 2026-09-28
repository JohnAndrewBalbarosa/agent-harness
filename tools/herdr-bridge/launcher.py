from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(prog="herdr-codex")
    parser.add_argument("-Profile", "--profile", dest="profile", choices=("personal", "cy", "feu"), required=True)
    parser.add_argument("-Project", "--project", dest="project", required=True)
    parser.add_argument("-Label", "--label", dest="label", default="")
    parser.add_argument("-DryRun", "--dry-run", dest="dry_run", action="store_true")
    parser.add_argument("codex_arguments", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    if args.codex_arguments[:1] == ["--"]:
        args.codex_arguments = args.codex_arguments[1:]
    return args


def powershell_command(args: argparse.Namespace) -> list[str]:
    command = [
        "powershell.exe",
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        str(Path(__file__).with_name("launch.ps1")),
        "-Profile",
        args.profile,
        "-Project",
        args.project,
    ]
    if args.label:
        command.extend(("-Label", args.label))
    if args.dry_run:
        command.append("-DryRun")
    return command


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    result = subprocess.run(
        powershell_command(args),
        input=json.dumps(args.codex_arguments, ensure_ascii=False),
        text=True,
        check=False,
    )
    return result.returncode


if __name__ == "__main__":
    raise SystemExit(main())
