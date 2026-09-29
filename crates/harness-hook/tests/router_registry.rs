//! Port of `tests/unit/test_consumers.py` (+ the services routing check of `test_services.py`): which real
//! consumers the router picks for each common event.

use std::collections::BTreeSet;
use std::path::PathBuf;

use harness_hook::consumers::all_consumers;
use harness_hook::events::make_event;
use harness_hook::hook_context::HookContext;
use serde_json::{Map, Value};

fn context(event_type: &str, native: &str, env: &[(&str, &str)]) -> HookContext {
    let payload: Map<String, Value> = [("session_id".to_owned(), Value::from("s"))]
        .into_iter()
        .collect();
    HookContext {
        event: make_event(
            event_type,
            "claude",
            "default",
            &payload,
            native,
            Map::new(),
        ),
        native_payload: Value::Object(payload),
        native_bytes: Vec::new(),
        env: env
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect(),
        harness_root: PathBuf::from("does-not-exist"),
    }
}

/// Names of the consumers subscribed to the event (event gating only).
fn names(event_type: &str) -> BTreeSet<&'static str> {
    all_consumers()
        .into_iter()
        .filter(|consumer| consumer.subscribes_to(event_type))
        .map(|consumer| consumer.name())
        .collect()
}

#[test]
fn turn_stop_routes_to_obs_notify_and_usage() {
    let routed = names("turn.stop");
    assert!(
        ["obs", "notify", "usage-fallback"]
            .iter()
            .all(|name| routed.contains(name)),
        "{routed:?}"
    );
    assert!(!routed.contains("tasks") && !routed.contains("context"));
}

#[test]
fn subagent_events_route_to_tasks_only_among_harness_consumers() {
    let routed = names("subagent.start");
    assert!(routed.contains("tasks"));
    assert!(
        ["obs", "notify", "context"]
            .iter()
            .all(|name| !routed.contains(name)),
        "{routed:?}"
    );
}

#[test]
fn turn_stop_routes_to_the_context_guard_only() {
    assert!(names("turn.stop").contains("context-guard"));
    assert!(!names("prompt.submit").contains("context-guard"));
}

#[test]
fn session_start_routes_to_context_and_services() {
    let routed = names("session.start");
    assert!(routed.contains("context") && routed.contains("services"));
    assert!(!names("turn.stop").contains("services"));
}

#[test]
fn herdr_bridge_only_inside_herdr() {
    let enabled = |env: &[(&str, &str)]| {
        let context = context("turn.stop", "Stop", env);
        all_consumers().into_iter().any(|consumer| {
            consumer.name() == "herdr-bridge"
                && consumer.subscribes_to("turn.stop")
                && consumer.is_enabled(&context)
        })
    };
    assert!(!enabled(&[]));
    assert!(!enabled(&[("HERDR_ENV", "1")]));
    assert!(enabled(&[("HERDR_ENV", "1"), ("HERDR_PANE_ID", "w1:p1")]));
}
