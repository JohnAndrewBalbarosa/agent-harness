from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path

USER = Path.home()
TOOL = USER / ".codex-shared" / "tools" / "observability-hub"
INSTANCES = {
    USER / ".codex": "hook-personal.cmd",
    USER / ".codex-feu": "hook-feu.cmd",
    USER / ".codex-cy": "hook-cy.cmd",
}
EVENTS = ["SessionStart", "UserPromptSubmit", "PostToolUse", "Stop", "SessionEnd"]


def install(config_path: Path, wrapper: str) -> None:
    value = json.loads(config_path.read_text(encoding="utf-8"))
    hooks = value.setdefault("hooks", {})
    command = str(TOOL / wrapper)
    for event in EVENTS:
        entries = hooks.setdefault(event, [])
        managed = [
            nested
            for entry in entries
            for nested in entry.get("hooks", [])
            if isinstance(nested, dict) and nested.get("statusMessage") == "Central Observability"
        ]
        timeout = 10 if event == "SessionEnd" else 15
        if managed:
            for nested in managed:
                nested.update(command=command, commandWindows=command, timeout=timeout)
        else:
            entries.append({"hooks": [{"type": "command", "command": command, "commandWindows": command, "timeout": timeout, "statusMessage": "Central Observability"}]})
    config_path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


sync = USER / ".codex-shared" / "sync-global.cmd"
if sync.exists():
    subprocess.run([os.environ.get("COMSPEC", "cmd.exe"), "/d", "/c", str(sync)], check=True)
else:
    for codex_dir, wrapper in INSTANCES.items():
        install(codex_dir / "hooks.json", wrapper)
