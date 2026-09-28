"""Render the rulesync source (hooks only) that points every agent hook at harness-hook (spec §8, spike 2026-09-28)."""
from __future__ import annotations

import shutil
import sys
from typing import Mapping, Sequence

from core.config import Config, hook_command

# Native hook event -> rulesync canonical event (verified with rulesync 22.0.0 for claudecode and codexcli).
CANONICAL = {
    "SessionStart": "sessionStart", "UserPromptSubmit": "beforeSubmitPrompt", "PostToolUse": "postToolUse",
    "Stop": "stop", "SubagentStart": "subagentStart", "SubagentStop": "subagentStop", "SessionEnd": "sessionEnd",
    "Notification": "notification", "PermissionRequest": "permissionRequest",
}
TIMEOUTS_S = {"PermissionRequest": 600, "SessionEnd": 10}
DEFAULT_TIMEOUT_S = 30
RTK_TIMEOUT_S = 10


def hooks_source(cfg: Config, manifests: Mapping[str, Mapping]) -> dict:
    doc: dict = {"version": 1, "hooks": {}}
    for agent, manifest in manifests.items():
        command = hook_command(cfg, agent, style=manifest.get("hook_command_style", "quoted"))
        hooks = {CANONICAL[native]: [{"command": command, "timeout": TIMEOUTS_S.get(native, DEFAULT_TIMEOUT_S)}]
                 for native in manifest.get("events", []) if native in CANONICAL}
        if cfg.rtk_bin and _rtk_on_path():  # runs natively (not via the router): it is on every Bash call and must rewrite its input
            hooks["preToolUse"] = [{"matcher": "Bash", "command": f"{cfg.rtk_bin} hook {agent}", "timeout": RTK_TIMEOUT_S}]
        doc[manifest["rulesync_target"]] = {"hooks": hooks}
    return doc


def _rtk_on_path() -> bool:
    """rtk rewrites commands to a bare `rtk <cmd>`: without `rtk` on PATH every Bash call would fail (exit 127)."""
    if shutil.which("rtk"):
        return True
    print("compile: RTK_BIN is set but `rtk` is not on PATH; rtk hook skipped", file=sys.stderr)
    return False


def rulesync_config(targets: Sequence[str]) -> dict:
    """`delete` MUST stay false and `preserveUnownedHooks` true: hand-written hooks in real homes are kept."""
    return {"targets": list(targets), "features": ["hooks"], "outputRoots": ["."], "delete": False,
            "preserveUnownedHooks": True, "global": True, "verbose": False, "silent": True}
