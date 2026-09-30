//! What subscriber.py `record_event` reads from one Herdr message: the event body, its name and its pane.

use harness_hook::events::{is_python_truthy, python_str};
use obs_cli::python_compat::truncate_chars;
use serde_json::{Map, Value};

const DEFAULT_EVENT_NAME: &str = "herdr.event";
const MAX_EVENT_NAME_CHARS: usize = 160;
const PANE_KEYS: [&str; 2] = ["pane_id", "paneId"];

/// `value.get("event") if isinstance(value.get("event"), dict) else value`.
#[inline]
pub fn event_body(message: &Map<String, Value>) -> &Map<String, Value> {
    match message.get("event") {
        Some(Value::Object(body)) => body,
        _ => message,
    }
}

/// `str(event.get("event") or event.get("type") or value.get("type") or "herdr.event")[:160]`.
pub fn event_name(message: &Map<String, Value>, body: &Map<String, Value>) -> String {
    let candidates = [body.get("event"), body.get("type"), message.get("type")];
    let name = candidates
        .into_iter()
        .flatten()
        .find(|value| is_python_truthy(value))
        .map(python_str)
        .unwrap_or_else(|| DEFAULT_EVENT_NAME.to_owned());
    truncate_chars(&name, MAX_EVENT_NAME_CHARS)
}

// Mental model: depth-first; at each object its own truthy `pane_id`/`paneId` wins before any child is searched,
// children are visited in document order; "" means no pane.
/// `find_pane(value)`.
pub fn find_pane(value: &Value) -> String {
    match value {
        Value::Object(map) => find_pane_in_object(map),
        Value::Array(items) => first_pane(items.iter()),
        _ => String::new(),
    }
}

/// `find_pane` applied to an object.
pub fn find_pane_in_object(map: &Map<String, Value>) -> String {
    let own_pane = PANE_KEYS
        .iter()
        .filter_map(|key| map.get(*key))
        .find(|value| is_python_truthy(value));
    match own_pane {
        Some(pane) => python_str(pane),
        None => first_pane(map.values()),
    }
}

#[inline]
fn first_pane<'a>(children: impl Iterator<Item = &'a Value>) -> String {
    children
        .map(find_pane)
        .find(|pane| !pane.is_empty())
        .unwrap_or_default()
}
