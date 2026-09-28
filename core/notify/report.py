"""Cross-reference agent-harness notifications: were toasts premature, missing, or silent?

Deterministic merge of four sources for a time window:
  1. notify consumer decisions  <root>/var/notify/notify-events.jsonl (AH_NOTIFY_DIR)
  2. Windows toast history      %LOCALAPPDATA%/Microsoft/Windows/Notifications/wpndatabase.db
                                (copied, read-only; Windows drops toasts once dismissed)
  3. Claude Code transcripts    turn boundaries: prompts, end_turn, background-task notifications
  4. Turn token usage           <root>/var/usage/turn-usage.jsonl

Usage: python -m core.notify.report [--minutes 180] [--limit 80] [--transcript PATH ...]
"""
from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import sqlite3
import tempfile
from datetime import datetime, timedelta, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
NOTIFY_LOG = Path(os.environ.get("AH_NOTIFY_DIR") or ROOT / "var" / "notify") / "notify-events.jsonl"
USAGE_LOG = ROOT / "var" / "usage" / "turn-usage.jsonl"
HARNESS_TITLES = ("Claude Code", "Codex")
TOAST_DB = Path(os.environ.get("LOCALAPPDATA", "")) / "Microsoft" / "Windows" / "Notifications" / "wpndatabase.db"
PREMATURE_GRACE = timedelta(seconds=5)
RELEVANT_TOAST_APP = re.compile(r"(?i)powershell|notifyicon|herdr|codex|claude|terminal")
TOAST_TEXT = re.compile(r"<text[^>]*>(.*?)</text>", re.S)
USER_BOUNDARIES = ("prompt", "command")


def parse_ts(value) -> datetime | None:
    if not value:
        return None
    try:
        return datetime.fromisoformat(str(value).replace("Z", "+00:00")).astimezone(timezone.utc)
    except ValueError:
        return None


def load_jsonl(path: Path) -> list[dict]:
    if not path.exists():
        return []
    rows = []
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        try:
            value = json.loads(line)
        except ValueError:
            continue
        if isinstance(value, dict):
            rows.append(value)
    return rows


def classify(entry: dict) -> str | None:
    if entry.get("type") == "assistant":
        return "assistant"
    if entry.get("type") != "user" or entry.get("isMeta"):
        return None
    content = (entry.get("message") or {}).get("content")
    if isinstance(content, list):
        if any(isinstance(block, dict) and block.get("type") == "tool_result" for block in content):
            return None
        content = " ".join(str(block.get("text", "")) for block in content if isinstance(block, dict))
    text = str(content or "").lstrip()
    if text.startswith("<task-notification>"):
        return "task-notification"
    if text.startswith(("<command-", "<local-command-")):
        return "command"
    return "prompt"


def timeline(entries: list[dict]) -> list[tuple[datetime, str]]:
    points = []
    for entry in entries:
        kind, ts = classify(entry), parse_ts(entry.get("timestamp"))
        if not kind or not ts:
            continue
        if kind == "assistant" and (entry.get("message") or {}).get("stop_reason") == "end_turn":
            kind = "end_turn"
        points.append((ts, kind))
    return sorted(points)


def premature_reason(toast_at: datetime, points: list[tuple[datetime, str]]) -> str | None:
    """A toast is premature if work continued before the user's next real prompt."""
    for ts, kind in points:
        if ts <= toast_at:
            continue
        if kind in USER_BOUNDARIES:
            return None
        if kind == "task-notification":
            return f"background task still running (finished {local(ts)})"
        if kind in ("assistant", "end_turn") and ts > toast_at + PREMATURE_GRACE:
            return f"agent kept working (activity at {local(ts)})"
    return None


def silent_reasons(record: dict) -> list[str]:
    if record.get("event") not in ("turn.stop", "attention.needed"):
        return []
    reasons = []
    decision = str(record.get("decision") or "")
    if decision == "skip":
        reasons.append(f"no toast: skip:{record.get('reason')}")
    result = str(record.get("toastResult") or "")
    if "error" in result:
        reasons.append(f"toast failed: {result}")
    state = record.get("windowsNotificationState")
    if state and state != "accepts-notifications":
        reasons.append(f"Windows suppressing toasts/sound: {state}")
    herdr = record.get("herdr") or {}
    if herdr.get("inHerdr") and herdr.get("focused"):
        reasons.append("Herdr plays no sound (the agent's workspace is focused)")
    return reasons


def toast_text(xml: str) -> str:
    return " / ".join(part.strip() for part in TOAST_TEXT.findall(xml or "") if part.strip())


def toast_source(app: str, text: str) -> str:
    """The notify consumer titles toasts with the agent display name; Herdr's own screen-detected ones do not."""
    app = app or ""
    if "powershell" in app.lower():
        return "agent-harness (Windows)"
    if app.startswith("NotifyIcon"):
        return "agent-harness via Herdr" if text.startswith(HARNESS_TITLES) else "Herdr screen detection"
    return app


def windows_toasts(since: datetime) -> list[dict]:
    if not TOAST_DB.exists():
        return []
    with tempfile.TemporaryDirectory(prefix="claude-toasts-") as folder:
        for suffix in ("", "-wal", "-shm"):
            source = Path(str(TOAST_DB) + suffix)
            if source.exists():
                shutil.copy2(source, Path(folder) / source.name)
        conn = sqlite3.connect(f"file:{Path(folder) / TOAST_DB.name}?mode=ro", uri=True)
        try:
            rows = conn.execute(
                "SELECT n.ArrivalTime/10000000 - 11644473600, h.PrimaryId, CAST(n.Payload AS TEXT) "
                "FROM Notification n JOIN NotificationHandler h ON h.RecordId = n.HandlerId WHERE n.Type = 'toast'"
            ).fetchall()
        finally:
            conn.close()
    toasts = []
    for unix, app, payload in rows:
        ts = datetime.fromtimestamp(unix, timezone.utc)
        text = toast_text(payload)
        if ts >= since and (RELEVANT_TOAST_APP.search(app or "") or re.search(r"(?i)claude|codex", text)):
            toasts.append({"ts": ts, "app": toast_source(app, text), "text": text})
    return toasts


def local(ts: datetime) -> str:
    return ts.astimezone().strftime("%H:%M:%S")


def build_events(since, notify_rows, usage_rows, timelines, toasts) -> list[tuple[datetime, str, list[str]]]:
    events = []
    all_points = sorted(point for points in timelines.values() for point in points)
    for row in notify_rows:
        ts = parse_ts(row.get("tsUtc"))
        if not ts or ts < since:
            continue
        flags = silent_reasons(row)
        if row.get("decision") == "toast":
            reason = premature_reason(ts, timelines.get(row.get("transcriptPath") or "", all_points))
            flags += [f"PREMATURE: {reason}"] if reason else []
        herdr = row.get("herdr") or {}
        summary = f"notify {row.get('agent')}/{row.get('instance')} {row.get('event')} -> {row.get('decision')}:{row.get('reason')}"
        events.append((ts, summary, flags))
    for toast in toasts:
        reason = premature_reason(toast["ts"], all_points)
        events.append((toast["ts"], f"TOAST shown by {toast['app']}: {toast['text']}", [f"PREMATURE: {reason}"] if reason else []))
    for row in usage_rows:
        ts = parse_ts(row.get("recordedAt"))
        main = row.get("main") or {}
        if ts and ts >= since:
            events.append((ts, f"tokens: out={main.get('output_tokens')} in={main.get('input_tokens')} "
                               f"cache_read={main.get('cache_read_input_tokens')} calls={main.get('api_calls')}", []))
    for ts, kind in all_points:
        if ts >= since and kind in ("prompt", "end_turn", "task-notification"):
            events.append((ts, f"transcript: {kind}", []))
    return sorted(events, key=lambda event: event[0])


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--minutes", type=int, default=180)
    parser.add_argument("--limit", type=int, default=80)
    parser.add_argument("--transcript", action="append", default=[])
    args = parser.parse_args(argv)

    since = datetime.now(timezone.utc) - timedelta(minutes=args.minutes)
    notify_rows = load_jsonl(NOTIFY_LOG)
    paths = {row.get("transcriptPath") for row in notify_rows if row.get("transcriptPath")} | set(args.transcript)
    timelines = {path: timeline(load_jsonl(Path(path))) for path in paths if Path(path).exists()}
    events = build_events(since, notify_rows, load_jsonl(USAGE_LOG), timelines, windows_toasts(since))

    for ts, summary, flags in events[-args.limit:]:
        print(f"{local(ts)}  {summary}")
        for flag in flags:
            print(f"          !! {flag}")
    flagged = [flag for _, _, flags in events for flag in flags]
    print(f"\n{len(events)} events in last {args.minutes} min | "
          f"premature: {sum(flag.startswith('PREMATURE') for flag in flagged)} | "
          f"silent/missed flags: {sum(not flag.startswith('PREMATURE') for flag in flagged)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
