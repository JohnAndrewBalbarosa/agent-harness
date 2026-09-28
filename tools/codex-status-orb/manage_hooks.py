from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import time
from pathlib import Path
from typing import Any

TOOL_ROOT = Path(__file__).resolve().parent
MARKER = "Codex Status Orb"
EVENTS = ["SessionStart", "UserPromptSubmit", "PermissionRequest", "PostToolUse", "Stop", "SessionEnd"]


def discover_codex_homes(user_home: Path | None = None, env_home: str | None = None) -> list[Path]:
    base = (user_home or Path.home()).resolve()
    candidates = [base / ".codex"]
    configured = env_home if env_home is not None else os.environ.get("CODEX_HOME")
    if configured:
        candidates.append(Path(configured).expanduser())
    candidates.extend(path for path in base.glob(".codex*") if (path / "config.toml").is_file())

    homes: list[Path] = []
    seen: set[str] = set()
    for candidate in candidates:
        resolved = candidate.resolve()
        key = str(resolved).casefold()
        if key not in seen and (resolved.name == ".codex" or (resolved / "config.toml").is_file()):
            seen.add(key)
            homes.append(resolved)
    return homes


def load_hooks(hooks_path: Path) -> dict[str, Any]:
    if not hooks_path.exists():
        return {"hooks": {}}
    with hooks_path.open("r", encoding="utf-8") as stream:
        value = json.load(stream)
    if not isinstance(value, dict) or not isinstance(value.setdefault("hooks", {}), dict):
        raise ValueError(f"Invalid hooks structure: {hooks_path}")
    return value


def is_ours(group: Any) -> bool:
    if not isinstance(group, dict):
        return False
    return any(isinstance(hook, dict) and hook.get("statusMessage") == MARKER for hook in group.get("hooks", []))


def backup(codex_home: Path, hooks_path: Path) -> Path | None:
    if not hooks_path.exists():
        return None
    folder = codex_home / "backups"
    folder.mkdir(parents=True, exist_ok=True)
    target = folder / f"hooks.before-status-orb.{time.strftime('%Y%m%d-%H%M%S')}.json"
    shutil.copy2(hooks_path, target)
    return target


def command() -> str:
    python = TOOL_ROOT / ".venv" / "Scripts" / "python.exe"
    hook = TOOL_ROOT / "hook.py"
    # Codex's Windows command runner rejects a command whose executable starts
    # with a quote. This installation path has no spaces, so quote only the
    # script argument.
    return f'{python} "{hook}"'


def install_home(codex_home: Path) -> None:
    hooks_path = codex_home / "hooks.json"
    saved = backup(codex_home, hooks_path)
    data = load_hooks(hooks_path)
    hooks = data["hooks"]
    for event in EVENTS:
        groups = hooks.get(event, [])
        if not isinstance(groups, list):
            groups = []
        groups = [group for group in groups if not is_ours(group)]
        handler = {
            "type": "command",
            "command": command(),
            "commandWindows": command(),
            "timeout": 10 if event == "SessionEnd" else 15,
            "statusMessage": MARKER,
        }
        group = {"hooks": [handler]}
        # Record attention before the interactive notifier blocks waiting for a choice.
        if event == "PermissionRequest":
            groups.insert(0, group)
        else:
            groups.append(group)
        hooks[event] = groups
    hooks_path.parent.mkdir(parents=True, exist_ok=True)
    with hooks_path.open("w", encoding="utf-8", newline="\n") as stream:
        json.dump(data, stream, indent=2, ensure_ascii=False)
        stream.write("\n")
    print(f"Installed {MARKER} hooks in {hooks_path}")
    if saved:
        print(f"Backup: {saved}")


def uninstall_home(codex_home: Path) -> None:
    hooks_path = codex_home / "hooks.json"
    if not hooks_path.exists():
        return
    saved = backup(codex_home, hooks_path)
    data = load_hooks(hooks_path)
    hooks = data["hooks"]
    for event in EVENTS:
        groups = hooks.get(event, [])
        if isinstance(groups, list):
            hooks[event] = [group for group in groups if not is_ours(group)]
    with hooks_path.open("w", encoding="utf-8", newline="\n") as stream:
        json.dump(data, stream, indent=2, ensure_ascii=False)
        stream.write("\n")
    print(f"Removed {MARKER} hooks from {hooks_path}; unrelated hooks preserved.")
    if saved:
        print(f"Backup: {saved}")


def install_homes(homes: list[Path]) -> None:
    """Install into every profile because named Codex homes can run alone.

    Some Codex Desktop launches load only the active ``CODEX_HOME`` hooks file.
    ``claim_hook_event`` deduplicates lifecycle events when a launch happens to
    load both the default and named-profile hook files.
    """
    for home in homes:
        install_home(home)


def install() -> None:
    shared_sync = Path.home() / ".codex-shared" / "sync-global.cmd"
    if shared_sync.exists():
        subprocess.run([os.environ.get("COMSPEC", "cmd.exe"), "/d", "/c", str(shared_sync)], check=True)
        return
    install_homes(discover_codex_homes())


def uninstall() -> None:
    if (Path.home() / ".codex-shared" / "hooks.json").exists():
        print("Shared runtime owns hooks; status-orb uninstall left shared hooks unchanged.")
        return
    for codex_home in discover_codex_homes():
        uninstall_home(codex_home)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=["install", "uninstall"])
    args = parser.parse_args()
    install() if args.action == "install" else uninstall()
