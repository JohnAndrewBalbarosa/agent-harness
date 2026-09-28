"""`harness compile`: render one source into every connected agent's native files (spec §8, R7).

Safety model: rulesync never runs against real homes. Target files are copied into a temporary simulation, rulesync
and the adapter patches run there, and only the resulting diffs are applied to real files, each backed up first
(<HARNESS_HOME>/var/backups/compile-<ts>/). Existing files that do not parse abort the run before anything is written.
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import tomllib
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path

from adapters.claude import patches as claude_patches
from adapters.codex import patches as codex_patches
from compile.patch import merge_block, merge_codex_hooks, merge_env
from compile.render import SKILL_NAME, hooks_source, rulesync_config, skill
from core.config import Config, Instance, load

ROOT = Path(__file__).resolve().parents[1]
LOCK_FILE = ".rulesync-hooks-lock.json"


class CompileError(Exception):
    pass


@dataclass(frozen=True)
class Write:
    path: Path
    before: str | None
    after: str


@dataclass(frozen=True)
class Link:
    path: Path
    target: Path


def _read(path: Path) -> str | None:
    return path.read_text(encoding="utf-8") if path.exists() else None


def _manifests(cfg: Config) -> dict[str, dict]:
    return {agent: tomllib.loads((ROOT / "adapters" / agent / "manifest.toml").read_text(encoding="utf-8"))
            for agent in dict.fromkeys(i.agent for i in cfg.instances)}


def _validate(cfg: Config) -> None:
    checks = [(i.home / name, parser) for i in cfg.instances for name, parser in
              ({"claude": [("settings.json", json.loads)], "codex": [("hooks.json", json.loads), ("config.toml", tomllib.loads)]}
               .get(i.agent, []))]
    for path, parser in checks:
        text = _read(path)
        if text is None:
            continue
        try:
            parser(text)
        except (ValueError, tomllib.TOMLDecodeError) as error:
            raise CompileError(f"{path} does not parse ({type(error).__name__}); fix it before compiling") from None


def _rulesync(project: Path, home: Path, target: str, version: str) -> None:
    npx = shutil.which("npx")
    if not npx:
        raise CompileError("npx (Node.js) is required for rulesync")
    env = {**os.environ, "HOME": str(home), "USERPROFILE": str(home)}
    done = subprocess.run([npx, "-y", f"rulesync@{version}", "generate", "--global", "--targets", target, "--features", "hooks"],
                          cwd=str(project), env=env, capture_output=True, timeout=300)
    if done.returncode != 0:
        tail = (done.stderr or done.stdout).decode("utf-8", errors="replace")[-800:]
        raise CompileError(f"rulesync failed for {target}: {tail}")


def _project(sim: Path, cfg: Config, manifests: dict) -> Path:
    project = sim / "project"
    (project / ".rulesync").mkdir(parents=True)
    (project / ".rulesync" / "hooks.jsonc").write_text(json.dumps(hooks_source(cfg, manifests), indent=2), encoding="utf-8")
    targets = [m["rulesync_target"] for m in manifests.values()]
    (project / "rulesync.jsonc").write_text(json.dumps(rulesync_config(targets), indent=2), encoding="utf-8")
    return project


def _state_path(cfg: Config) -> Path:
    return cfg.harness_home / "var" / "compile" / "state.json"


def _without_block(text: str, begin: str, end: str) -> str:
    start = text.find(begin)
    stop = text.find(end, start) if start != -1 else -1
    return text if start == -1 or stop == -1 else text[:start] + text[stop + len(end):]


def _changed(path: Path, after: str) -> list[Write]:
    before = _read(path)
    return [] if before == after else [Write(path, before, after)]


def _lock_path(cfg: Config, instance: Instance) -> Path:
    """rulesync's hook-ownership lock, kept out of the agent home."""
    return cfg.harness_home / "var" / "compile" / f"{instance.agent}-{instance.name}{LOCK_FILE}"


def _skill(cfg: Config, instance: Instance) -> list[Write]:
    return _changed(instance.home / "skills" / SKILL_NAME / "SKILL.md", skill(cfg))


def _plan_claude(sim: Path, project: Path, cfg: Config, instance: Instance, state: dict) -> list[Write]:
    sim_home = sim / f"claude-{instance.name}"
    (sim_home / ".claude").mkdir(parents=True)
    if (instance.home / "settings.json").exists():
        shutil.copy2(instance.home / "settings.json", sim_home / ".claude" / "settings.json")
    if _lock_path(cfg, instance).exists():
        shutil.copy2(_lock_path(cfg, instance), sim_home / ".claude" / LOCK_FILE)
    _rulesync(project, sim_home, "claudecode", cfg.rulesync_version)
    generated = json.loads((sim_home / ".claude" / "settings.json").read_text(encoding="utf-8"))
    previous = set(state.get("claudeManagedEnv", {}).get(instance.name, []))
    settings = merge_env(generated, claude_patches.env(cfg), previous)
    claude_md = merge_block(_read(instance.home / "CLAUDE.md") or "", claude_patches.policy_import(cfg),
                            claude_patches.POLICY_BEGIN, claude_patches.POLICY_END)
    writes = _changed(instance.home / "settings.json", json.dumps(settings, indent=2, ensure_ascii=False) + "\n")
    lock = _read(sim_home / ".claude" / LOCK_FILE)
    if lock is not None:
        writes += _changed(_lock_path(cfg, instance), lock)
    return writes + _changed(instance.home / "CLAUDE.md", claude_md) + _skill(cfg, instance)


def codex_config(config_text: str, cfg: Config) -> str:
    """config.toml with the managed OTel exporter.

    An [otel] outside the managed block is accepted unchanged when it equals the managed one: configs generated from
    a canonical source (e.g. a shared common.toml re-serialized by a sync tool) cannot keep the marker comments.
    """
    block = codex_patches.otel_block(cfg)
    unmanaged = tomllib.loads(_without_block(config_text, codex_patches.OTEL_BEGIN, codex_patches.OTEL_END)).get("otel")
    if unmanaged is not None:
        if unmanaged == tomllib.loads(block)["otel"]:
            return config_text
        raise CompileError("already has an unmanaged [otel] table that differs from the managed one; "
                           "remove it or move it into the managed block")
    new_config = merge_block(config_text, block, codex_patches.OTEL_BEGIN, codex_patches.OTEL_END)
    tomllib.loads(new_config)
    return new_config


def _plan_codex(sim: Path, project: Path, cfg: Config, instances: list[Instance]) -> list[Write | Link]:
    stage = sim / "codex-stage"
    (stage / ".codex").mkdir(parents=True)
    _rulesync(project, stage, "codexcli", cfg.rulesync_version)
    generated = json.loads((stage / ".codex" / "hooks.json").read_text(encoding="utf-8"))
    policy = cfg.harness_home / "policy" / "AGENTS.md"
    actions: list[Write | Link] = []
    for instance in instances:
        hooks = merge_codex_hooks(json.loads(_read(instance.home / "hooks.json") or "{}"), generated)
        actions += _changed(instance.home / "hooks.json", json.dumps(hooks, indent=2, ensure_ascii=False) + "\n")
        try:
            new_config = codex_config(_read(instance.home / "config.toml") or "", cfg)
        except CompileError as error:
            raise CompileError(f"{instance.home / 'config.toml'}: {error}") from None
        actions += _changed(instance.home / "config.toml", new_config) + _skill(cfg, instance)
        target = instance.home / "AGENTS.md"
        if not (target.exists() and policy.exists() and os.path.samefile(target, policy)):
            actions.append(Link(target, policy))
    return actions


def plan(cfg: Config) -> list[Write | Link]:
    _validate(cfg)
    manifests = _manifests(cfg)
    state = json.loads(_read(_state_path(cfg)) or "{}")
    with tempfile.TemporaryDirectory(prefix="agent-harness-compile-") as tmp:
        sim = Path(tmp)
        project = _project(sim, cfg, manifests)
        actions: list[Write | Link] = []
        for instance in (i for i in cfg.instances if i.agent == "claude"):
            actions += _plan_claude(sim, project, cfg, instance, state)
        codex = [i for i in cfg.instances if i.agent == "codex"]
        if codex:
            actions += _plan_codex(sim, project, cfg, codex)
        return dedupe(actions)


def dedupe(actions: list[Write | Link]) -> list[Write | Link]:
    """One write per file: several agent homes may hard-link the same file (e.g. a shared Codex hooks.json)."""
    kept: list[Write | Link] = []
    for action in actions:
        twin = next((k for k in kept if isinstance(k, Write) and isinstance(action, Write) and action.path.exists()
                     and k.path.exists() and os.path.samefile(k.path, action.path)), None)
        if twin is None:
            kept.append(action)
        elif twin.after != action.after:
            raise CompileError(f"{twin.path} and {action.path} are the same file but would get different content")
    return kept


def _backup(path: Path, backup_root: Path) -> None:
    if path.exists():
        destination = backup_root / path.drive.replace(":", "") / path.relative_to(path.anchor)
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(path, destination)


def apply(cfg: Config, actions: list[Write | Link]) -> None:
    backup_root = cfg.harness_home / "var" / "backups" / f"compile-{datetime.now():%Y%m%dT%H%M%S}"
    # A running agent may rewrite its own settings between plan and apply; never clobber that edit.
    stale = [str(a.path) for a in actions if isinstance(a, Write) and _read(a.path) != a.before]
    if stale:
        raise CompileError(f"changed since the compile was planned, nothing written; rerun: {', '.join(stale)}")
    for action in actions:
        _backup(action.path, backup_root)
        action.path.parent.mkdir(parents=True, exist_ok=True)
        if isinstance(action, Write) and action.path.exists() and os.stat(action.path).st_nlink > 1:
            with action.path.open("w", encoding="utf-8") as stream:  # in place: keep the hard-link group
                stream.write(action.after)
        elif isinstance(action, Write):
            temporary = action.path.with_name(action.path.name + ".agent-harness.tmp")
            temporary.write_text(action.after, encoding="utf-8")
            os.replace(temporary, action.path)
        else:
            action.path.unlink(missing_ok=True)
            try:
                os.link(action.target, action.path)
            except OSError:
                shutil.copy2(action.target, action.path)
    state_path = _state_path(cfg)
    state_path.parent.mkdir(parents=True, exist_ok=True)
    state_path.write_text(json.dumps({"claudeManagedEnv": {i.name: sorted(claude_patches.env(cfg))
                                                           for i in cfg.instances if i.agent == "claude"}}), encoding="utf-8")


def compile_all(cfg: Config, dry_run: bool) -> list[Write | Link]:
    actions = plan(cfg)
    if actions and not dry_run:
        apply(cfg, actions)
    return actions


def main(argv: list[str] | None = None) -> int:
    args = list(sys.argv[1:] if argv is None else argv)
    env_file = Path(args[args.index("--env-file") + 1]) if "--env-file" in args else Path.home() / ".agent-harness" / ".env"
    try:
        actions = compile_all(load(env_file, os.environ), dry_run="--dry-run" in args)
    except (CompileError, ValueError, OSError) as error:
        print(f"compile failed: {error}", file=sys.stderr)
        return 1
    for action in actions:
        print(f"{'=' if isinstance(action, Link) else ('+' if action.before is None else '~')} {action.path}")
    print(f"{len(actions)} change(s){' (dry run, nothing written)' if '--dry-run' in args else ''}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
