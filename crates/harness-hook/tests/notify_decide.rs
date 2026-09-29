//! Port of tests/unit/test_notify_decide.py.

use harness_hook::consumers::notify::decide::{decide, Action, NotifyState, RateLimit, Sound};
use harness_hook::events::{make_event, Event};
use serde_json::{json, Map, Value};

const NOW: i64 = 1_000_000;
const RATE: RateLimit = RateLimit {
    limit: 5,
    window_s: 60,
};

fn ev(event_type: &str, data: Value) -> Event {
    let native = match event_type {
        "prompt.submit" => "UserPromptSubmit",
        "turn.stop" => "Stop",
        _ => "Notification",
    };
    let payload = json!({"session_id": "s"}).as_object().cloned().unwrap();
    let data: Map<String, Value> = data.as_object().cloned().unwrap_or_default();
    make_event(event_type, "claude", "default", &payload, native, data)
}

fn attention(kind: &str, message: Value) -> Event {
    ev(
        "attention.needed",
        json!({"attention": {"kind": kind, "message": message}}),
    )
}

fn state(pending: bool, recent: &[i64]) -> NotifyState {
    NotifyState {
        pending,
        recent_toasts: recent.to_vec(),
    }
}

#[test]
fn prompt_marks_pending() {
    let (decision, new_state) = decide(
        &ev("prompt.submit", json!({"prompt": "x"})),
        &state(false, &[]),
        &[],
        NOW,
        RATE,
    );
    assert_eq!((decision.action, new_state.pending), (Action::Mark, true));
}

#[test]
fn stop_with_pending_and_no_background_toasts_done() {
    let (decision, new_state) = decide(
        &ev("turn.stop", json!({})),
        &state(true, &[]),
        &[],
        NOW,
        RATE,
    );
    assert_eq!(
        (decision.action, decision.sound, new_state.pending),
        (Action::Toast, Some(Sound::Done), false)
    );
    assert_eq!(decision.message.as_deref(), Some("Tapos na ang prompt!"));
    assert_eq!(new_state.recent_toasts, vec![NOW]);
}

#[test]
fn stop_without_pending_is_skipped() {
    let (decision, _) = decide(
        &ev("turn.stop", json!({})),
        &state(false, &[]),
        &[],
        NOW,
        RATE,
    );
    assert_eq!(
        (decision.action, decision.reason),
        (Action::Skip, "no-pending-prompt")
    );
}

#[test]
fn stop_hook_active_is_skipped_and_keeps_pending() {
    let (decision, new_state) = decide(
        &ev("turn.stop", json!({"stop_hook_active": true})),
        &state(true, &[]),
        &[],
        NOW,
        RATE,
    );
    assert_eq!(
        (decision.action, decision.reason, new_state.pending),
        (Action::Skip, "stop-hook-active", true)
    );
}

#[test]
fn stop_with_background_tasks_defers_and_keeps_pending() {
    let outstanding = vec!["agent-1".to_owned()];
    let (decision, new_state) = decide(
        &ev("turn.stop", json!({})),
        &state(true, &[]),
        &outstanding,
        NOW,
        RATE,
    );
    assert_eq!(
        (decision.action, decision.reason, new_state.pending),
        (Action::Defer, "background-tasks", true)
    );
}

#[test]
fn permission_always_toasts_request() {
    let outstanding = vec!["agent-1".to_owned()];
    let event = attention("permission", json!("Claude needs your permission"));
    let (decision, _) = decide(&event, &state(false, &[]), &outstanding, NOW, RATE);
    assert_eq!(
        (decision.action, decision.sound, decision.message.as_deref()),
        (
            Action::Toast,
            Some(Sound::Request),
            Some("Claude needs your permission")
        )
    );
}

#[test]
fn idle_defers_while_background_tasks_run() {
    let outstanding = vec!["shell-1".to_owned()];
    let (decision, _) = decide(
        &attention("idle", json!("waiting")),
        &state(false, &[]),
        &outstanding,
        NOW,
        RATE,
    );
    assert_eq!(
        (decision.action, decision.reason),
        (Action::Defer, "background-tasks")
    );
}

#[test]
fn idle_without_background_toasts() {
    let (decision, _) = decide(
        &attention("idle", json!("waiting")),
        &state(false, &[]),
        &[],
        NOW,
        RATE,
    );
    assert_eq!(
        (decision.action, decision.sound),
        (Action::Toast, Some(Sound::Request))
    );
}

#[test]
fn rate_limit_skips_and_old_toasts_expire() {
    let busy: Vec<i64> = (0..5).map(|offset| NOW - offset).collect();
    let (decision, new_state) = decide(
        &ev("turn.stop", json!({})),
        &state(true, &busy),
        &[],
        NOW,
        RATE,
    );
    assert_eq!(
        (decision.action, decision.reason, new_state.pending),
        (Action::Skip, "rate-limited", false)
    );

    let old: Vec<i64> = (0..5).map(|offset| NOW - 100 - offset).collect();
    let (decision, _) = decide(
        &ev("turn.stop", json!({})),
        &state(true, &old),
        &[],
        NOW,
        RATE,
    );
    assert_eq!(decision.action, Action::Toast);
}

#[test]
fn future_timestamps_do_not_block() {
    let skewed: Vec<i64> = (0..5).map(|offset| NOW + 28_800 + offset).collect();
    let (decision, _) = decide(
        &ev("turn.stop", json!({})),
        &state(true, &skewed),
        &[],
        NOW,
        RATE,
    );
    assert_eq!(decision.action, Action::Toast);
}

#[test]
fn attention_message_default() {
    let (decision, _) = decide(
        &attention("permission", Value::Null),
        &state(false, &[]),
        &[],
        NOW,
        RATE,
    );
    assert_eq!(decision.message.as_deref(), Some("Kailangan ng input mo!"));
}

#[test]
fn state_is_not_mutated() {
    let original = state(true, &[NOW - 1]);
    let _ = decide(&ev("turn.stop", json!({})), &original, &[], NOW, RATE);
    assert_eq!(original, state(true, &[NOW - 1]));
}
