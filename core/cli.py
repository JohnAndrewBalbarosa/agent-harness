"""`harness <command>`: packaged operator commands, so agents and people do not rebuild them ad hoc.

  agents [--json]                           supported agents, CLI on PATH, subscribed instances
  preflight <agent> [--instance N] [--init] launch check (spec §4.5a); prints one line, exit code = verdict
  usage [--json]                            deterministic token report (Claude transcripts, Codex rollouts)
  logs [<component>] [--tail N]             bounded lifecycle-log summary: outcome counts + last failures
  image <file> [--max-edge 1024]            downscaled copy of a screenshot to read instead (prints its path)
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
    usage = commands.add_parser("usage", parents=[common])
    usage.add_argument("--json", action="store_true")
    logs = commands.add_parser("logs", parents=[common])
    logs.add_argument("component", nargs="?")
    logs.add_argument("--tail", type=int, default=500)
    shrink = commands.add_parser("image", parents=[common])
    shrink.add_argument("file", type=Path)
    shrink.add_argument("--max-edge", type=int, default=1024)
    shrink.add_argument("--cache-dir", type=Path, default=ROOT / "var" / "images")
    return parser


def _compile(cfg) -> None:
    from compile.run import compile_all
    compile_all(cfg, dry_run=False)


def main(argv: Sequence[str] | None = None) -> int:
    try:
        args = _parser().parse_args(list(sys.argv[1:] if argv is None else argv))
    except SystemExit as exit_:
        return int(exit_.code or 0)
    try:
        return _run(args)
    except (ValueError, OSError) as error:  # unreadable or invalid .env: say so plainly
        print(f"agent-harness: configuration error in {args.env_file}: {error}")
        return registry.CODES["misconfigured"]


def _usage(args: argparse.Namespace) -> int:
    from core.config import load
    from core.usage import report
    cfg = load(args.env_file, os.environ)
    claude_homes = [i.home for i in cfg.instances if i.agent == "claude"] or [Path.home() / ".claude"]
    data = {"claude": report.claude(claude_homes[0] / "projects"),
            "codex": report.codex({i.name: i.home for i in cfg.instances if i.agent == "codex"})}
    print(json.dumps(data) if args.json else report.render(data))
    return 0


def _logs(args: argparse.Namespace) -> int:
    if not args.component:
        for path in sorted(args.log_dir.glob("*.lifecycle.jsonl")):
            print(path.name.removesuffix(".lifecycle.jsonl"))
        return 0
    path = args.log_dir / f"{args.component}.lifecycle.jsonl"
    try:
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()[-max(1, args.tail):]
    except OSError:
        print(f"no log named {args.component!r} in {args.log_dir}")
        return 1
    counts: dict[tuple[str, str], int] = {}
    failures: list[str] = []
    for line in lines:
        try:
            record = json.loads(line)
        except ValueError:
            continue
        key = (str(record.get("operation")), str(record.get("outcome")))
        counts[key] = counts.get(key, 0) + 1
        if record.get("outcome") == "failed":
            failures.append(f"  {record.get('timestamp')} {key[0]} {json.dumps(record.get('details'))[:200]}")
    print(f"{args.component}: last {len(lines)} records")
    for (operation, outcome), number in sorted(counts.items(), key=lambda kv: -kv[1])[:10]:
        print(f"  {operation} {outcome} {number}")
    if failures:
        print("last failures:")
        for failure in failures[-5:]:
            print(failure)
    return 0


def _run(args: argparse.Namespace) -> int:
    if args.command == "image":
        from core import image
        print(image.downscale(args.file, args.max_edge, args.cache_dir))
        return 0
    if args.command == "usage":
        return _usage(args)
    if args.command == "logs":
        return _logs(args)
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
