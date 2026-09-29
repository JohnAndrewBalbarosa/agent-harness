//! The `turn-usage.record` payload (`core/usage/fallback.py:build_record`) and its local-log extension.

use chrono::{DateTime, Utc};
use obs_cli::python_compat::uuid5_url_text;
use serde_json::{json, Value};

use crate::events::Event;
use crate::transcripts::TurnUsage;

/// `obs_client.GLOBAL_PROJECT_ID` name: sessions outside registered repositories.
const GLOBAL_PROJECT_NAME: &str = "codex-observability:global-agent-project";

/// `str(uuid.uuid5(uuid.NAMESPACE_URL, "codex-observability:global-agent-project"))`.
pub fn global_project_id() -> String {
    uuid5_url_text(GLOBAL_PROJECT_NAME)
}

/// `build_record(event, usage, project_id)`, keys in Python order.
pub fn build_record(event: &Event, usage: &TurnUsage, project_id: &str) -> Value {
    let turn = usage
        .turn_id
        .as_deref()
        .filter(|turn| !turn.is_empty())
        .or(event.turn_id.as_deref());
    let turn_text = turn.unwrap_or("None");
    let availability = if usage.api_calls > 0 {
        "exact"
    } else {
        "unavailable"
    };
    json!({
        "usageId": uuid5_url_text(&format!("{}-hook:{}:{turn_text}", event.agent, event.session_id)),
        "projectId": project_id,
        "nativeSession": event.session_id,
        "nativeTurn": turn,
        "promptId": Value::Null,
        "inputTokens": usage.input_tokens,
        "outputTokens": usage.output_tokens,
        "cachedInputTokens": usage.cached_input_tokens,
        "reasoningTokens": usage.reasoning_tokens,
        "availability": availability,
        "source": format!("{}_hook", event.agent),
        "sourceEventId": format!("{}:{turn_text}:{}", event.session_id, event.native_event),
    })
}

/// `{**record, "recordedAt", "apiCalls", "agent", "instance", "enqueued"}` for `turn-usage.jsonl`.
pub fn local_log_record(
    record: &Value,
    event: &Event,
    usage: &TurnUsage,
    enqueued: bool,
    recorded_at: DateTime<Utc>,
) -> Value {
    let mut extended = record.as_object().cloned().unwrap_or_default();
    extended.insert(
        "recordedAt".into(),
        Value::String(python_isoformat_utc(recorded_at)),
    );
    extended.insert("apiCalls".into(), json!(usage.api_calls));
    extended.insert("agent".into(), Value::String(event.agent.clone()));
    extended.insert("instance".into(), Value::String(event.instance.clone()));
    extended.insert("enqueued".into(), Value::Bool(enqueued));
    Value::Object(extended)
}

/// `datetime.now(timezone.utc).isoformat()`: `+00:00` suffix, microseconds omitted when zero.
pub fn python_isoformat_utc(instant: DateTime<Utc>) -> String {
    if instant.timestamp_subsec_micros() == 0 {
        instant.format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
    } else {
        instant.format("%Y-%m-%dT%H:%M:%S%.6f+00:00").to_string()
    }
}
