//! Common event contract `agent-harness.event/v1` (`core/events.py`).

use obs_cli::python_compat::json_text::{to_python_json, JsonLayout};
use serde_json::{json, Map, Value};

pub const SCHEMA: &str = "agent-harness.event/v1";
pub const EVENT_TYPES: [&str; 9] = [
    "session.start",
    "session.end",
    "prompt.submit",
    "tool.requested",
    "tool.completed",
    "turn.stop",
    "attention.needed",
    "subagent.start",
    "subagent.stop",
];
pub const NATIVE_PAYLOAD_LIMIT_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub event: String,
    pub agent: String,
    pub instance: String,
    pub session_id: String,
    pub cwd: Option<String>,
    pub transcript_path: Option<String>,
    pub ts: String,
    pub turn_id: Option<String>,
    pub data: Map<String, Value>,
    pub native_event: String,
    pub native_payload: Map<String, Value>,
}

/// `make_event`: common keys are snake_case in both Codex and Claude payloads; a native payload whose
/// JSON exceeds 8 KiB is dropped and flagged `data.native_truncated = true`.
pub fn make_event(
    event: &str,
    agent: &str,
    instance: &str,
    payload: &Map<String, Value>,
    native_event: &str,
    data: Map<String, Value>,
) -> Event {
    let mut event_data = data;
    let native_payload = if is_oversized(payload) {
        event_data.insert("native_truncated".into(), Value::Bool(true));
        Map::new()
    } else {
        payload.clone()
    };

    Event {
        event: event.to_owned(),
        agent: agent.to_owned(),
        instance: instance.to_owned(),
        session_id: session_id_of(payload),
        cwd: optional_text(payload.get("cwd")),
        transcript_path: optional_text(payload.get("transcript_path")),
        ts: utc_now_millis(),
        turn_id: optional_text(payload.get("turn_id")),
        data: event_data,
        native_event: native_event.to_owned(),
        native_payload,
    }
}

/// `to_json(event)`: `json.dumps(..., ensure_ascii=False)` with default separators and this key order.
pub fn to_json(event: &Event) -> String {
    let document = json!({
        "schema": SCHEMA,
        "event": event.event,
        "agent": event.agent,
        "instance": event.instance,
        "session_id": event.session_id,
        "turn_id": event.turn_id,
        "cwd": event.cwd,
        "transcript_path": event.transcript_path,
        "ts": event.ts,
        "data": Value::Object(event.data.clone()),
        "native": {"event": event.native_event, "payload": Value::Object(event.native_payload.clone())},
    });
    to_python_json(&document, JsonLayout::Spaced, false)
}

/// `%Y-%m-%dT%H:%M:%S.mmmZ`.
pub fn utc_now_millis() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

#[inline]
pub fn is_known_event(event: &str) -> bool {
    EVENT_TYPES.contains(&event)
}

#[inline]
fn is_oversized(payload: &Map<String, Value>) -> bool {
    let text = to_python_json(&Value::Object(payload.clone()), JsonLayout::Spaced, false);
    text.len() > NATIVE_PAYLOAD_LIMIT_BYTES
}

/// `str(payload.get("session_id") or "unknown")`.
fn session_id_of(payload: &Map<String, Value>) -> String {
    match payload.get("session_id") {
        Some(value) if is_python_truthy(value) => python_str(value),
        _ => "unknown".to_owned(),
    }
}

/// `str(value) if value not in (None, "") else None`.
fn optional_text(value: Option<&Value>) -> Option<String> {
    match value {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) if text.is_empty() => None,
        Some(other) => Some(python_str(other)),
    }
}

/// Python `str()` of a JSON value (strings unquoted, everything else as `json.dumps` text is close enough
/// for identifiers, which are always strings in real payloads).
pub fn python_str(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::Null => "None".to_owned(),
        other => to_python_json(other, JsonLayout::Spaced, false),
    }
}

/// Python truthiness of a JSON value.
pub fn is_python_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|float| float != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn missing_session_becomes_unknown_and_empty_strings_become_none() {
        let event = make_event(
            "turn.stop",
            "claude",
            "default",
            &payload(json!({"cwd": "", "turn_id": "t1"})),
            "Stop",
            Map::new(),
        );
        assert_eq!(event.session_id, "unknown");
        assert_eq!(event.cwd, None);
        assert_eq!(event.turn_id.as_deref(), Some("t1"));
    }

    #[test]
    fn oversized_native_payload_is_dropped_and_flagged() {
        let big = "x".repeat(NATIVE_PAYLOAD_LIMIT_BYTES);
        let event = make_event(
            "tool.completed",
            "claude",
            "default",
            &payload(json!({"session_id": "s", "blob": big})),
            "PostToolUse",
            Map::new(),
        );
        assert!(event.native_payload.is_empty());
        assert_eq!(event.data["native_truncated"], Value::Bool(true));
    }

    #[test]
    fn json_uses_python_default_separators_and_key_order() {
        let mut event = make_event(
            "session.end",
            "codex",
            "cy",
            &payload(json!({"session_id": "s1"})),
            "SessionEnd",
            Map::new(),
        );
        event.ts = "2026-09-30T01:02:03.004Z".into();
        let text = to_json(&event);
        assert!(text.starts_with(
            r#"{"schema": "agent-harness.event/v1", "event": "session.end", "agent": "codex""#
        ));
        assert!(text
            .ends_with(r#""native": {"event": "SessionEnd", "payload": {"session_id": "s1"}}}"#));
    }
}
