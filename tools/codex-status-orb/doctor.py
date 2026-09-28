from __future__ import annotations

import importlib.metadata
import hashlib
import json
import subprocess
import sys
from pathlib import Path

from manage_hooks import discover_codex_homes
from status_common import DATA_ROOT, FOCUS_DIAGNOSTICS_PATH, HEALTH_PATH, LOG_PATH, TOOL_ROOT, ensure_dirs, load_config, load_json, process_matches

EXPECTED = {
    "pystray": "0.19.5",
    "Pillow": "12.3.0",
    "psutil": "7.2.2",
    "pywin32": "312",
    "watchdog": "6.0.0",
    "pywinauto": "0.6.9",
}


def check(name: str, ok: bool, detail: str = "") -> bool:
    print(f"{'PASS' if ok else 'FAIL'}  {name}{': ' + detail if detail else ''}")
    return ok


def main() -> int:
    results: list[bool] = []
    results.append(check("Python", sys.version_info[:2] >= (3, 11), sys.version.split()[0]))
    for package, version in EXPECTED.items():
        try:
            actual = importlib.metadata.version(package)
            results.append(check(package, actual == version, actual))
        except Exception as exc:
            results.append(check(package, False, str(exc)))
    ensure_dirs()
    results.append(check("Data directory", DATA_ROOT.exists(), str(DATA_ROOT)))
    shared_hooks = Path.home() / ".codex-shared" / "hooks.json"
    for codex_home in discover_codex_homes():
        hooks_path = codex_home / "hooks.json"
        try:
            hooks = json.loads(hooks_path.read_text(encoding="utf-8"))
            serialized = json.dumps(hooks)
            linked = shared_hooks.is_file() and hooks_path.samefile(shared_hooks)
            results.append(check(
                f"Shared router hooks ({codex_home.name})",
                linked and "global-hook-router.cmd" in serialized,
                str(hooks_path),
            ))
        except Exception as exc:
            results.append(check(f"Hooks file ({codex_home.name})", False, str(exc)))
    router_path = Path.home() / ".codex-shared" / "tools" / "global-hook-router" / "router.py"
    router_source = router_path.read_text(encoding="utf-8", errors="replace") if router_path.is_file() else ""
    results.append(check(
        "Notification authority routing",
        "notification_authority" in router_source and "legacy-fallback" in router_source,
        str(router_path),
    ))
    vendor = TOOL_ROOT / "vendor" / "codex-cli-notify"
    try:
        commit = subprocess.check_output(["git", "-C", str(vendor), "rev-parse", "HEAD"], text=True).strip()
        results.append(check("Notifier pin", commit == "ff01555b99d41ff95322d06ab92d1f2650141cfc", commit))
    except Exception as exc:
        results.append(check("Notifier pin", False, str(exc)))
    config = load_config()
    results.append(check("Orb config", isinstance(config, dict), str(config)))
    health = load_json(HEALTH_PATH, {})
    heartbeat_age = __import__("time").time() - float(health.get("updated_at", 0)) if health else 999999
    health_ok = bool(health) and heartbeat_age < 10 and process_matches(int(health.get("pid", 0)))
    results.append(check("Orb heartbeat", health_ok, f"age={heartbeat_age:.1f}s checkpoint={health.get('checkpoint', 'missing')}"))
    results.append(check("Log file", LOG_PATH.exists(), str(LOG_PATH)))
    bell = TOOL_ROOT / "assets" / "service-bell.mp3"
    bell_hash = hashlib.sha256(bell.read_bytes()).hexdigest().upper() if bell.is_file() else "missing"
    results.append(check("Done bell", bell_hash == "64A54F3B2F3967D936AB66F2C0CE01A920C4C10A493891BD069C25EEC54BF1F1", bell_hash))
    focus_diagnostics = load_json(FOCUS_DIAGNOSTICS_PATH, {})
    focus_ok = not focus_diagnostics or all(key in focus_diagnostics for key in ("success", "total_ms", "started_at"))
    results.append(check("Focus diagnostics", focus_ok, str(FOCUS_DIAGNOSTICS_PATH)))
    print("\nHealthy" if all(results) else "\nNeeds attention")
    return 0 if all(results) else 1


if __name__ == "__main__":
    raise SystemExit(main())
