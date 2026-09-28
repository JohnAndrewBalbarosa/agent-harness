""".env-driven configuration (spec §5). The repo ships `.env.example`; each machine owns `<HARNESS_HOME>/.env`.

Values may reference Windows environment variables as %NAME%. Process environment variables override the file.
"""
from __future__ import annotations

import os
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Literal, Mapping

REQUIRED = ("HARNESS_HOME", "INSTANCES")
DEFAULTS = {
    "HARNESS_PYTHON": r"%APPDATA%\uv\python\cpython-3.14.7-windows-x86_64-none\python.exe",
    "OBS_ENDPOINT": "http://127.0.0.1:4319",
    "OTLP_PORT": "4320",
    "NOTIFY_RATE_LIMIT": "5/60",
    "RULESYNC_VERSION": "22.0.0",
    "HERDR_BIN": "",
}
KNOWN_KEYS = REQUIRED + tuple(DEFAULTS)
_PERCENT_VAR = re.compile(r"%([A-Za-z_][A-Za-z0-9_]*)%")
_INSTANCE = re.compile(r"^\s*([a-z][a-z0-9_]*):([A-Za-z0-9_-]+)=(.+?)\s*$")


@dataclass(frozen=True)
class Instance:
    agent: str
    name: str
    home: Path


@dataclass(frozen=True)
class Config:
    harness_home: Path
    python: Path
    instances: tuple[Instance, ...]
    obs_endpoint: str
    otlp_port: int
    notify_rate: tuple[int, int]
    herdr_bin: Path | None
    rulesync_version: str


def parse_env_text(text: str) -> dict[str, str]:
    values: dict[str, str] = {}
    for raw in text.splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        value = value.strip()
        if len(value) >= 2 and value[0] == value[-1] and value[0] in "\"'":
            value = value[1:-1]
        values[key.strip()] = value
    return values


def expand(value: str, environ: Mapping[str, str]) -> str:
    return _PERCENT_VAR.sub(lambda m: environ.get(m.group(1), m.group(0)), value)


def _instances(value: str) -> tuple[Instance, ...]:
    found = []
    for entry in filter(None, (part.strip() for part in value.split(";"))):
        match = _INSTANCE.match(entry)
        if not match:
            raise ValueError(f"INSTANCES entry must look like agent:name=home, got {entry!r}")
        found.append(Instance(match.group(1), match.group(2), Path(match.group(3))))
    if not found:
        raise ValueError("INSTANCES must list at least one agent:name=home entry")
    return tuple(found)


def _int(key: str, value: str) -> int:
    try:
        return int(value)
    except ValueError:
        raise ValueError(f"{key} must be an integer, got {value!r}") from None


def load(env_file: Path, environ: Mapping[str, str]) -> Config:
    file_values = parse_env_text(env_file.read_text(encoding="utf-8")) if env_file.exists() else {}
    merged = {**DEFAULTS, **file_values, **{k: environ[k] for k in KNOWN_KEYS if environ.get(k)}}
    missing = [key for key in REQUIRED if not merged.get(key)]
    if missing:
        raise ValueError(f"missing required configuration: {', '.join(missing)}")
    values = {key: expand(value, environ) for key, value in merged.items()}
    limit, _, window = values["NOTIFY_RATE_LIMIT"].partition("/")
    return Config(
        harness_home=Path(values["HARNESS_HOME"]), python=Path(values["HARNESS_PYTHON"]),
        instances=_instances(values["INSTANCES"]), obs_endpoint=values["OBS_ENDPOINT"],
        otlp_port=_int("OTLP_PORT", values["OTLP_PORT"]),
        notify_rate=(_int("NOTIFY_RATE_LIMIT", limit), _int("NOTIFY_RATE_LIMIT", window or "60")),
        herdr_bin=Path(values["HERDR_BIN"]) if values["HERDR_BIN"] else None,
        rulesync_version=values["RULESYNC_VERSION"],
    )


RUNTIME_KEYS = ("OBS_ENDPOINT", "OTLP_PORT", "NOTIFY_RATE_LIMIT", "HERDR_BIN", "HARNESS_PYTHON", "HUB_DIR")


def runtime_env(env_file: Path, environ: Mapping[str, str]) -> dict[str, str]:
    """Environment for hook consumers and services: `environ` plus the runtime keys from `<HARNESS_HOME>/.env`.

    Process variables win over the file. HERDR_BIN is also exported as HERDR_BIN_PATH, the name herdr-bridge reads.
    """
    try:
        file_values = parse_env_text(env_file.read_text(encoding="utf-8"))
    except OSError:
        file_values = {}
    merged = {**{k: expand(v, environ) for k, v in file_values.items() if k in RUNTIME_KEYS and v}, **environ}
    if merged.get("HERDR_BIN") and not merged.get("HERDR_BIN_PATH"):
        merged["HERDR_BIN_PATH"] = merged["HERDR_BIN"]
    return merged


def _short_path(path: Path) -> str | None:
    """Windows 8.3 short name (no spaces) for an existing path, or None when unavailable."""
    if os.name != "nt":
        return None
    import ctypes
    buffer = ctypes.create_unicode_buffer(32768)
    length = ctypes.windll.kernel32.GetShortPathNameW(str(path), buffer, len(buffer))
    return buffer.value if 0 < length < len(buffer) else None


def hook_command(cfg: Config, agent: str, native_event: str | None = None,
                 style: Literal["quoted", "bare"] = "quoted") -> str:
    """Hook command line for an agent's config.

    `quoted` (Claude Code, Git Bash runner): the script path is quoted, so profiles with spaces work.
    `bare` (Codex CLI): Codex's Windows command runner rejects an executable that starts with a quote, so the path
    is emitted unquoted, using its 8.3 short name when it contains spaces; if none exists, refuse loudly.
    """
    script = cfg.harness_home / "core" / "router" / "harness-hook.cmd"
    if style == "bare":
        executable = str(script)
        if " " in executable:
            # Shorten only the directory: compile finds harness-owned entries by the file name "harness-hook".
            short_dir = _short_path(script.parent) if script.exists() else None
            executable = f"{short_dir}\\{script.name}" if short_dir else ""
            if not executable or " " in executable:
                raise ValueError(f"{script} contains spaces and has no 8.3 short name; Codex hooks cannot quote the "
                                 "executable. Install HARNESS_HOME at a path without spaces.")
    else:
        executable = f'"{script}"'
    return f"{executable} {agent}" + (f" {native_event}" if native_event else "")
