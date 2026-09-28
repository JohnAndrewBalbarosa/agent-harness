"""`harness <command>`: packaged operator commands, so agents and people do not rebuild them ad hoc.

  agents [--json]                           supported agents, CLI on PATH, subscribed instances
  preflight <agent> [--instance N] [--init] launch check (spec §4.5a); prints one line, exit code = verdict
Common options: --env-file PATH (default <HARNESS_HOME>/.env), --log-dir PATH (default <HARNESS_HOME>/var/logs)
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import sys
from pathlib import Path
from typing import Sequence

from core import registry

ROOT = Path(__file__).resolve().parents[1]


def _parser() -> argparse.ArgumentParser:
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--env-file", type=Path, default=ROOT / ".env")
    common.add_argument("--log-dir", type=Path, default=ROOT / "var" / "logs")
    parser = argparse.ArgumentParser(prog="harness")
    commands = parser.add_subparsers(dest="command", required=True)
    agents = commands.add_parser("agents", parents=[common])
    agents.add_argument("--json", action="store_true")
    preflight = commands.add_parser("preflight", parents=[common])
    preflight.add_argument("agent")
    preflight.add_argument("--instance")
    preflight.add_argument("--init", action="store_true")
    return parser


def _compile(cfg) -> None:
    from compile.run import compile_all
    compile_all(cfg, dry_run=False)


def main(argv: Sequence[str] | None = None) -> int:
    try:
        args = _parser().parse_args(list(sys.argv[1:] if argv is None else argv))
    except SystemExit as exit_:
        return int(exit_.code or 0)
    if args.command == "agents":
        rows = registry.agents(args.env_file, os.environ, shutil.which)
        if args.json:
            print(json.dumps(rows))
        else:
            for row in rows:
                print(f"{row['agent']:<10} cli={'yes' if row['binary'] else 'no ':<3} subscribed={','.join(row['instances']) or '-'}")
        return 0
    verdict = registry.preflight(args.agent, env_file=args.env_file, environ=os.environ, init=args.init,
                                 which=shutil.which, compile_instance=_compile, log_dir=args.log_dir,
                                 instance=args.instance)
    print(verdict.message)
    return verdict.code


if __name__ == "__main__":
    raise SystemExit(main())
