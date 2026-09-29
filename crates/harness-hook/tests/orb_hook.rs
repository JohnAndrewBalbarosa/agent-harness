//! Ported from `tools/codex-status-orb/tests/test_status.py` and `tests/unit/test_orb_hook.py` (hook side).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use harness_hook::consumer::Consumer;
use harness_hook::consumers::orb::dedupe::claim_hook_event;
use harness_hook::consumers::orb::session_store::OrbSessionStore;
use harness_hook::consumers::orb::{handle_hook, OrbConsumer};
use harness_hook::events::make_event;
use harness_hook::hook_context::HookContext;
use obs_cli::python_compat::with_platform_newlines;
use serde_json::{json, Map, Value};

fn context(root: &Path, data: &Path, payload: Value, extra_env: &[(&str, &str)]) -> HookContext {
    let native = payload.as_object().cloned().unwrap();
    let native_event = native
        .get("hook_event_name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let mut env: BTreeMap<String, String> = extra_env
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    env.insert("CODEX_STATUS_ORB_DATA".into(), data.display().to_string());
    HookContext {
        event: make_event(
            "turn.stop",
            "codex",
            "personal",
            &native,
            &native_event,
            Map::new(),
        ),
        native_bytes: serde_json::to_vec(&payload).unwrap(),
        native_payload: payload,
        env,
        harness_root: root.to_path_buf(),
    }
}

fn session_state(data: &Path, session: &str) -> Value {
    let text =
        std::fs::read_to_string(data.join("sessions").join(format!("{session}.json"))).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn herdr_env() -> [(&'static str, &'static str); 2] {
    [("HERDR_ENV", "1"), ("HERDR_PANE_ID", "w1:p1")]
}

#[test]
fn stop_hook_contract() {
    let root = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let payload = json!({"session_id": "test-session", "hook_event_name": "Stop", "cwd": data.path().display().to_string()});

    let stdout = handle_hook(&context(root.path(), data.path(), payload, &herdr_env()));

    assert_eq!(stdout, with_platform_newlines("{\"continue\":true}\n"));
    let state = session_state(data.path(), "test-session");
    assert_eq!(state["status"], "ready");
    assert_eq!(state["herdr_managed"], true);
    assert_eq!(state["herdr_pane_id"], "w1:p1");
}

#[test]
fn permission_hook_has_no_decision() {
    let root = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let payload = json!({"session_id": "test-session", "hook_event_name": "PermissionRequest", "cwd": "C:/x"});

    let stdout = handle_hook(&context(root.path(), data.path(), payload, &herdr_env()));

    assert_eq!(stdout, "");
    let state = session_state(data.path(), "test-session");
    assert_eq!(state["status"], "needs_input");
    assert_eq!(state["herdr_managed"], true);
}

#[test]
fn session_file_uses_python_layout_and_key_order() {
    let root = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let payload = json!({"session_id": "s1", "hook_event_name": "UserPromptSubmit", "cwd": "C:/work", "transcript_path": "t.jsonl"});

    handle_hook(&context(root.path(), data.path(), payload, &[]));

    let text = std::fs::read_to_string(data.path().join("sessions").join("s1.json")).unwrap();
    assert!(text.starts_with("{\n  \"session_id\": \"s1\",\n  \"event\": \"UserPromptSubmit\",\n  \"status\": \"running\",\n"));
    assert!(text.ends_with("}\n"));
    let state: Map<String, Value> = serde_json::from_str(&text).unwrap();
    let keys: Vec<&str> = state.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "session_id",
            "event",
            "status",
            "cwd",
            "transcript_path",
            "event_ns",
            "updated_at",
            "hook_received_ns",
            "herdr_managed",
            "herdr_pane_id",
            "codex_pid",
            "codex_create_time",
            "shell_pid",
            "terminal_pid",
            "terminal_kind",
            "wt_session",
            "state_written_ns",
        ]
    );
    assert_eq!(state["herdr_managed"], false);
    assert_eq!(state["transcript_path"], "t.jsonl");
    assert!(data.path().join("sessions").join(".s1.lock").is_file());
    assert!(data.path().join("sessions").join(".s1.dedupe").is_file());
}

#[test]
fn session_end_removes_the_session_file() {
    let root = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    handle_hook(&context(
        root.path(),
        data.path(),
        json!({"session_id": "gone", "hook_event_name": "Stop"}),
        &[],
    ));
    assert!(data.path().join("sessions").join("gone.json").exists());

    handle_hook(&context(
        root.path(),
        data.path(),
        json!({"session_id": "gone", "hook_event_name": "SessionEnd"}),
        &[],
    ));

    assert!(!data.path().join("sessions").join("gone.json").exists());
}

#[test]
fn session_start_writes_state_and_attempts_the_orb_launch() {
    let root = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let payload = json!({"hook_event_name": "SessionStart", "session_id": "s1", "cwd": "C:/x"});

    handle_hook(&context(
        root.path(),
        data.path(),
        payload,
        &[("ORB_UI", "0")],
    ));

    assert_eq!(session_state(data.path(), "s1")["status"], "idle");
    let log = std::fs::read_to_string(data.path().join("status-orb.log")).unwrap();
    assert!(log.contains(
        "checkpoint=hook_processed event=\"SessionStart\" session=\"s1\" status=\"idle\""
    ));
    // No venv interpreter in the temp root, so the detached launch fails and is logged, never raised.
    assert!(log.contains("orb-launch-error"));
    assert!(log.contains("checkpoint=hook_returning"));
}

#[test]
fn duplicate_hook_payload_is_claimed_once() {
    let data = tempfile::tempdir().unwrap();
    let store = OrbSessionStore::new(data.path().join("sessions"));
    let payload =
        json!({"session_id": "abc", "hook_event_name": "UserPromptSubmit", "turn_id": "turn-1"});
    let payload = payload.as_object().unwrap();

    assert!(claim_hook_event(&store, payload, 100.0).unwrap());
    assert!(!claim_hook_event(&store, payload, 101.0).unwrap());
    let mut next_turn = payload.clone();
    next_turn.insert("turn_id".into(), json!("turn-2"));
    assert!(claim_hook_event(&store, &next_turn, 102.0).unwrap());
}

#[test]
fn duplicate_window_expires_after_three_seconds_and_ignores_hook_source() {
    let data = tempfile::tempdir().unwrap();
    let store = OrbSessionStore::new(data.path().join("sessions"));
    let first = json!({"session_id": "abc", "hook_event_name": "Stop", "hook_source": "user"});
    let second = json!({"session_id": "abc", "hook_event_name": "Stop", "hookSource": "project"});

    assert!(claim_hook_event(&store, first.as_object().unwrap(), 10.0).unwrap());
    assert!(!claim_hook_event(&store, second.as_object().unwrap(), 12.0).unwrap());
    assert!(claim_hook_event(&store, second.as_object().unwrap(), 15.5).unwrap());
}

#[test]
fn duplicate_event_is_skipped_by_the_hook() {
    let root = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let payload = json!({"session_id": "dup", "hook_event_name": "Stop"});
    handle_hook(&context(root.path(), data.path(), payload.clone(), &[]));

    let stdout = handle_hook(&context(root.path(), data.path(), payload, &[]));

    assert_eq!(stdout, with_platform_newlines("{\"continue\":true}\n"));
    let log = std::fs::read_to_string(data.path().join("status-orb.log")).unwrap();
    assert!(log.contains("checkpoint=hook_duplicate_skipped"));
}

#[test]
fn atomic_state_rejects_older_event() {
    let data = tempfile::tempdir().unwrap();
    let store = OrbSessionStore::new(data.path().join("sessions"));
    let newer = json!({"session_id": "abc", "status": "ready", "event_ns": 20});
    let older = json!({"session_id": "abc", "status": "running", "event_ns": 10});

    assert!(store
        .write_session(newer.as_object().cloned().unwrap())
        .unwrap());
    assert!(!store
        .write_session(older.as_object().cloned().unwrap())
        .unwrap());
    assert_eq!(session_state(data.path(), "abc")["status"], "ready");
}

#[test]
fn consumer_subscribes_to_everything_but_subagents_and_needs_the_venv() {
    let root = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let orb = OrbConsumer;
    assert!(orb.subscribes_to("tool.completed"));
    assert!(orb.subscribes_to("attention.needed"));
    assert!(!orb.subscribes_to("subagent.start"));
    assert!(!orb.subscribes_to("unknown.event"));

    let ctx = context(root.path(), data.path(), json!({"session_id": "x"}), &[]);
    assert!(!orb.is_enabled(&ctx));
    let python: PathBuf = root.path().join("tools/codex-status-orb/.venv/Scripts");
    std::fs::create_dir_all(&python).unwrap();
    std::fs::write(python.join("python.exe"), b"").unwrap();
    assert!(orb.is_enabled(&ctx));
}
