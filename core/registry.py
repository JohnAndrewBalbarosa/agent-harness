"""Agent registry and launch preflight (spec 2026-09-28 herdr-plugin §4.5a).

supported  = an adapter exists (adapters/<agent>/manifest.toml)
subscribed = listed in .env INSTANCES
preflight  = supported → installed → subscribed → configured → working, each step logged to
             var/logs/preflight.lifecycle.jsonl; the verdict is one printable line plus an exit code.
"""
from __future__ import annotations

import json
import os
import re
import tomllib
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Callable, Mapping

from adapters.base import load_adapter
from core import lifecycle
from core.config import Config, load

ROOT = Path(__file__).resolve().parents[1]
FAILURE_WINDOW = timedelta(hours=24)
CODES = {"ready": 0, "unsupported": 10, "not_installed": 11, "not_subscribed": 12, "not_configured": 13, "broken": 14, "misconfigured": 15}
_SAFE = re.compile(r"^[a-z][a-z0-9_-]*$")

Which = Callable[[str], "str | None"]
CompileInstance = Callable[[Config], object]


@dataclass(frozen=True)
class Verdict:
    agent: str
    status: str
    message: str

    @property
    def code(self) -> int:
        return CODES[self.status]


def manifests() -> dict[str, dict]:
    return {path.parent.name: tomllib.loads(path.read_text(encoding="utf-8"))
            for path in sorted((ROOT / "adapters").glob("*/manifest.toml"))}


def add_instance(env_text: str, agent: str, name: str, home: str) -> str:
    """.env text with `agent:name=home` appended to INSTANCES (created when missing); unchanged if already present."""
    entry = f"{agent}:{name}={home}"
    lines = env_text.splitlines()
    for index, line in enumerate(lines):
        if line.strip().startswith("INSTANCES="):
            value = line.split("=", 1)[1].strip().strip('"')
            if any(part.strip().split("=", 1)[0] == f"{agent}:{name}" for part in value.split(";")):
                return env_text
            lines[index] = f"INSTANCES={value};{entry}" if value else f"INSTANCES={entry}"
            return "\n".join(lines) + "\n"
    return env_text.rstrip("\n") + f"\nINSTANCES={entry}\n"


def agents(env_file: Path, environ: Mapping[str, str], which: Which) -> list[dict]:
    cfg = load(env_file, environ)
    return [{"agent": agent, "binary": which(str(m.get("binary", agent))),
             "instances": [i.name for i in cfg.instances if i.agent == agent]} for agent, m in manifests().items()]


def _configured(home: Path, manifest: Mapping, agent: str) -> bool:
    try:
        document = json.loads((home / str(manifest.get("hook_config", ""))).read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return False
    return any(_routes(command, agent) for command in _commands(document))


def _commands(value: object) -> list[str]:
    if isinstance(value, dict):
        own = [value["command"]] if isinstance(value.get("command"), str) else []
        return own + [c for v in value.values() for c in _commands(v)]
    if isinstance(value, list):
        return [c for v in value for c in _commands(v)]
    return []


def _routes(command: str, agent: str) -> bool:
    """`<path>\\harness-hook.cmd <agent>` (quoted or bare/8.3 path)."""
    _, found, rest = command.partition("harness-hook")
    tokens = rest.replace('"', " ").split()  # ['.cmd', '<agent>', ...]
    return bool(found) and len(tokens) >= 2 and tokens[1] == agent


def _broken_reason(agent: str, log_dir: Path, now: datetime) -> str | None:
    try:
        adapter = load_adapter(agent)
        for fixture in sorted((ROOT / "adapters" / agent / "fixtures").glob("*.json")):
            adapter.normalize(fixture.name.split(".")[0], json.loads(fixture.read_text(encoding="utf-8")), {})
    except Exception as error:  # the adapter itself cannot handle its own recorded payloads
        return f"adapter self-test failed: {type(error).__name__}: {error}"
    if not (ROOT / "core" / "router" / "harness-hook.cmd").exists():
        return "harness-hook.cmd missing from HARNESS_HOME"
    try:
        lines = (log_dir / "hook-router.lifecycle.jsonl").read_text(encoding="utf-8").splitlines()[-2000:]
    except OSError:
        return None
    for line in reversed(lines):
        try:
            record = json.loads(line)
            when = datetime.fromisoformat(str(record["timestamp"]).replace("Z", "+00:00"))
        except (ValueError, KeyError):
            continue
        details = record.get("details") or {}
        if now - when > FAILURE_WINDOW or details.get("agent") != agent:
            continue
        if record.get("outcome") == "failed" or details.get("failures"):
            reason = details.get("error") or ", ".join(f"{k}={v}" for k, v in (details.get("failures") or {}).items())
            return f"{record.get('operation')} failed at {record['timestamp']}: {reason}"
    return None


def preflight(agent: str, env_file: Path, environ: Mapping[str, str], init: bool, which: Which,
              compile_instance: CompileInstance, log_dir: Path, now: datetime | None = None,
              instance: str | None = None) -> Verdict:
    now = now or datetime.now(timezone.utc)
    log = log_dir / "preflight.lifecycle.jsonl"

    def step(name: str, outcome: str, **details: object) -> None:
        lifecycle.emit(log, "preflight", f"step.{name}", outcome, agent=agent, **details)

    def verdict(status: str, message: str) -> Verdict:
        lifecycle.emit(log, "preflight", "preflight.verdict", "succeeded" if status == "ready" else "failed",
                       agent=agent, status=status, message=message)
        return Verdict(agent, status, message)

    manifest = manifests().get(agent) if _SAFE.match(agent) else None
    if manifest is None:
        step("supported", "failed")
        return verdict("unsupported", f"agent-harness: {agent} is not supported (no adapter); running without the harness")
    step("supported", "succeeded")

    binary = which(str(manifest.get("binary", agent)))
    if not binary:
        step("installed", "failed", binary=manifest.get("binary", agent))
        return verdict("not_installed", f"agent-harness: {agent} is supported but its CLI is not on PATH")
    step("installed", "succeeded", binary_path=binary)

    cfg = load(env_file, environ)
    chosen = [i for i in cfg.instances if i.agent == agent and (instance is None or i.name == instance)]
    if not chosen:
        if not init:
            step("subscribed", "failed")
            return verdict("not_subscribed", f"agent-harness: {agent} is supported but not subscribed (run with --init)")
        home = os.path.expanduser(str(manifest.get("default_home", f"~/.{agent}")))
        env_file.write_text(add_instance(env_file.read_text(encoding="utf-8"), agent, instance or "default", home), encoding="utf-8")
        cfg = load(env_file, environ)
        chosen = [i for i in cfg.instances if i.agent == agent]
        step("subscribed", "initialized", home=home)
    else:
        step("subscribed", "succeeded", instances=[i.name for i in chosen])

    unconfigured = [i for i in chosen if not _configured(i.home, manifest, agent)]
    if unconfigured:
        if not init:
            step("configured", "failed", instances=[i.name for i in unconfigured])
            return verdict("not_configured", f"agent-harness: {agent} hooks are not installed (run with --init)")
        compile_instance(cfg)
        still = [i.name for i in chosen if not _configured(i.home, manifest, agent)]
        if still:
            step("configured", "failed", instances=still, after="compile")
            return verdict("broken", f"agent-harness: {agent} compile ran but hooks are still missing for {', '.join(still)}")
        step("configured", "initialized")
    else:
        step("configured", "succeeded")

    reason = _broken_reason(agent, log_dir, now)
    if reason:
        step("working", "failed", reason=reason)
        return verdict("broken", f"agent-harness: {agent} is set up but not working: {reason}")
    step("working", "succeeded")
    return verdict("ready", f"agent-harness: {agent} ready ({', '.join(i.name for i in chosen)})")
