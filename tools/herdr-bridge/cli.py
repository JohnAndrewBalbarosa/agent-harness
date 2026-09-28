from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
import uuid
from pathlib import Path

from common import SESSION_ROOT, herdr_binary, load_observability_module, project_record, read_json
from state import highest_priority


def sessions() -> list[dict]:
    values: list[dict] = []
    if not SESSION_ROOT.exists():
        return values
    for path in SESSION_ROOT.glob("*.json"):
        value = read_json(path)
        if value and value.get("active", True):
            values.append(value)
    return values


def focus_priority() -> int:
    target = highest_priority(sessions())
    if not target:
        print(json.dumps({"focused": False, "reason": "no_active_sessions"}))
        return 1
    pane = str(target.get("pane_id") or "")
    if target.get("runtime_scope") == "herdr" and pane:
        result = subprocess.run(
            [herdr_binary(), "agent", "focus", pane], check=False, capture_output=True, text=True,
            creationflags=int(getattr(subprocess, "CREATE_NO_WINDOW", 0)),
        )
        print(json.dumps({"focused": result.returncode == 0, "session": target.get("session_id"), "pane": pane, "state": target.get("canonical_state"), "detail": (result.stdout or result.stderr)[-500:]}))
        return result.returncode
    print(json.dumps({"focused": False, "reason": "outside_herdr", "session": target.get("session_id"), "state": target.get("canonical_state")}))
    return 2


def dashboard() -> int:
    rows = sorted(sessions(), key=lambda item: (item.get("project_id", ""), item.get("profile", ""), item.get("session_id", "")))
    print(json.dumps({"generated_at": time.time(), "sessions": rows}, ensure_ascii=False, indent=2))
    return 0


def doctor() -> int:
    shared_root = Path(__file__).resolve().parents[2]
    local_app_data = Path(os.environ.get("LOCALAPPDATA", ""))
    hook_paths = [shared_root / "hooks.json"] + [Path.home() / name / "hooks.json" for name in (".codex", ".codex-cy", ".codex-feu")]
    launchers = [local_app_data / "Microsoft" / "WindowsApps" / f"codex-{profile}.cmd" for profile in ("personal", "cy", "feu")]
    checks: list[dict] = []

    binary = Path(herdr_binary())
    checks.append({"name": "herdr_binary", "ok": binary.is_file(), "path": str(binary)})
    status = subprocess.run([str(binary), "status", "--json"], check=False, capture_output=True, text=True) if binary.is_file() else None
    try:
        status_payload = json.loads(status.stdout) if status and status.returncode == 0 else {}
    except json.JSONDecodeError:
        status_payload = {}
    server_running = bool(status_payload.get("server", {}).get("running"))
    checks.append({"name": "herdr_server", "ok": server_running})

    hooks_linked = all(path.is_file() and path.samefile(hook_paths[0]) for path in hook_paths[1:])
    checks.append({"name": "shared_hooks_hardlinks", "ok": hooks_linked, "count": len(hook_paths)})

    launcher_results = []
    for path in launchers:
        content = path.read_text(encoding="utf-8", errors="replace") if path.is_file() else ""
        launcher_results.append(path.is_file() and "herdr-codex.cmd" in content and "CODEX_HERDR_BYPASS" in content)
    checks.append({"name": "profile_launchers", "ok": all(launcher_results), "count": sum(launcher_results)})

    hook_source = Path(__file__).with_name("hook.py").read_text(encoding="utf-8", errors="replace")
    protocol_ok = "report-agent-session" in hook_source and "herdr:codex" in hook_source and "report-agent" in hook_source
    checks.append({"name": "session_and_lifecycle_protocol", "ok": protocol_ok})

    active = sessions()
    bound = sum(1 for item in active if item.get("runtime_scope") == "herdr" and item.get("pane_id"))
    healthy = all(item["ok"] for item in checks)
    print(json.dumps({"healthy": healthy, "checks": checks, "activeSessions": len(active), "herdrBoundSessions": bound}, ensure_ascii=False, indent=2))
    return 0 if healthy else 1


def append_handoff(args: argparse.Namespace) -> int:
    project = project_record(args.cwd)
    if not project:
        raise SystemExit("No observability.project.toml found for the requested cwd.")
    project_id = str(project[1]["id"])
    obs = load_observability_module()
    obs.enqueue([obs.operation("handoff.record", {
        "handoffId": str(uuid.uuid4()), "projectId": project_id,
        "baseVersion": args.base_version, "handoffKind": args.kind,
        "profileName": args.profile, "nativeSession": args.session,
        "content": args.content,
    })])
    print(json.dumps({"recorded": True, "projectId": project_id, "kind": args.kind}))
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(prog="herdr-bridge")
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("focus-priority")
    sub.add_parser("dashboard")
    sub.add_parser("doctor")
    handoff = sub.add_parser("handoff")
    handoff.add_argument("--cwd", required=True)
    handoff.add_argument("--profile", choices=["personal", "cy", "feu"], required=True)
    handoff.add_argument("--kind", choices=["progress", "unfinished", "observation", "decision", "architecture", "constraint"], required=True)
    handoff.add_argument("--content", required=True)
    handoff.add_argument("--session", default="")
    handoff.add_argument("--base-version", type=int)
    args = parser.parse_args()
    if args.command == "focus-priority":
        return focus_priority()
    if args.command == "dashboard":
        return dashboard()
    if args.command == "doctor":
        return doctor()
    return append_handoff(args)


if __name__ == "__main__":
    raise SystemExit(main())
