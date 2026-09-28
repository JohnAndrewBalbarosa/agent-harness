#!/usr/bin/env python3
"""Install Codex CLI Notify as hook-only configuration.

Global install:
  python3 scripts/install.py --scope global

Repo-local install:
  python3 scripts/install.py --scope repo --repo /path/to/repo
"""
from __future__ import annotations

import argparse
import json
import os
import platform
import shutil
import stat
import sys
from pathlib import Path
from typing import Any, Dict, MutableMapping, Optional

PACKAGE_NAME = "codex-cli-notify"
HOOK_SCRIPT_BASENAME = "codex_cli_notify.py"
LEGACY_LOCAL_SCRIPT_BASENAME = "codex_local_notifier.py"
LEGACY_SCRIPT_BASENAME = "codex_push_notifier.py"
LEGACY_SKILL_NAME = "codex-push-notifier"
STATUS_MESSAGES = {"Codex CLI Notify", "Codex Local Notifier", "Codex Push Notifier"}
EVENTS = {
    "PermissionRequest": "*",
    "Stop": None,
    "SubagentStop": "*",
}
EVENT_TIMEOUTS = {
    "PermissionRequest": 600,
    "Stop": 5,
    "SubagentStop": 5,
}
DEFAULT_CONFIG = {
    "events": ["PermissionRequest", "Stop", "SubagentStop"],
    "log_path": "~/.codex/codex-cli-notify.log",
    "encoding": "utf-8",
    "language": "auto",
    "message_max_chars": 360,
    "windows": {
        "enabled": True,
        "mode": "overlay",
        "duration_ms": 6500,
        "width": 440,
        "margin": 24,
        "position": "bottom-right",
        "color_mode": "accent",
        "opacity": 0.96,
        "colors": {
            "PermissionRequest": "#F59E0B",
            "Stop": "#22C55E",
            "SubagentStop": "#3B82F6",
            "default": "#64748B",
        },
        "permission_actions": {
            "enabled": True,
            "timeout_seconds": 300,
            "timeout_choice": "defer",
            "close_choice": "defer",
            "deny_message": "Denied from the Codex local permission toast.",
            "button_labels": {
                "allow": "Allow once",
                "deny": "Deny",
                "defer": "Use CLI prompt",
            },
        },
    },
    "macos": {"enabled": True},
    "linux": {"enabled": True, "notify_send_path": "notify-send"},
}


def _source_root() -> Path:
    return Path(__file__).resolve().parents[1]


def _copy_tree(src: Path, dst: Path) -> None:
    if src.resolve() == dst.resolve():
        return
    if dst.exists():
        shutil.rmtree(dst)
    ignore = shutil.ignore_patterns(".DS_Store", "__pycache__", "*.pyc")
    shutil.copytree(src, dst, ignore=ignore)


def _ensure_executable(path: Path) -> None:
    if os.name != "nt":
        mode = path.stat().st_mode
        path.chmod(mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def _load_hooks(path: Path) -> MutableMapping[str, Any]:
    if not path.exists():
        return {"hooks": {}}
    try:
        with path.open("r", encoding="utf-8") as f:
            data = json.load(f)
        if not isinstance(data, MutableMapping):
            raise ValueError("hooks.json root is not an object")
        hooks = data.setdefault("hooks", {})
        if not isinstance(hooks, MutableMapping):
            raise ValueError("hooks key is not an object")
        return data
    except Exception as exc:
        backup = path.with_suffix(path.suffix + ".bak")
        shutil.copy2(path, backup)
        print(f"Existing hooks file could not be parsed; backed up to {backup}: {exc}", file=sys.stderr)
        return {"hooks": {}}


def _is_our_hook_group(group: Any) -> bool:
    if not isinstance(group, dict):
        return False
    for hook in group.get("hooks", []) or []:
        if not isinstance(hook, dict):
            continue
        command = str(hook.get("command") or "") + " " + str(hook.get("commandWindows") or hook.get("command_windows") or "")
        status = str(hook.get("statusMessage") or "")
        if (
            HOOK_SCRIPT_BASENAME in command
            or LEGACY_LOCAL_SCRIPT_BASENAME in command
            or LEGACY_SCRIPT_BASENAME in command
            or status in STATUS_MESSAGES
        ):
            return True
    return False


def _quote(path: Path) -> str:
    return f'"{str(path)}"'


def _posix_command(script: Path, config: Path) -> str:
    return f'python3 {_quote(script)} --config {_quote(config)}'


def _windows_command(script: Path, config: Path) -> str:
    launcher = "py -3" if shutil.which("py") else "python"
    return f'{launcher} {_quote(script)} --config {_quote(config)}'


def _hook_command(script: Path, config: Path, event: str) -> Dict[str, Any]:
    hook: Dict[str, Any] = {
        "type": "command",
        "timeout": EVENT_TIMEOUTS.get(event, 5),
        "statusMessage": "Codex CLI Notify",
    }
    if platform.system().lower() == "windows":
        hook["command"] = _windows_command(script, config)
        hook["commandWindows"] = _windows_command(script, config)
    else:
        hook["command"] = _posix_command(script, config)
    return hook


def _merge_hooks(hooks_path: Path, script_path: Path, config_path: Path) -> None:
    data = _load_hooks(hooks_path)
    hooks_obj = data.setdefault("hooks", {})
    assert isinstance(hooks_obj, MutableMapping)

    for event, matcher in EVENTS.items():
        existing = hooks_obj.get(event, [])
        if not isinstance(existing, list):
            existing = []
        existing = [group for group in existing if not _is_our_hook_group(group)]
        group: Dict[str, Any] = {"hooks": [_hook_command(script_path, config_path, event)]}
        if matcher is not None:
            group["matcher"] = matcher
        existing.append(group)
        hooks_obj[event] = existing

    hooks_path.parent.mkdir(parents=True, exist_ok=True)
    with hooks_path.open("w", encoding="utf-8") as f:
        json.dump(data, f, indent=2, ensure_ascii=False)
        f.write("\n")


def _write_config(config_path: Path, overwrite: bool = False) -> None:
    config_path.parent.mkdir(parents=True, exist_ok=True)
    if config_path.exists() and not overwrite:
        return
    with config_path.open("w", encoding="utf-8") as f:
        json.dump(DEFAULT_CONFIG, f, indent=2, ensure_ascii=False)
        f.write("\n")


def _global_paths() -> Dict[str, Path]:
    home = Path.home()
    return {
        "root": home / ".codex" / "hooks" / PACKAGE_NAME,
        "hooks": home / ".codex" / "hooks.json",
        "config": home / ".codex" / "codex-cli-notify.json",
        "legacy_skill": home / ".agents" / "skills" / LEGACY_SKILL_NAME,
    }


def _repo_paths(repo: Path) -> Dict[str, Path]:
    repo = repo.expanduser().resolve()
    if not repo.exists() or not repo.is_dir():
        raise SystemExit(f"Repo path does not exist or is not a directory: {repo}")
    return {
        "root": repo / ".codex" / "hooks" / PACKAGE_NAME,
        "hooks": repo / ".codex" / "hooks.json",
        "config": repo / ".codex" / "codex-cli-notify.json",
        "legacy_skill": repo / ".agents" / "skills" / LEGACY_SKILL_NAME,
    }


def _remove_legacy_skill(path: Path) -> bool:
    if path.exists() and path.is_dir():
        shutil.rmtree(path)
        return True
    return False


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--scope", choices=["global", "repo"], default="global", help="Install globally for this user or repo-locally.")
    parser.add_argument("--repo", default=".", help="Repository path for --scope repo.")
    parser.add_argument("--overwrite-config", action="store_true", help="Overwrite an existing codex-cli-notify.json.")
    parser.add_argument("--remove-legacy-skill", action="store_true", help="Remove the old ~/.agents/skills/codex-push-notifier folder if present.")
    args = parser.parse_args()

    src = _source_root()
    paths = _global_paths() if args.scope == "global" else _repo_paths(Path(args.repo))
    target_root = paths["root"]
    target_root.parent.mkdir(parents=True, exist_ok=True)
    _copy_tree(src, target_root)

    script_path = target_root / "scripts" / HOOK_SCRIPT_BASENAME
    _ensure_executable(script_path)
    _write_config(paths["config"], overwrite=args.overwrite_config)
    _merge_hooks(paths["hooks"], script_path, paths["config"])
    legacy_removed = _remove_legacy_skill(paths["legacy_skill"]) if args.remove_legacy_skill else False

    print("Codex CLI Notify installed (hook-only).")
    print(f"Hook package: {target_root}")
    print(f"Hooks file:   {paths['hooks']}")
    print(f"Config file:  {paths['config']}")
    print(f"Script:       {script_path}")
    if legacy_removed:
        print(f"Removed legacy skill folder: {paths['legacy_skill']}")
    print("Next steps:")
    print("  1. Restart Codex if it is already running.")
    print("  2. Run /hooks in Codex and trust the Codex CLI Notify hook definitions.")
    print(f"  3. Test notifications: python3 {script_path} --config {paths['config']} --test all")
    print(f"  4. Test approval stdout: python3 {script_path} --config {paths['config']} --test permission --simulate-choice allow")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
