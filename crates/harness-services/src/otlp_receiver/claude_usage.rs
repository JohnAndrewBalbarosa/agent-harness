//! adapters/claude/otel.py `map`: Claude Code `api_request` → `turn-usage.record` data (whitelisted
//! fields only). One row per API request; `nativeTurn` is the OTel `prompt.id`. OTel carries no
//! reasoning tokens (the transcript fallback has them).

use obs_cli::python_compat::{truncate_chars, uuid5_url_text};
use serde_json::{json, Value};

use super::otlp_decoding::OtelAttributes;
use super::python_semantics::{count, first_truthy, is_truthy, python_str};

/// otel.py:13 `API_REQUEST_EVENTS`.
pub const API_REQUEST_EVENTS: [&str; 2] = ["api_request", "claude_code.api_request"];
/// `str(...)[:240]` session bound shared by both adapters.
pub const SESSION_CHAR_LIMIT: usize = 240;

/// `None` for other events and for requests without an id (they cannot be deduplicated safely).
pub fn map_api_request(attributes: &OtelAttributes) -> Option<Value> {
    // Mental model: event filter → request id (dedupe key) → token math → record in Python key order.
    if !is_api_request(attributes) {
        return None;
    }
    let request_id = request_id(attributes)?;
    let cache_read = count(attributes.get("cache_read_tokens"));
    let input_tokens = count(attributes.get("input_tokens"))
        .saturating_add(cache_read)
        .saturating_add(count(attributes.get("cache_creation_tokens")));

    Some(json!({
        "usageId": uuid5_url_text(&format!("claude-otel:{request_id}")),
        "nativeSession": native_session(attributes, "session.id"),
        "nativeTurn": attributes.get("prompt.id").cloned().unwrap_or(Value::Null),
        "promptId": null,
        "inputTokens": input_tokens,
        "outputTokens": count(attributes.get("output_tokens")),
        "cachedInputTokens": cache_read,
        "reasoningTokens": null,
        "availability": "exact",
        "source": "claude_otel",
        "sourceEventId": request_id,
    }))
}

#[inline]
fn is_api_request(attributes: &OtelAttributes) -> bool {
    attributes
        .get("event.name")
        .and_then(Value::as_str)
        .is_some_and(|name| API_REQUEST_EVENTS.contains(&name))
}

/// `str(attrs.get("request_id") or attrs.get("client_request_id") or "")`, `None` when empty.
#[inline]
fn request_id(attributes: &OtelAttributes) -> Option<String> {
    let chosen = first_truthy(&[
        attributes.get("request_id"),
        attributes.get("client_request_id"),
    ])?;
    let text = python_str(Some(chosen));
    (!text.is_empty()).then_some(text)
}

/// `str(attrs.get(key) or "unknown")[:240]`.
#[inline]
pub fn native_session(attributes: &OtelAttributes, key: &str) -> String {
    let session = attributes
        .get(key)
        .filter(|value| is_truthy(value))
        .map_or_else(|| "unknown".to_owned(), |value| python_str(Some(value)));
    truncate_chars(&session, SESSION_CHAR_LIMIT)
}
