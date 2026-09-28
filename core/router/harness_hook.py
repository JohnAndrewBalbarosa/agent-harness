"""`harness-hook <agent> [<native-event>]`: the single entry point for every agent hook (spec §4, R1).

Reads the native payload from stdin, normalizes it through the agent's adapter, fans the common event out to
consumers in parallel (hidden windows, per-consumer timeout) and prints the adapter's native response.
It never fails its agent: every error path prints the adapter's safe default and exits 0 (R3).
"""
from __future__ import annotations

import concurrent.futures
import json
import os
import subprocess
import sys
import time
import tomllib
from pathlib import Path
from typing import Any, Callable, Mapping, Sequence

from adapters.base import ConsumerResult, load_adapter
from core import lifecycle
from core.config import runtime_env
from core.events import Event, to_json
from core.router.consumers import ROOT, Consumer, consumers_for

MAX_CONSUMER_STDOUT = 65536
ENV_FILE = ROOT / ".env"
Registry = Callable[[Event, Mapping[str, str]], Sequence[Consumer]]


def _log_path(env: Mapping[str, str]) -> Path:
    return Path(env.get("AH_LOG_DIR") or ROOT / "var" / "logs") / "hook-router.lifecycle.jsonl"


def _payload(stdin: bytes) -> dict[str, Any] | None:
    try:
        value = json.loads(stdin.decode("utf-8") or "{}")
    except (UnicodeDecodeError, json.JSONDecodeError):
        return None
    return value if isinstance(value, dict) else None


def _child_env(env: Mapping[str, str], event: Event) -> dict[str, str]:
    child = {**os.environ, **env}
    child.update(AH_AGENT=event.agent, AH_INSTANCE=event.instance, AH_EVENT=event.event, PYTHONUTF8="1",
                 PYTHONPATH=os.pathsep.join(filter(None, [str(ROOT), child.get("PYTHONPATH", "")])))
    return child


def _invoke(consumer: Consumer, stdin: bytes, env: dict[str, str], log: Path) -> ConsumerResult:
    started = time.perf_counter()
    flags = int(getattr(subprocess, "CREATE_NO_WINDOW", 0))
    try:
        process = subprocess.Popen(list(consumer.command), stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=subprocess.DEVNULL, env=env, cwd=str(ROOT), creationflags=flags)
    except OSError as error:
        lifecycle.emit(log, "router", "consumer.launch_failed", "failed", consumer=consumer.name, error=type(error).__name__)
        return ConsumerResult(consumer.name, 127, b"")
    try:
        stdout, _ = process.communicate(input=stdin, timeout=consumer.timeout_s)
    except subprocess.TimeoutExpired:
        process.kill()
        process.communicate()
        lifecycle.emit(log, "router", "consumer.timeout", "failed", consumer=consumer.name, timeoutS=consumer.timeout_s)
        return ConsumerResult(consumer.name, 124, b"")
    lifecycle.emit(log, "router", "consumer.exited", "succeeded" if process.returncode == 0 else "failed",
                   consumer=consumer.name, exitCode=process.returncode,
                   durationMs=round((time.perf_counter() - started) * 1000, 1))
    return ConsumerResult(consumer.name, process.returncode, stdout[:MAX_CONSUMER_STDOUT])


def _fan_out(consumers: Sequence[Consumer], event: Event, native: bytes, env: Mapping[str, str], log: Path) -> list[ConsumerResult]:
    if not consumers:
        return []
    event_bytes = to_json(event).encode("utf-8")
    child_env = _child_env(env, event)
    with concurrent.futures.ThreadPoolExecutor(max_workers=len(consumers)) as pool:
        futures = [pool.submit(_invoke, c, event_bytes if c.stdin == "event" else native, child_env, log) for c in consumers]
        return [future.result() for future in futures]


def _check_version(agent: str, payload: Mapping[str, Any], log: Path) -> None:
    version = payload.get("version")
    if not version:
        return
    try:
        manifest = tomllib.loads((ROOT / "adapters" / agent / "manifest.toml").read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError):
        return
    if str(version) not in manifest.get("tested_versions", []):
        lifecycle.emit(log, "router", "agent.version.untested", "observed", agent=agent, version=str(version))


def run(agent: str, native_event: str | None, stdin: bytes, env: Mapping[str, str],
        registry: Registry = consumers_for) -> tuple[str, int]:
    started = time.perf_counter()
    log = _log_path(env)
    try:
        adapter = load_adapter(agent)
    except KeyError:
        lifecycle.emit(log, "router", "hook.agent.unknown", "failed", agent=agent)
        return "", 0
    payload = _payload(stdin)
    if payload is None:
        lifecycle.emit(log, "router", "hook.payload.invalid", "failed", agent=agent, nativeEvent=native_event)
        return adapter.safe_default(native_event), 0
    native = native_event or str(payload.get("hook_event_name") or "") or None
    try:
        event = adapter.normalize(native, payload, env)
        if event is None:
            return adapter.safe_default(native), 0
        if event.event == "session.start":
            _check_version(agent, payload, log)
        subscribed = [consumer for consumer in registry(event, env) if event.event in consumer.events]
        results = _fan_out(subscribed, event, stdin, env, log)
        lifecycle.emit(log, "router", "hook.dispatch", "succeeded", agent=agent, instance=event.instance,
                       event=event.event, nativeEvent=native, consumers=[r.name for r in results],
                       failures={r.name: r.exit_code for r in results if r.exit_code != 0},
                       durationMs=round((time.perf_counter() - started) * 1000, 1))
        return adapter.respond(event, results), 0
    except Exception as error:  # fail open (R3)
        lifecycle.emit(log, "router", "hook.dispatch", "failed", agent=agent, nativeEvent=native, error=type(error).__name__)
        return adapter.safe_default(native), 0


def main(argv: Sequence[str] | None = None) -> int:
    args = list(sys.argv[1:] if argv is None else argv)
    if not args:
        return 0
    stdin = b"" if sys.stdin is None or sys.stdin.isatty() else sys.stdin.buffer.read()
    output, code = run(args[0], args[1] if len(args) > 1 else None, stdin, runtime_env(ENV_FILE, os.environ))
    if output:
        sys.stdout.buffer.write(output.encode("utf-8"))
        sys.stdout.buffer.flush()
    return code


if __name__ == "__main__":
    raise SystemExit(main())
