//! subscriber.py `record_event` inputs: event body, name, pane, binding lookup, subscribe request, line classes.

use harness_services::herdr_subscriber::event_payload::{event_body, event_name, find_pane};
use harness_services::herdr_subscriber::event_recorder::RecordedEvent;
use harness_services::herdr_subscriber::pane_binding::{binding_for_pane, PaneBinding};
use harness_services::herdr_subscriber::subscription::{
    classify_line, subscribe_line, subscribe_request, IncomingMessage,
};
use serde_json::{json, Map, Value};

fn object(value: Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap()
}

#[test]
fn body_is_the_nested_event_object_or_the_message() {
    let wrapped = object(json!({"type": "event", "event": {"event": "pane.created"}}));
    assert_eq!(
        event_body(&wrapped),
        &object(json!({"event": "pane.created"}))
    );
    let flat = object(json!({"event": "pane.closed", "pane_id": "p"}));
    assert_eq!(event_body(&flat), &flat);
}

#[test]
fn name_prefers_body_event_then_body_type_then_message_type() {
    let message = object(json!({"type": "outer", "event": {"event": "", "type": "inner"}}));
    assert_eq!(event_name(&message, event_body(&message)), "inner");
    let message = object(json!({"type": "outer", "event": {"event": 0}}));
    assert_eq!(event_name(&message, event_body(&message)), "outer");
    let message = object(json!({"data": 1}));
    assert_eq!(event_name(&message, event_body(&message)), "herdr.event");
    let message = object(json!({"event": 42}));
    assert_eq!(event_name(&message, event_body(&message)), "42");
    let message = object(json!({"event": "n".repeat(200)}));
    assert_eq!(event_name(&message, event_body(&message)).len(), 160);
}

#[test]
fn pane_is_found_depth_first_with_own_keys_first() {
    assert_eq!(
        find_pane(&json!({"paneId": "camel", "child": {"pane_id": "deep"}})),
        "camel"
    );
    assert_eq!(
        find_pane(&json!({"pane_id": "", "paneId": "second"})),
        "second"
    );
    assert_eq!(
        find_pane(&json!({"a": [1, {"b": {"pane_id": 7}}], "c": {"pane_id": "later"}})),
        "7"
    );
    assert_eq!(find_pane(&json!({"a": {"pane_id": null}})), "");
    assert_eq!(find_pane(&json!("pane_id")), "");
}

#[test]
fn recorded_event_combines_name_pane_and_sanitized_payload() {
    let message =
        object(json!({"event": {"event": "pane.exited", "pane_id": "w1:p2", "token": "t"}}));
    let recorded = RecordedEvent::from_message(&message, 3);
    assert_eq!(recorded.event_name, "pane.exited");
    assert_eq!(recorded.pane_id, "w1:p2");
    assert_eq!(recorded.sequence, 3);
    assert_eq!(recorded.payload["token"], json!("<redacted>"));

    let binding = PaneBinding {
        project_id: json!("proj"),
        native_session: Value::Null,
    };
    let data = recorded.operation_data(&binding);
    let keys: Vec<&str> = data
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "herdrEventId",
            "projectId",
            "nativeSession",
            "paneId",
            "eventName",
            "sequenceNo",
            "payload"
        ]
    );
    assert_eq!(data["paneId"], json!("w1:p2"));
    assert_eq!(data["sequenceNo"], json!(3));
    assert_eq!(data["herdrEventId"].as_str().unwrap().len(), 36);
}

#[test]
fn binding_is_the_newest_session_with_the_pane_and_a_project() {
    let dir = tempfile::tempdir().unwrap();
    let write = |name: &str, value: Value| {
        std::fs::write(dir.path().join(name), value.to_string()).unwrap();
    };
    write(
        "old.json",
        json!({"pane_id": "p1", "project_id": "A", "session_id": "s-old", "updated_at": 10.0}),
    );
    write(
        "new.json",
        json!({"pane_id": "p1", "project_id": "B", "session_id": "s-new", "updated_at": 20.5}),
    );
    write(
        "noproject.json",
        json!({"pane_id": "p1", "project_id": "", "updated_at": 99}),
    );
    write(
        "other.json",
        json!({"pane_id": "p2", "project_id": "C", "updated_at": 50}),
    );
    write(
        "ignored.txt",
        json!({"pane_id": "p1", "project_id": "D", "updated_at": 100}),
    );
    std::fs::write(dir.path().join("broken.json"), b"{not json").unwrap();

    assert_eq!(
        binding_for_pane(dir.path(), "p1"),
        Some(PaneBinding {
            project_id: json!("B"),
            native_session: json!("s-new")
        })
    );
    assert_eq!(
        binding_for_pane(dir.path(), "p2").map(|binding| binding.native_session),
        Some(Value::Null)
    );
    assert_eq!(binding_for_pane(dir.path(), ""), None);
    assert_eq!(binding_for_pane(dir.path(), "p9"), None);
    assert_eq!(binding_for_pane(&dir.path().join("missing"), "p1"), None);
}

#[test]
fn subscribe_request_matches_the_python_json() {
    let line = subscribe_line("shared-x");
    assert_eq!(
        line,
        concat!(
            r#"{"id":"shared-x","method":"events.subscribe","params":{"subscriptions":["#,
            r#"{"type":"workspace.created"},{"type":"workspace.closed"},{"type":"tab.created"},"#,
            r#"{"type":"tab.closed"},{"type":"pane.created"},{"type":"pane.exited"},"#,
            r#"{"type":"pane.closed"},{"type":"pane.agent_detected"}]}}"#,
            "\n"
        )
    );
    assert_eq!(
        subscribe_request("shared-x")["method"],
        json!("events.subscribe")
    );
}

#[test]
fn lines_are_classified_like_the_python_loop() {
    assert_eq!(
        classify_line(br#"{"id":"r1","result":{}}"#, "r1"),
        IncomingMessage::Reply { is_rejected: false }
    );
    assert_eq!(
        classify_line(br#"{"id":"r1","error":{"code":1}}"#, "r1"),
        IncomingMessage::Reply { is_rejected: true }
    );
    assert_eq!(
        classify_line(br#"{"id":"r1","error":null}"#, "r1"),
        IncomingMessage::Reply { is_rejected: false }
    );
    assert_eq!(classify_line(b"[1,2]", "r1"), IncomingMessage::Ignored);
    assert_eq!(classify_line(b"not json", "r1"), IncomingMessage::Malformed);
    assert_eq!(classify_line(b"", "r1"), IncomingMessage::Malformed);
    assert!(matches!(
        classify_line(br#"{"id":"other","event":"x"}"#, "r1"),
        IncomingMessage::Event(_)
    ));
}
