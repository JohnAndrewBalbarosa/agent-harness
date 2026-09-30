//! The `events.subscribe` request subscriber.py sends and how it classifies each incoming line.

use harness_hook::events::is_python_truthy;
use serde_json::{json, Map, Value};
use uuid::Uuid;

pub const SUBSCRIBED_EVENT_TYPES: [&str; 8] = [
    "workspace.created",
    "workspace.closed",
    "tab.created",
    "tab.closed",
    "pane.created",
    "pane.exited",
    "pane.closed",
    "pane.agent_detected",
];
const REQUEST_ID_PREFIX: &str = "shared-";
const SUBSCRIBE_METHOD: &str = "events.subscribe";

/// `f"shared-{uuid.uuid4()}"`.
#[inline]
pub fn new_request_id() -> String {
    format!("{REQUEST_ID_PREFIX}{}", Uuid::new_v4())
}

/// `{"id", "method": "events.subscribe", "params": {"subscriptions": [{"type": ...}, ...]}}`.
pub fn subscribe_request(request_id: &str) -> Value {
    let subscriptions: Vec<Value> = SUBSCRIBED_EVENT_TYPES
        .iter()
        .map(|name| json!({ "type": name }))
        .collect();
    json!({
        "id": request_id,
        "method": SUBSCRIBE_METHOD,
        "params": { "subscriptions": subscriptions },
    })
}

/// `json.dumps(request, separators=(",", ":")) + "\n"` (all ASCII, so `ensure_ascii` changes nothing).
pub fn subscribe_line(request_id: &str) -> String {
    format!("{}\n", subscribe_request(request_id))
}

/// One decoded line.
#[derive(Debug, Clone, PartialEq)]
pub enum IncomingMessage {
    /// Valid JSON that is not an object: skipped.
    Ignored,
    /// The reply to our request; `is_rejected` when its `error` is truthy.
    Reply { is_rejected: bool },
    /// Anything else: a Herdr event.
    Event(Map<String, Value>),
    /// Not JSON (`json.JSONDecodeError`).
    Malformed,
}

// Mental model: lossy UTF-8 → JSON → non-object ignored → our id = reply → else event.
pub fn classify_line(line: &[u8], request_id: &str) -> IncomingMessage {
    let text = String::from_utf8_lossy(line);
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return IncomingMessage::Malformed;
    };
    let Value::Object(message) = value else {
        return IncomingMessage::Ignored;
    };
    if message.get("id").and_then(Value::as_str) == Some(request_id) {
        let is_rejected = message.get("error").is_some_and(is_python_truthy);
        return IncomingMessage::Reply { is_rejected };
    }
    IncomingMessage::Event(message)
}
