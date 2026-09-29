//! Port of tests/unit/test_claude_background.py.

use chrono::{DateTime, TimeZone, Utc};
use harness_hook::transcripts::claude_background::{background_shells, scan};
use harness_hook::transcripts::jsonl_stream::Entry;
use serde_json::{json, Value};

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 28, 12, 0, 0).unwrap()
}

fn entry(value: Value) -> Entry {
    value.as_object().cloned().unwrap()
}

fn tool_result(text: &str, ts: &str) -> Entry {
    entry(json!({"type": "user", "timestamp": ts,
                 "message": {"content": [{"type": "tool_result", "tool_use_id": "t", "content": text}]}}))
}

fn notification(task_id: &str, status: &str, entry_type: &str) -> Entry {
    let text = format!("<task-notification>\n<task-id>{task_id}</task-id>\n<status>{status}</status>\n</task-notification>");
    let ts = "2026-09-28T11:55:00Z";
    if entry_type == "user" {
        return entry(json!({"type": "user", "timestamp": ts, "message": {"content": text}}));
    }
    entry(json!({"type": entry_type, "timestamp": ts, "content": text}))
}

fn bash_background(shell_id: &str, ts: &str) -> Entry {
    tool_result(
        &format!(
            "Command running in background with ID: {shell_id}. Output is being written to ..."
        ),
        ts,
    )
}

fn bash_timeout(shell_id: &str) -> Entry {
    tool_result(
        &format!("Command did not complete within its 120s timeout and was moved to the background (ID: {shell_id}). Output ..."),
        "2026-09-28T11:50:00Z",
    )
}

const TS: &str = "2026-09-28T11:50:00Z";

#[test]
fn background_shells_are_tracked() {
    let entries = [bash_background("b1x", TS), bash_timeout("b2y")];
    assert_eq!(scan(&entries, now()), ["b1x", "b2y"]);
}

#[test]
fn notifications_complete_shells_in_any_entry_type() {
    let entries = [
        bash_background("b1", TS),
        bash_timeout("b2"),
        notification("b1", "stopped", "queue-operation"),
        notification("b2", "failed", "attachment"),
    ];
    assert!(scan(&entries, now()).is_empty());
}

#[test]
fn user_task_notification_completes_a_shell() {
    let entries = [
        bash_background("b1", TS),
        notification("b1", "completed", "user"),
    ];
    assert!(scan(&entries, now()).is_empty());
}

#[test]
fn agent_launches_are_left_to_subagent_hooks() {
    let agent = tool_result(
        "Async agent launched successfully.\nagentId: abc123 (internal ID)",
        TS,
    );
    assert!(scan([&agent], now()).is_empty());
}

#[test]
fn echoed_markers_are_ignored() {
    let echoed = tool_result(
        "120 LAUNCH 'moved to the background (ID: b85)'\nCommand running in background with ID: zz",
        TS,
    );
    assert!(scan([&echoed], now()).is_empty());
}

#[test]
fn echoed_notification_does_not_complete() {
    let entries = [
        bash_background("b1", TS),
        tool_result("grep hit: <task-notification>\n<task-id>b1</task-id>", TS),
    ];
    assert_eq!(scan(&entries, now()), ["b1"]);
}

#[test]
fn stale_shells_are_ignored() {
    let entries = [
        bash_background("old", "2026-09-28T08:00:00Z"),
        bash_background("new", TS),
    ];
    assert_eq!(scan(&entries, now()), ["new"]);
}

#[test]
fn background_shells_reads_transcript_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.jsonl");
    let first = Value::Object(bash_background("s9", TS)).to_string();
    std::fs::write(&path, format!("{first}\nnot json\n")).unwrap();
    assert_eq!(background_shells(&path, now()), ["s9"]);
}

#[test]
fn missing_transcript_is_empty() {
    assert!(background_shells(std::path::Path::new("/does/not/exist.jsonl"), now()).is_empty());
}
