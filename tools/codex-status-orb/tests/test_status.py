from __future__ import annotations

import importlib
import json
import os
import subprocess
import sys
import threading
import time
from pathlib import Path
from types import MethodType


def load_common(tmp_path: Path):
    os.environ["CODEX_STATUS_ORB_DATA"] = str(tmp_path)
    import status_common

    return importlib.reload(status_common)


def test_aggregate_priority(tmp_path):
    common = load_common(tmp_path)
    assert common.aggregate_status([]) == "idle"
    assert common.aggregate_status([{"status": "running"}, {"status": "ready"}]) == "ready"
    assert common.aggregate_status([{"status": "needs_input"}, {"status": "blocked"}]) == "blocked"


def test_atomic_state_rejects_older_event(tmp_path):
    common = load_common(tmp_path)
    newer = {"session_id": "abc", "status": "ready", "event_ns": 20}
    older = {"session_id": "abc", "status": "running", "event_ns": 10}
    assert common.write_session(newer)
    assert not common.write_session(older)
    assert common.read_sessions()[0]["status"] == "ready"


def test_session_id_is_filename_safe(tmp_path):
    common = load_common(tmp_path)
    assert common.safe_session_id("../bad:id") == "badid"


def test_duplicate_hook_payload_is_claimed_once(tmp_path):
    common = load_common(tmp_path)
    payload = {"session_id": "abc", "hook_event_name": "UserPromptSubmit", "turn_id": "turn-1"}

    assert common.claim_hook_event(payload)
    assert not common.claim_hook_event(payload)
    assert common.claim_hook_event({**payload, "turn_id": "turn-2"})


def test_hook_dedup_accepts_lone_surrogate(tmp_path):
    common = load_common(tmp_path)
    payload = {"session_id": "unicode", "hook_event_name": "PostToolUse", "tool_response": "broken-\udc81"}
    assert common.claim_hook_event(payload)
    assert not common.claim_hook_event(payload)


def test_structured_turn_abort_is_detected(tmp_path):
    common = load_common(tmp_path)
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(
        json.dumps({"type": "event_msg", "payload": {"type": "turn_aborted", "reason": "interrupted", "completed_at": 20}}) + "\n",
        encoding="utf-8",
    )
    found = common.find_turn_abort_after({"session_id": "abort", "updated_at": 10, "transcript_path": str(transcript)})
    assert found and found["reason"] == "interrupted"
    assert common.find_turn_abort_after({"session_id": "abort", "updated_at": 21, "transcript_path": str(transcript)}) is None


def test_structured_task_complete_is_detected(tmp_path):
    common = load_common(tmp_path)
    transcript = tmp_path / "session.jsonl"
    transcript.write_text(
        json.dumps({"type": "event_msg", "payload": {"type": "task_complete", "turn_id": "turn-1", "completed_at": 20}}) + "\n",
        encoding="utf-8",
    )
    found = common.find_task_complete_after({"session_id": "complete", "updated_at": 10, "transcript_path": str(transcript)})
    assert found and found["turn_id"] == "turn-1"
    assert common.find_task_complete_after({"session_id": "complete", "updated_at": 21, "transcript_path": str(transcript)}) is None


def test_pending_and_resolved_user_input_are_detected(tmp_path):
    common = load_common(tmp_path)
    transcript = tmp_path / "session.jsonl"
    request = {"timestamp": "2026-09-02T05:19:23Z", "type": "response_item",
               "payload": {"type": "function_call", "name": "request_user_input", "call_id": "call-1",
                           "arguments": json.dumps({"questions": [{"header": "Technology"}]})}}
    transcript.write_text(json.dumps(request) + "\n", encoding="utf-8")
    pending = common.find_user_input_state_after({"session_id": "input", "transcript_path": str(transcript)})
    assert pending and pending["pending"] is True
    assert pending["input_kind"] == "technology_selection"
    output = {"timestamp": "2026-09-02T05:20:00Z", "type": "response_item",
              "payload": {"type": "function_call_output", "call_id": "call-1", "output": "answer"}}
    transcript.write_text(json.dumps(request) + "\n" + json.dumps(output) + "\n", encoding="utf-8")
    resolved = common.find_user_input_state_after({"session_id": "input", "transcript_path": str(transcript)})
    assert resolved and resolved["pending"] is False
    assert resolved["resolved_at"] > resolved["requested_at"]


def test_stop_hook_contract(tmp_path):
    tool_root = Path(__file__).resolve().parents[1]
    env = os.environ.copy()
    env["CODEX_STATUS_ORB_DATA"] = str(tmp_path)
    env["HERDR_ENV"] = "1"
    env["HERDR_PANE_ID"] = "w1:p1"
    payload = {"session_id": "test-session", "hook_event_name": "Stop", "cwd": str(tmp_path)}
    result = subprocess.run(
        [sys.executable, str(tool_root / "hook.py")],
        input=json.dumps(payload),
        text=True,
        capture_output=True,
        env=env,
        check=False,
    )
    assert result.returncode == 0
    assert json.loads(result.stdout) == {"continue": True}
    state = json.loads((tmp_path / "sessions" / "test-session.json").read_text(encoding="utf-8"))
    assert state["status"] == "ready"
    assert state["herdr_managed"] is True
    assert state["herdr_pane_id"] == "w1:p1"


def test_permission_hook_has_no_decision(tmp_path):
    tool_root = Path(__file__).resolve().parents[1]
    env = os.environ.copy()
    env["CODEX_STATUS_ORB_DATA"] = str(tmp_path)
    env["HERDR_ENV"] = "1"
    env["HERDR_PANE_ID"] = "w1:p1"
    payload = {"session_id": "test-session", "hook_event_name": "PermissionRequest", "cwd": str(tmp_path)}
    result = subprocess.run(
        [sys.executable, str(tool_root / "hook.py")],
        input=json.dumps(payload),
        text=True,
        capture_output=True,
        env=env,
        check=False,
    )
    assert result.returncode == 0
    assert result.stdout == ""
    state = json.loads((tmp_path / "sessions" / "test-session.json").read_text(encoding="utf-8"))
    assert state["status"] == "needs_input"
    assert state["herdr_managed"] is True
    assert state["herdr_pane_id"] == "w1:p1"


def test_needs_input_uses_verified_attention_ping(monkeypatch, tmp_path):
    os.environ["CODEX_STATUS_ORB_DATA"] = str(tmp_path)
    import orb

    orb = importlib.reload(orb)
    instance = orb.StatusOrb.__new__(orb.StatusOrb)
    instance.config = {"muted": False}
    instance.sound_focus_context = MethodType(lambda _self, _session: {
        "focused": False, "foreground_pid": 0, "terminal_pid": 0,
    }, instance)
    played = []
    instance.play_attention_ping = MethodType(lambda _self: played.append(True) or True, instance)
    decisions = []
    monkeypatch.setattr(orb, "checkpoint", lambda name, **fields: decisions.append((name, fields)))

    instance.play_status("needs_input", {})

    assert played == [True]
    assert ("sound_decision", {
        "status": "needs_input", "decision": "played", "sound": "attention-ping",
        "focused": False, "foreground_pid": 0, "terminal_pid": 0,
    }) in decisions


def test_herdr_managed_session_defers_sound_to_herdr(monkeypatch, tmp_path):
    os.environ["CODEX_STATUS_ORB_DATA"] = str(tmp_path)
    import orb

    orb = importlib.reload(orb)
    instance = orb.StatusOrb.__new__(orb.StatusOrb)
    instance.config = {"muted": False}
    instance.sound_focus_context = MethodType(lambda _self, _session: {
        "focused": False, "foreground_pid": 0, "terminal_pid": 0,
    }, instance)
    played = []
    instance.play_attention_ping = MethodType(lambda _self: played.append(True) or True, instance)
    decisions = []
    monkeypatch.setattr(orb, "checkpoint", lambda name, **fields: decisions.append((name, fields)))

    instance.play_status("needs_input", {"herdr_managed": True})

    assert played == []
    assert decisions == [("sound_decision", {
        "status": "needs_input", "decision": "suppressed", "reason": "herdr-native-authority",
        "authority": "herdr", "focused": False, "foreground_pid": 0, "terminal_pid": 0,
    })]


def test_attention_ping_plays_both_tones(monkeypatch, tmp_path):
    os.environ["CODEX_STATUS_ORB_DATA"] = str(tmp_path)
    import orb

    orb = importlib.reload(orb)
    tones = []
    events = []
    monkeypatch.setattr(orb.winsound, "Beep", lambda frequency, duration: tones.append((frequency, duration)))
    monkeypatch.setattr(orb, "checkpoint", lambda name, **fields: events.append((name, fields)))

    instance = orb.StatusOrb.__new__(orb.StatusOrb)
    assert instance.play_attention_ping()
    assert tones == list(orb.ATTENTION_TONES)
    assert ("attention_ping_played", {"success": True, "tone_count": 2}) in events


def test_first_event_for_new_session_plays_status_sound(monkeypatch, tmp_path):
    os.environ["CODEX_STATUS_ORB_DATA"] = str(tmp_path)
    import orb

    orb = importlib.reload(orb)
    session = {
        "session_id": "new-session", "status": "needs_input", "event": "PermissionRequest",
        "event_ns": time.time_ns(), "state_written_ns": time.time_ns(),
    }
    monkeypatch.setattr(orb, "read_sessions", lambda: [session])
    monkeypatch.setattr(orb, "checkpoint", lambda *_args, **_fields: None)
    instance = orb.StatusOrb.__new__(orb.StatusOrb)
    instance.known_events = {}
    instance.sessions = []
    instance.dirty = threading.Event()
    for method in (
        "recover_aborted_turns", "recover_user_input_states", "recover_missing_stop_hooks",
        "discover_codex_processes", "validate_processes",
    ):
        setattr(instance, method, MethodType(lambda _self, _sessions: None, instance))
    played = []
    instance.play_status = MethodType(lambda _self, status, current: played.append((status, current["session_id"])), instance)

    instance.scan()

    assert played == [("needs_input", "new-session")]
    assert instance.known_events["new-session"] == session["event_ns"]
