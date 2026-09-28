from __future__ import annotations

import json
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from pathlib import Path
from typing import Any

from common import (
    SESSION_ROOT,
    atomic_json,
    content_hash,
    herdr_binary,
    load_observability_module,
    log_event,
    profile_name,
    project_record,
    read_json,
    safe_id,
)
from state import decision_for_hook


def payload_value(payload: dict[str, Any], *names: str, default: Any = "") -> Any:
    for name in names:
        value = payload.get(name)
        if value not in (None, ""):
            return value
    return default


def run_herdr(args: list[str], timeout: float = 4.0, output_limit: int = 1000) -> tuple[int, str]:
    started = time.perf_counter()
    try:
        process = subprocess.Popen(
            [herdr_binary(), *args],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            encoding="utf-8",
            errors="replace",
            close_fds=True,
            creationflags=int(getattr(subprocess, "CREATE_NO_WINDOW", 0)),
        )
        log_event("helper.process.started", "started", operation=safe_id(args[0] if args else "unknown"),
                  processId=process.pid, windowMode="hidden")
        try:
            stdout, _ = process.communicate(timeout=timeout)
        except subprocess.TimeoutExpired:
            process.kill()
            stdout, _ = process.communicate()
            log_event("helper.process.failed", "failed", operation=safe_id(args[0] if args else "unknown"),
                      processId=process.pid, reason="timeout", durationMs=round((time.perf_counter() - started) * 1000, 3),
                      windowMode="hidden")
            return 124, (stdout or "")[-output_limit:]
        log_event("helper.process.exited", "succeeded" if process.returncode == 0 else "failed",
                  operation=safe_id(args[0] if args else "unknown"), processId=process.pid, exitCode=process.returncode,
                  durationMs=round((time.perf_counter() - started) * 1000, 3), windowMode="hidden")
        return process.returncode, (stdout or "")[-output_limit:]
    except OSError as exc:
        log_event("helper.process.failed", "failed", operation=safe_id(args[0] if args else "unknown"),
                  reason="launch_failed", errorType=type(exc).__name__, windowMode="hidden")
        return 1, str(exc)[:1000]


def ensure_subscriber() -> None:
    if os.environ.get("HERDR_ENV") != "1" or not os.environ.get("HERDR_SOCKET_PATH"):
        return
    script = Path(__file__).with_name("subscriber.py")
    flags = 0
    for name in ("DETACHED_PROCESS", "CREATE_NEW_PROCESS_GROUP", "CREATE_NO_WINDOW"):
        flags |= int(getattr(subprocess, name, 0))
    try:
        process = subprocess.Popen(
            [sys.executable, str(script)],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            close_fds=True,
            creationflags=flags,
            env=os.environ.copy(),
        )
        log_event("subscriber.process.started", "started", processId=process.pid, windowMode="hidden")
    except OSError as exc:
        log_event("subscriber.launch", "failed", error=type(exc).__name__)


def redact_evidence(value: str) -> str:
    value = re.sub(r"(?i)(authorization|api[_-]?key|token|password|secret|cookie)\s*[:=]\s*\S+", r"\1=<redacted>", value)
    return value[-32768:]


def capture_evidence(project_id: str, native_session: str, pane_id: str, trigger: str) -> None:
    read_code, recent = run_herdr(["pane", "read", pane_id, "--source", "recent-unwrapped", "--lines", "80"], timeout=5.0, output_limit=32768)
    explain_code, explain_raw = run_herdr(["agent", "explain", pane_id, "--json"], timeout=5.0, output_limit=12000)
    if read_code != 0 and explain_code != 0:
        return
    recent = redact_evidence(recent)
    try:
        explain = json.loads(explain_raw) if explain_code == 0 else {"error": explain_raw[-1000:]}
    except json.JSONDecodeError:
        explain = {"raw": redact_evidence(explain_raw)[-4000:]}
    encoded = recent.encode("utf-8", errors="replace")[:32768]
    recent = encoded.decode("utf-8", errors="replace")
    enqueue_observability("pane-evidence.record", {
        "snapshotId": str(uuid.uuid4()), "projectId": project_id, "nativeSession": native_session,
        "paneId": pane_id, "triggerCode": trigger, "contentSha256": content_hash(recent),
        "lineCount": min(80, recent.count("\n") + (1 if recent else 0)), "byteCount": len(encoded),
        "redactedContent": recent, "explain": explain if isinstance(explain, dict) else {"value": explain},
    })


def enqueue_observability(kind: str, data: dict[str, Any]) -> None:
    try:
        obs = load_observability_module()
        obs.enqueue([obs.operation(kind, data)], flush_now=False)
    except Exception as exc:  # hook integration must never block Codex
        log_event("observability.enqueue", "failed", operation=kind, error=type(exc).__name__)


def exact_usage(payload: dict[str, Any]) -> tuple[dict[str, int | None], str]:
    candidates = [payload.get("usage"), payload.get("token_usage")]
    response = payload.get("response")
    if isinstance(response, dict):
        candidates.extend([response.get("usage"), response.get("token_usage")])
    source = next((item for item in candidates if isinstance(item, dict)), None)
    aliases = {
        "inputTokens": ("input_tokens", "inputTokens", "prompt_tokens"),
        "outputTokens": ("output_tokens", "outputTokens", "completion_tokens"),
        "cachedInputTokens": ("cached_input_tokens", "cachedInputTokens", "cached_tokens"),
        "reasoningTokens": ("reasoning_tokens", "reasoningTokens"),
    }
    values: dict[str, int | None] = {}
    if source:
        for target, names in aliases.items():
            raw = next((source.get(name) for name in names if source.get(name) is not None), None)
            values[target] = int(raw) if isinstance(raw, (int, float)) and raw >= 0 else None
    else:
        values = {target: None for target in aliases}
    availability = "exact" if any(value is not None for value in values.values()) else "unavailable"
    return values, availability


def build_context(project_id: str, profile: str, source: str) -> str:
    endpoint = os.environ.get("OBS_ENDPOINT", "http://127.0.0.1:4319")
    query = urllib.parse.urlencode({"profile": profile, "source": source})
    url = f"{endpoint}/v1/projects/{urllib.parse.quote(project_id)}/context/bootstrap?{query}"
    try:
        with urllib.request.urlopen(url, timeout=2.0) as response:
            value = json.load(response)
    except (OSError, urllib.error.URLError, json.JSONDecodeError):
        return ""
    if not isinstance(value, dict):
        return ""
    sections: list[str] = []
    context = value.get("context")
    if isinstance(context, dict):
        for label, key in (("Vision", "vision_summary"), ("Architecture", "architecture_summary"), ("Current goal", "current_goal_summary")):
            text = str(context.get(key) or "").strip()
            if text:
                sections.append(f"{label}: {text}")
        constraints = context.get("constraints")
        if isinstance(constraints, list) and constraints:
            sections.append("Constraints: " + "; ".join(str(item) for item in constraints[:12]))
    handoffs = value.get("handoffs")
    if isinstance(handoffs, list) and handoffs:
        lines = [f"- [{item.get('handoff_kind','progress')}] {str(item.get('content',''))[:1200]}" for item in handoffs[:5]]
        sections.append("Live handoff:\n" + "\n".join(lines))
    errors = value.get("errors")
    if isinstance(errors, list) and errors:
        lines = [f"- {str(item.get('normalized_summary',''))[:500]}" for item in errors[:3]]
        sections.append("Unresolved errors:\n" + "\n".join(lines))
    code_version = value.get("code_version")
    if isinstance(code_version, dict):
        sections.append(f"Code version: {code_version.get('commit_sha') or 'uncommitted'} ({code_version.get('branch_name') or 'no branch'})")
    if not sections:
        return ""
    limit = 6000 if source != "compact" else 3000
    return ("Shared project context (database-derived; query older evidence only when needed):\n\n" + "\n\n".join(sections))[:limit]


def main() -> int:
    try:
        payload = json.load(sys.stdin)
        if not isinstance(payload, dict):
            payload = {}
    except json.JSONDecodeError:
        payload = {}

    event = str(payload_value(payload, "hook_event_name", "hookEventName", default=os.environ.get("CODEX_STATUS_EVENT", "")))
    native_session = safe_id(payload_value(payload, "session_id", "sessionId"))
    native_turn = safe_id(payload_value(payload, "turn_id", "turnId"), "")
    cwd = str(payload_value(payload, "cwd", "working_directory", default=os.getcwd()))
    profile = profile_name()
    project = project_record(cwd)
    project_id = (str(project[1]["id"]) if project else
                  str(uuid.uuid5(uuid.NAMESPACE_URL, "codex-observability:global-agent-project")))
    pane_id = str(os.environ.get("HERDR_PANE_ID", "")).strip()
    workspace_id = str(os.environ.get("HERDR_WORKSPACE_ID", "")).strip()
    tab_id = str(os.environ.get("HERDR_TAB_ID", "")).strip()
    runtime_scope = "herdr" if pane_id and os.environ.get("HERDR_ENV") == "1" else "outside_herdr"
    if runtime_scope == "herdr":
        ensure_subscriber()
    state_path = SESSION_ROOT / f"{native_session}.json"
    previous = read_json(state_path)
    decision = decision_for_hook(event, payload)
    sequence = int(previous.get("sequence_no", 0)) + 1
    now = time.time()

    record = {
        **previous,
        "session_id": native_session,
        "turn_id": native_turn,
        "project_id": project_id,
        "profile": profile,
        "cwd": cwd,
        "runtime_scope": runtime_scope,
        "workspace_id": workspace_id,
        "tab_id": tab_id,
        "pane_id": pane_id,
        "sequence_no": sequence,
        "updated_at": now,
    }
    if decision:
        record.update(
            canonical_state=decision.canonical,
            status="running" if decision.canonical == "working" else decision.canonical,
            projected_state=decision.projected,
            reason=decision.reason,
            authority=decision.authority,
            signal=decision.signal,
            state_entered_at=now if previous.get("canonical_state") != decision.canonical else previous.get("state_entered_at", now),
        )
    atomic_json(state_path, record)

    if project_id:
        enqueue_observability("herdr.binding", {
            "bindingId": str(uuid.uuid4()), "projectId": project_id, "nativeSession": native_session,
            "profileName": profile, "workspaceId": workspace_id or None, "tabId": tab_id or None,
            "paneId": pane_id or None, "runtimeScope": runtime_scope, "cwd": cwd,
        })
        if decision:
            enqueue_observability("agent-state.record", {
                "stateEventId": str(uuid.uuid4()), "projectId": project_id, "nativeSession": native_session,
                "nativeTurn": native_turn or None, "profileName": profile,
                "previousState": previous.get("canonical_state"), "canonicalState": decision.canonical,
                "projectedState": decision.projected, "reason": decision.reason,
                "authority": decision.authority, "signalSource": decision.signal,
                "sequenceNo": sequence, "conflict": False,
                "attributes": {"runtimeScope": runtime_scope, "paneId": pane_id or None},
            })
        if event == "Stop":
            usage, availability = exact_usage(payload)
            enqueue_observability("turn-usage.record", {
                "usageId": str(uuid.uuid4()), "projectId": project_id, "nativeSession": native_session,
                "nativeTurn": native_turn or None, "promptId": None,
                **usage, "availability": availability, "source": "codex_hook",
                "sourceEventId": f"{native_session}:{native_turn}:{event}:{sequence}",
            })

    if runtime_scope == "herdr":
        if event == "SessionStart":
            args = ["pane", "report-agent-session", pane_id, "--source", "herdr:codex", "--agent", "codex", "--seq", str(sequence), "--agent-session-id", native_session]
            run_herdr(args)
        if decision:
            args = ["pane", "report-agent", pane_id, "--source", "shared:codex-state", "--agent", "codex", "--state", decision.projected, "--message", f"{decision.canonical}: {decision.reason}", "--seq", str(sequence)]
            code, output = run_herdr(args)
            log_event("state.projected", "succeeded" if code == 0 else "failed", session=native_session, state=decision.canonical, projected=decision.projected, pane=pane_id, detail=output[:300])
            if project_id and (previous.get("canonical_state") != decision.canonical or code != 0):
                capture_evidence(project_id, native_session, pane_id, "state.transition" if code == 0 else "state.projection_failed")

    if event == "SessionEnd":
        if runtime_scope == "herdr":
            run_herdr(["pane", "release-agent", pane_id, "--source", "shared:codex-state", "--agent", "codex"])
        record["active"] = False
        atomic_json(state_path, record)

    if event == "SessionStart" and project_id:
        source = str(payload.get("source") or "startup")
        context = build_context(project_id, profile, source)
        if context:
            print(json.dumps({"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": context}}, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
