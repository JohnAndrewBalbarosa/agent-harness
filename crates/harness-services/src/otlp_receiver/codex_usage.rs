//! adapters/codex/otel.py `map`: Codex `codex.sse_event` of kind `response.completed` →
//! `turn-usage.record` data. Codex sends no request id, so the dedupe key is
//! `session:event.timestamp:input:output` (raw attribute text, `None` when absent). Personal fields
//! (`user.email`, `user.account_id`) are never read.

use obs_cli::python_compat::uuid5_url_text;
use serde_json::{json, Value};

use super::claude_usage::native_session;
use super::otlp_decoding::OtelAttributes;
use super::python_semantics::{count, python_str};

pub const SSE_EVENT_NAME: &str = "codex.sse_event";
pub const COMPLETED_EVENT_KIND: &str = "response.completed";

pub fn map_response_completed(attributes: &OtelAttributes) -> Option<Value> {
    // Mental model: event filter → session + source-event text (dedupe key) → record in Python order.
    if !is_response_completed(attributes) {
        return None;
    }
    let session = native_session(attributes, "conversation.id");
    let source_event = source_event_text(&session, attributes);

    Some(json!({
        "usageId": uuid5_url_text(&format!("codex-otel:{source_event}")),
        "nativeSession": session,
        "nativeTurn": attributes.get("turn.id").cloned().unwrap_or(Value::Null),
        "promptId": null,
        "inputTokens": count(attributes.get("input_token_count")),
        "outputTokens": count(attributes.get("output_token_count")),
        "cachedInputTokens": count(attributes.get("cached_token_count")),
        "reasoningTokens": count(attributes.get("reasoning_token_count")),
        "availability": "exact",
        "source": "codex_otel",
        "sourceEventId": source_event,
    }))
}

#[inline]
fn is_response_completed(attributes: &OtelAttributes) -> bool {
    let text_of = |key: &str| attributes.get(key).and_then(Value::as_str);
    text_of("event.name") == Some(SSE_EVENT_NAME)
        && text_of("event.kind") == Some(COMPLETED_EVENT_KIND)
}

/// otel.py:19 `f"{session}:{event.timestamp}:{input_token_count}:{output_token_count}"`.
#[inline]
fn source_event_text(session: &str, attributes: &OtelAttributes) -> String {
    format!(
        "{session}:{}:{}:{}",
        python_str(attributes.get("event.timestamp")),
        python_str(attributes.get("input_token_count")),
        python_str(attributes.get("output_token_count")),
    )
}
