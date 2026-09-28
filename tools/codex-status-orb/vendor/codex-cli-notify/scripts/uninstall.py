#!/usr/bin/env python3
"""Remove Codex CLI Notify hook-only installation."""
from __future__ import annotations

import argparse
import json
import shutil
from pathlib import Path
from typing import Any, MutableMapping

PACKAGE_NAME = "codex-cli-notify"
HOOK_SCRIPT_BASENAME = "codex_cli_notify.py"
LEGACY_LOCAL_SCRIPT_BASENAME = "codex_local_notifier.py"
LEGACY_SCRIPT_BASENAME = "codex_push_notifier.py"
LEGACY_SKILL_NAME = "codex-push-notifier"
STATUS_MESSAGES = {"Codex CLI Notify", "Codex Local Notifier", "Codex Push Notifier"}
EVENTS = ["PermissionRequest", "Stop", "SubagentStop"]


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


def _remove_from_hooks(path: Path) -> bool:
    if not path.exists():
        return False
    with path.open("r", encoding="utf-8") as f:
        data = json.load(f)
    hooks = data.get("hooks", {})
    if not isinstance(hooks, MutableMapping):
        return False
    changed = False
    for event in EVENTS:
        groups = hooks.get(event, [])
        if not isinstance(groups, list):
            continue
        new_groups = [group for group in groups if not _is_our_hook_group(group)]
        if len(new_groups) != len(groups):
            hooks[event] = new_groups
            changed = True
    if changed:
        with path.open("w", encoding="utf-8") as f:
            json.dump(data, f, indent=2, ensure_ascii=False)
            f.write("\n")
    return changed


def _global_paths() -> dict[str, Path]:
    home = Path.home()
    return {
        "root": home / ".codex" / "hooks" / PACKAGE_NAME,
        "hooks": home / ".codex" / "hooks.json",
        "config": home / ".codex" / "codex-cli-notify.json",
        "legacy_skill": home / ".agents" / "skills" / LEGACY_SKILL_NAME,
    }


def _repo_paths(repo: Path) -> dict[str, Path]:
    repo = repo.expanduser().resolve()
    return {
        "root": repo / ".codex" / "hooks" / PACKAGE_NAME,
        "hooks": repo / ".codex" / "hooks.json",
        "config": repo / ".codex" / "codex-cli-notify.json",
        "legacy_skill": repo / ".agents" / "skills" / LEGACY_SKILL_NAME,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--scope", choices=["global", "repo"], default="global")
    parser.add_argument("--repo", default=".")
    parser.add_argument("--keep-package", action="store_true", help="Remove hook entries only; keep copied hook package files.")
    parser.add_argument("--remove-config", action="store_true", help="Also delete codex-cli-notify.json.")
    parser.add_argument("--remove-legacy-skill", action="store_true", help="Also delete the old .agents/skills/codex-push-notifier folder.")
    args = parser.parse_args()

    paths = _global_paths() if args.scope == "global" else _repo_paths(Path(args.repo))
    changed = _remove_from_hooks(paths["hooks"])

    removed_package = False
    if not args.keep_package and paths["root"].exists():
        shutil.rmtree(paths["root"])
        removed_package = True

    removed_config = False
    if args.remove_config and paths["config"].exists():
        paths["config"].unlink()
        removed_config = True

    removed_legacy = False
    if args.remove_legacy_skill and paths["legacy_skill"].exists():
        shutil.rmtree(paths["legacy_skill"])
        removed_legacy = True

    print(f"Hooks updated:      {changed} ({paths['hooks']})")
    print(f"Package removed:    {removed_package} ({paths['root']})")
    print(f"Config removed:     {removed_config} ({paths['config']})")
    print(f"Legacy skill removed: {removed_legacy} ({paths['legacy_skill']})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
