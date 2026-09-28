from __future__ import annotations

import hashlib
import json
import os
import sys
import time
import tomllib
from pathlib import Path
from typing import Any


SHARED_ROOT = Path(__file__).resolve().parents[2]
TOOL_ROOT = Path(__file__).resolve().parent
DATA_ROOT = SHARED_ROOT / "var" / "herdr-bridge"
SESSION_ROOT = DATA_ROOT / "sessions"
LOG_PATH = SHARED_ROOT / "var" / "logs" / "herdr-bridge.lifecycle.jsonl"
OBS_ROOT = SHARED_ROOT / "tools" / "observability-hub"


def ensure_dirs() -> None:
    SESSION_ROOT.mkdir(parents=True, exist_ok=True)
    LOG_PATH.parent.mkdir(parents=True, exist_ok=True)


def safe_id(value: Any, fallback: str = "unknown") -> str:
    raw = str(value or fallback).strip()
    allowed = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_.:"
    cleaned = "".join(ch if ch in allowed else "_" for ch in raw)
    return cleaned[:240] or fallback


def now_iso() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def log_event(event: str, outcome: str = "succeeded", **fields: Any) -> None:
    ensure_dirs()
    record = {
        "timestamp": now_iso(),
        "severity": "error" if outcome == "failed" else "info",
        "component": "herdr-bridge",
        "event": event,
        "outcome": outcome,
        **fields,
    }
    line = json.dumps(record, ensure_ascii=True, separators=(",", ":")) + "\n"
    try:
        if LOG_PATH.exists() and LOG_PATH.stat().st_size > 5 * 1024 * 1024:
            rotated = LOG_PATH.with_suffix(".jsonl.1")
            rotated.unlink(missing_ok=True)
            os.replace(LOG_PATH, rotated)
        with LOG_PATH.open("a", encoding="utf-8") as handle:
            handle.write(line)
    except OSError:
        pass


def atomic_json(path: Path, value: dict[str, Any]) -> None:
    ensure_dirs()
    temp = path.with_suffix(path.suffix + f".{os.getpid()}.tmp")
    temp.write_text(json.dumps(value, ensure_ascii=True, indent=2), encoding="utf-8")
    os.replace(temp, path)


def read_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
        return value if isinstance(value, dict) else {}
    except (OSError, json.JSONDecodeError):
        return {}


def profile_name() -> str:
    explicit = os.environ.get("CODEX_SHARED_PROFILE", "").strip().lower()
    if explicit in {"personal", "cy", "feu"}:
        return explicit
    home = os.environ.get("CODEX_HOME", "").lower().replace("/", "\\")
    if home.endswith("\\.codex-cy"):
        return "cy"
    if home.endswith("\\.codex-feu"):
        return "feu"
    return "personal"


def project_record(cwd: str | Path) -> tuple[Path, dict[str, Any]] | None:
    start = Path(cwd or os.getcwd()).resolve()
    for candidate in (start, *start.parents):
        marker = candidate / "observability.project.toml"
        if marker.is_file():
            try:
                data = tomllib.loads(marker.read_text(encoding="utf-8"))
            except (OSError, tomllib.TOMLDecodeError):
                return None
            project = data.get("project") if isinstance(data, dict) else None
            if isinstance(project, dict) and project.get("id"):
                return marker, project
            return None
    return None


def herdr_binary() -> str:
    inherited = os.environ.get("HERDR_BIN_PATH", "").strip()
    if inherited and Path(inherited).is_file():
        return inherited
    local = Path(os.environ.get("LOCALAPPDATA", "")) / "Programs" / "Herdr" / "bin" / "herdr.exe"
    return str(local if local.is_file() else "herdr")


def content_hash(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8", errors="replace")).hexdigest()


def load_observability_module():
    import importlib.util

    module_path = OBS_ROOT / "obs.py"
    spec = importlib.util.spec_from_file_location("shared_observability_cli", module_path)
    if spec is None or spec.loader is None:
        raise RuntimeError("observability module loader unavailable")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module

