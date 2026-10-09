//! PostToolUse on a live execution (hook.py `elif event == "PostToolUse"`): the `tool-use.record`
//! analytics row, the `agent.tool.completed` event (raw input/response kept losslessly in `rawPayload`),
//! then the Taglish check and the behaviour audit, each advancing the execution's sequence number.

use chrono::{DateTime, NaiveDate, NaiveDateTime};
use obs_cli::python_compat::{
    parse_python_int, python_utc_now, truncate_chars, uuid5_url_text, uuid7_text,
};
use rusqlite::params;
use serde_json::{json, Map, Value};

use super::behaviour_audit::{behaviour_operations, observe_tool, ToolCounters, ToolObservation};
use super::event_records::{event_emit_data, Signal, TraceSlot};
use super::hook_scope::HookScope;
use super::language_policy::{communication_language_mismatches, mismatch_operations};
use super::operations::operation;
use super::payload_fields::{python_or, text_or, Payload};
use super::project_registration::ProjectIdentity;
use super::session_chain::ContextRow;
use super::spool_store::Spool;
use crate::events::is_python_truthy;

const TOOL_ID_LIMIT: usize = 240;
const TOOL_NAME_LIMIT: usize = 160;
const STARTED_AT_LIMIT: usize = 40;
const MAX_DURATION_MS: i64 = 86_400_000;
const BUILT_IN_TOOLS: [&str; 4] = ["bash", "shell", "exec_command", "apply_patch"];
const ISO_OFFSET_FORMATS: [&str; 4] = [
    "%Y-%m-%dT%H:%M:%S%.f%:z",
    "%Y-%m-%d %H:%M:%S%.f%:z",
    "%Y-%m-%dT%H:%M%:z",
    "%Y-%m-%d %H:%M%:z",
];
const ISO_NAIVE_FORMATS: [&str; 4] = [
    "%Y-%m-%dT%H:%M:%S%.f",
    "%Y-%m-%d %H:%M:%S%.f",
    "%Y-%m-%dT%H:%M",
    "%Y-%m-%d %H:%M",
];

/// Mental model: analytics + completion event → language check → behaviour audit → persist counters.
pub fn record_tool_use(
    scope: &HookScope,
    spool: &Spool,
    project: &ProjectIdentity,
    row: &ContextRow,
    operations: &mut Vec<Value>,
) -> anyhow::Result<()> {
    let observation = observe_tool(scope.payload);
    let execution_id = row.execution_id_json();
    let trace_id = row.trace_id_json();
    let slot = TraceSlot {
        execution_id: &execution_id,
        project_id: &project.id,
        trace_id: &trace_id,
    };
    let mut counters = ToolCounters {
        sequence_no: row.sequence_no + 1,
        wait_count: row.wait_count,
        background_count: row.background_count,
    };

    operations.extend(completion_operations(
        scope,
        project,
        row,
        &slot,
        &observation,
        counters.sequence_no,
    ));

    let mismatches = communication_language_mismatches(scope.payload, &observation.tool);
    if !mismatches.is_empty() {
        counters.sequence_no += 1;
        operations.extend(mismatch_operations(
            &slot,
            counters.sequence_no,
            &mismatches,
        ));
    }
    if row.behavior_audit_enabled != 0 {
        operations.extend(behaviour_operations(
            scope.context,
            &slot,
            &observation,
            &mut counters,
        ));
    }

    spool.conn().execute(
        "UPDATE context SET sequence_no=?,wait_count=?,background_count=?,updated_at=? WHERE native_session=?",
        params![counters.sequence_no, counters.wait_count, counters.background_count, python_utc_now(), scope.native_session],
    )?;
    Ok(())
}

/// `tool-use.record` then `agent.tool.completed`.
fn completion_operations(
    scope: &HookScope,
    project: &ProjectIdentity,
    row: &ContextRow,
    slot: &TraceSlot,
    observation: &ToolObservation,
    sequence_no: i64,
) -> Vec<Value> {
    let analytics = tool_analytics(scope.payload, &observation.tool, &python_utc_now());
    let failed = analytics.get("failed").is_some_and(is_python_truthy);
    let tool_identity = match analytics.get("toolUseId").and_then(Value::as_str) {
        Some(tool_use_id) => tool_use_id.to_owned(),
        None => format!("sequence:{sequence_no}"),
    };

    let mut record = analytics.clone();
    record.insert(
        "toolRecordId".into(),
        json!(uuid5_url_text(&format!(
            "tool:{}:{}:{tool_identity}",
            project.id_text, scope.native_session
        ))),
    );
    record.insert("projectId".into(), project.id.clone());
    record.insert("promptId".into(), row.prompt_id_json());
    record.insert("executionId".into(), row.execution_id_json());
    record.insert("nativeSession".into(), json!(scope.native_session));
    record.insert("nativeTurn".into(), row.native_turn_json());

    let signal = Signal {
        code: "agent.tool.completed",
        category: 3,
        severity: if failed { 4 } else { 1 },
        outcome: if failed { 3 } else { 1 },
        summary: format!("Tool completed: {}", observation.tool),
        attributes: json!({
            "tool": observation.tool,
            "origin": analytics_field(&analytics, "origin"),
            "durationMs": analytics_field(&analytics, "durationMs"),
            "outcomeStatus": analytics_field(&analytics, "outcomeStatus"),
            "cacheStatus": analytics_field(&analytics, "cacheStatus"),
            "discoveryMode": analytics_field(&analytics, "discoveryMode"),
        }),
    };
    let mut event = event_emit_data(&uuid7_text(), slot, sequence_no, signal);
    event.insert(
        "rawPayload".into(),
        json!({
            "toolInput": python_or(scope.payload, &["tool_input", "toolInput"]),
            "toolResponse": python_or(scope.payload, &["tool_response", "toolResponse"]),
        }),
    );
    vec![
        operation("tool-use.record", Value::Object(record), None),
        operation("event.emit", Value::Object(event), None),
    ]
}

/// hook.py `tool_analytics(value, tool, observed_at)`: bounded, content-free tool facts.
pub fn tool_analytics(payload: &Payload, tool: &str, observed_at: &str) -> Map<String, Value> {
    let response = response_fields(python_or(payload, &["tool_response", "toolResponse"]));
    let tool_id = text_or(payload, &["tool_use_id", "toolUseId"], "", TOOL_ID_LIMIT);
    let parent_id = text_or(payload, &["parent_tool_use_id"], "", TOOL_ID_LIMIT);
    let origin = tool_origin(tool);
    let failed = failure_flag(&response);
    let cache_hit = response
        .get("cache_hit")
        .or_else(|| response.get("cacheHit"))
        .and_then(Value::as_bool);

    let analytics = json!({
        "toolUseId": non_empty(tool_id),
        "toolName": truncate_chars(tool, TOOL_NAME_LIMIT),
        "origin": origin,
        "parentToolUseId": non_empty(parent_id),
        "startedAt": started_at(payload, &response),
        "completedAt": observed_at,
        "durationMs": duration_ms(payload, &response),
        "failed": failed,
        "outcomeStatus": match failed { Some(true) => "failure", Some(false) => "success", None => "unknown" },
        "cacheHit": cache_hit,
        "cacheStatus": match cache_hit { Some(true) => "hit", Some(false) => "miss", None => "unknown" },
        "discoveryMode": discovery_mode(tool),
    });
    match analytics {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// Structured objects are used directly. A string is decoded only when the whole value is JSON.
/// Ordinary prose remains opaque so words such as "error" cannot create false failures.
fn response_fields(response: Value) -> Map<String, Value> {
    match response {
        Value::Object(map) => map,
        Value::String(text) => serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default(),
        _ => Map::new(),
    }
}

#[inline]
fn discovery_mode(tool: &str) -> &'static str {
    let normalized = tool.to_lowercase().replace(['.', '_'], "");
    if normalized == "toolsearch" || normalized == "functionstoolsearch" {
        "dynamic"
    } else {
        "direct"
    }
}

/// `mcp` / `harness` / `built-in` / `unknown` by name prefix.
pub fn tool_origin(tool: &str) -> &'static str {
    let lower = tool.to_lowercase();
    if lower.starts_with("mcp__") || lower.starts_with("mcp.") {
        "mcp"
    } else if lower.starts_with("harness.") || lower.starts_with("ah_") {
        "harness"
    } else if lower.starts_with("functions.")
        || lower.starts_with("functions__")
        || BUILT_IN_TOOLS.contains(&lower.as_str())
    {
        "built-in"
    } else {
        "unknown"
    }
}

/// `is_error` (bool) → `isError` (bool) → `exit_code not in (None, 0)` when present → `True if error else None`.
fn failure_flag(response: &Map<String, Value>) -> Option<bool> {
    let explicit = response
        .get("is_error")
        .and_then(Value::as_bool)
        .or_else(|| response.get("isError").and_then(Value::as_bool));
    if explicit.is_some() {
        return explicit;
    }
    match response.get("exit_code") {
        Some(exit_code) => Some(!is_python_zero_or_none(exit_code)),
        None => response
            .get("error")
            .is_some_and(is_python_truthy)
            .then_some(true),
    }
}

/// `value in (None, 0)` (Python: `0 == 0.0 == False`).
#[inline]
fn is_python_zero_or_none(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Bool(flag) => !flag,
        Value::Number(number) => number.as_f64() == Some(0.0),
        _ => false,
    }
}

/// `value.get("duration_ms", response.get("duration_ms"))` as `int()` within 0..=86_400_000, else None.
fn duration_ms(payload: &Payload, response: &Map<String, Value>) -> Option<i64> {
    let raw = match payload.get("duration_ms") {
        Some(value) => value,
        None => response.get("duration_ms")?,
    };
    let whole = match raw {
        Value::Bool(flag) => i64::from(*flag),
        Value::Number(number) => number.as_i64().or_else(|| {
            number
                .as_f64()
                .filter(|float| float.is_finite())
                .map(|float| float.trunc() as i64)
        })?,
        Value::String(text) => parse_python_int(text)?,
        _ => return None,
    };
    (0..=MAX_DURATION_MS).contains(&whole).then_some(whole)
}

/// `value.get("started_at") or response.get("started_at")`, kept only when it is an ISO string ≤ 40 chars.
fn started_at(payload: &Payload, response: &Map<String, Value>) -> Option<String> {
    let candidate = payload
        .get("started_at")
        .filter(|value| is_python_truthy(value))
        .or_else(|| response.get("started_at"))?;
    let text = candidate.as_str()?;
    (text.chars().count() <= STARTED_AT_LIMIT && is_python_isoformat(text)).then(|| text.to_owned())
}

/// `datetime.fromisoformat(text.replace("Z", "+00:00"))` succeeds (common extended forms).
fn is_python_isoformat(text: &str) -> bool {
    let offset_text = text.replace('Z', "+00:00");
    ISO_OFFSET_FORMATS
        .iter()
        .any(|format| DateTime::parse_from_str(&offset_text, format).is_ok())
        || ISO_NAIVE_FORMATS
            .iter()
            .any(|format| NaiveDateTime::parse_from_str(&offset_text, format).is_ok())
        || NaiveDate::parse_from_str(&offset_text, "%Y-%m-%d").is_ok()
}

#[inline]
fn analytics_field(analytics: &Map<String, Value>, key: &str) -> Value {
    analytics.get(key).cloned().unwrap_or(Value::Null)
}

#[inline]
fn non_empty(text: String) -> Option<String> {
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(value: Value) -> Payload {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn tool_record_redacts_payload_and_preserves_explicit_signals() {
        let record = tool_analytics(
            &payload(
                json!({"tool_use_id": "tool-1", "tool_response": {"is_error": true, "secret": "private"}, "duration_ms": 17}),
            ),
            "mcp__calendar__list",
            "2026-09-29T00:00:00Z",
        );
        assert_eq!(record["toolUseId"], "tool-1");
        assert_eq!(record["origin"], "mcp");
        assert_eq!(record["durationMs"], 17);
        assert_eq!(record["failed"], true);
        assert_eq!(record["outcomeStatus"], "failure");
        assert_eq!(record["cacheStatus"], "unknown");
        assert_eq!(record["discoveryMode"], "direct");
        assert!(!Value::Object(record).to_string().contains("private"));

        let cached = tool_analytics(
            &payload(json!({"tool_response": {"cache_hit": true}})),
            "harness.cache",
            "2026-09-29T00:00:00Z",
        );
        assert_eq!(
            (cached["cacheHit"].clone(), cached["cacheStatus"].clone()),
            (json!(true), json!("hit"))
        );

        let decoded = tool_analytics(
            &payload(json!({"tool_response": "{\"exit_code\":2,\"cacheHit\":false}"})),
            "functions.tool_search",
            "2026-09-29T00:00:00Z",
        );
        assert_eq!(decoded["outcomeStatus"], "failure");
        assert_eq!(decoded["cacheStatus"], "miss");
        assert_eq!(decoded["discoveryMode"], "dynamic");

        let opaque = tool_analytics(
            &payload(json!({"tool_response": "error appears in ordinary output"})),
            "Bash",
            "2026-09-29T00:00:00Z",
        );
        assert_eq!(opaque["outcomeStatus"], "unknown");
    }

    #[test]
    fn failure_falls_back_to_exit_code_then_error() {
        let failed = |response: Value| {
            tool_analytics(&payload(json!({"tool_response": response})), "bash", "t")["failed"]
                .clone()
        };
        assert_eq!(failed(json!({"exit_code": 0})), json!(false));
        assert_eq!(failed(json!({"exit_code": 2})), json!(true));
        assert_eq!(failed(json!({"exit_code": null})), json!(false));
        assert_eq!(failed(json!({"error": "boom"})), json!(true));
        assert_eq!(failed(json!({"isError": false})), json!(false));
        assert_eq!(failed(json!({})), Value::Null);
    }

    #[test]
    fn duration_and_start_time_are_bounded() {
        let record = tool_analytics(
            &payload(json!({"duration_ms": 90_000_000, "started_at": "2026-09-29T00:00:00Z"})),
            "Bash",
            "t",
        );
        assert_eq!(record["durationMs"], Value::Null);
        assert_eq!(record["startedAt"], "2026-09-29T00:00:00Z");
        assert_eq!(record["origin"], "built-in");
        let record = tool_analytics(
            &payload(json!({"duration_ms": "12", "started_at": "yesterday"})),
            "x",
            "t",
        );
        assert_eq!(record["durationMs"], 12);
        assert_eq!(record["startedAt"], Value::Null);
    }
}
