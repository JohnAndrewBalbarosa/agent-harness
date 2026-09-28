"""Checks one adapter against the adapter contract (spec §7). Returns human-readable violations; [] means conformant."""
from __future__ import annotations

import json
import tomllib
from pathlib import Path

from core.events import EVENT_TYPES, from_json, to_json

REQUIRED_MANIFEST_KEYS = ("agent", "events", "rulesync_target", "tested_versions")
PROBE_EVENTS = ("FutureEvent", None)


def _manifest(adapter_dir: Path, violations: list[str]) -> dict:
    path = adapter_dir / "manifest.toml"
    try:
        manifest = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        violations.append(f"manifest unreadable: {path} ({type(error).__name__})")
        return {}
    missing = [key for key in REQUIRED_MANIFEST_KEYS if key not in manifest]
    if missing:
        violations.append(f"manifest missing keys: {missing}")
    return manifest


def _check_fixture(name: str, adapter, path: Path, violations: list[str]) -> None:
    payload = json.loads(path.read_text(encoding="utf-8"))
    native = payload.get("hook_event_name")
    try:
        event = adapter.normalize(native, payload)
    except Exception as error:  # the contract forbids raising
        violations.append(f"{path.name}: normalize raised {type(error).__name__}")
        return
    if event is None:
        return
    if event.event not in EVENT_TYPES:
        violations.append(f"{path.name}: unknown common event {event.event!r}")
    if event.agent != name:
        violations.append(f"{path.name}: event agent {event.agent!r} != adapter {name!r}")
    if from_json(to_json(event)) != event:
        violations.append(f"{path.name}: event does not round-trip through JSON")
    try:
        if not isinstance(adapter.respond(event, []), str):
            violations.append(f"{path.name}: respond did not return str")
    except Exception as error:
        violations.append(f"{path.name}: respond raised {type(error).__name__}")


def conformance_violations(name: str, adapter, adapter_dir: Path) -> list[str]:
    violations: list[str] = []
    if getattr(adapter, "name", None) != name:
        violations.append(f"adapter.name {getattr(adapter, 'name', None)!r} != {name!r}")
    manifest = _manifest(adapter_dir, violations)
    for native in list(manifest.get("events", [])) + list(PROBE_EVENTS):
        if not isinstance(adapter.safe_default(native), str):
            violations.append(f"safe_default({native!r}) did not return str")
    try:
        if adapter.normalize("FutureEvent", {"hook_event_name": "FutureEvent", "session_id": "s"}) is not None:
            violations.append("unknown native events must normalize to None")
    except Exception as error:
        violations.append(f"normalize(FutureEvent) raised {type(error).__name__}")
    for path in sorted((adapter_dir / "fixtures").glob("*.json")):
        _check_fixture(name, adapter, path, violations)
    return violations
