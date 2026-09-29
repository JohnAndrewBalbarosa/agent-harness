//! Ported from `tools/herdr-bridge/tests/test_state.py`.

use harness_hook::consumers::herdr_bridge::state_machine::{decision_for_hook, highest_priority};
use serde_json::{json, Map, Value};

fn object(value: Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap()
}

fn canonical(event: &str) -> &'static str {
    decision_for_hook(event, &Map::new()).unwrap().canonical
}

#[test]
fn hook_mapping_preserves_custom_semantics() {
    assert_eq!(canonical("SessionStart"), "idle");
    assert_eq!(canonical("UserPromptSubmit"), "working");
    assert_eq!(canonical("PermissionRequest"), "needs_input");
    assert_eq!(canonical("Stop"), "ready");
    assert!(decision_for_hook("SessionEnd", &Map::new()).is_none());
}

#[test]
fn permission_reason_is_bounded_and_projected_as_blocked() {
    let payload = object(json!({"permission_reason": "x".repeat(300)}));
    let decision = decision_for_hook("PermissionRequest", &payload).unwrap();
    assert_eq!(decision.projected, "blocked");
    assert_eq!(decision.reason.len(), 160);
    assert_eq!(
        decision_for_hook("PermissionRequest", &Map::new())
            .unwrap()
            .reason,
        "permission"
    );
}

#[test]
fn needs_input_ties_use_oldest_waiting() {
    let older = object(
        json!({"session_id": "older", "canonical_state": "needs_input", "state_entered_at": 10}),
    );
    let newer = object(
        json!({"session_id": "newer", "canonical_state": "needs_input", "state_entered_at": 20}),
    );
    let sessions = [newer, older];
    assert_eq!(
        highest_priority(&sessions, 100.0).unwrap()["session_id"],
        "older"
    );
}

#[test]
fn other_ties_use_newest_transition() {
    let older =
        object(json!({"session_id": "older", "canonical_state": "ready", "state_entered_at": 10}));
    let newer =
        object(json!({"session_id": "newer", "canonical_state": "ready", "state_entered_at": 20}));
    let sessions = [older, newer];
    assert_eq!(
        highest_priority(&sessions, 100.0).unwrap()["session_id"],
        "newer"
    );
}

#[test]
fn higher_priority_state_wins_and_empty_has_no_winner() {
    let blocked = object(json!({"session_id": "b", "status": "blocked"}));
    let working =
        object(json!({"session_id": "w", "canonical_state": "working", "updated_at": 99}));
    assert_eq!(
        highest_priority(&[working, blocked], 100.0).unwrap()["session_id"],
        "b"
    );
    assert!(highest_priority(&[], 100.0).is_none());
}
