//! The two data shapes hook.py builds repeatedly: `event.emit` (execution-scoped signal) and
//! `error.record` (policy violation grouped by fingerprint). Key order matches hook.py's dict literals.

use obs_cli::python_compat::uuid5_url_text;
use serde_json::{json, Map, Value};

use super::fingerprints::sha256_hex;

const COMPONENT: &str = "codex";
const POLICY_TOP_FRAME: &str = "hook.py:PostToolUse";

/// Which execution/trace an event belongs to (row values are passed through as-is, `None` → null).
pub struct TraceSlot<'a> {
    pub execution_id: &'a Value,
    pub project_id: &'a Value,
    pub trace_id: &'a Value,
}

/// One `event.emit` signal.
pub struct Signal<'a> {
    pub code: &'a str,
    pub category: i64,
    pub severity: i64,
    pub outcome: i64,
    pub summary: String,
    pub attributes: Value,
}

/// `{"eventId", "executionId", "projectId", "traceId", "sequenceNo", "code", "category", "severity",
/// "outcome", "component", "messageSummary", "attributes"}`; callers append `rawPayload` when used.
pub fn event_emit_data(
    event_id: &str,
    slot: &TraceSlot,
    sequence_no: i64,
    signal: Signal,
) -> Map<String, Value> {
    let data = json!({
        "eventId": event_id,
        "executionId": slot.execution_id,
        "projectId": slot.project_id,
        "traceId": slot.trace_id,
        "sequenceNo": sequence_no,
        "code": signal.code,
        "category": signal.category,
        "severity": signal.severity,
        "outcome": signal.outcome,
        "component": COMPONENT,
        "messageSummary": signal.summary,
        "attributes": signal.attributes,
    });
    match data {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// A policy violation raised from PostToolUse.
pub struct PolicyViolation<'a> {
    /// Text hashed (unkeyed) into the error-group fingerprint.
    pub fingerprint_source: String,
    pub exception_class: &'a str,
    pub summary: &'a str,
    pub event_id: &'a str,
    pub observed_at: &'a str,
}

/// `error.record` data: `errorGroupId = uuid5(URL, fingerprint)`, `fingerprint = sha256(source)`.
pub fn error_record_data(project_id: &Value, violation: &PolicyViolation) -> Value {
    let fingerprint = sha256_hex(&violation.fingerprint_source);
    json!({
        "errorGroupId": uuid5_url_text(&fingerprint),
        "projectId": project_id,
        "fingerprint": fingerprint,
        "exceptionClass": violation.exception_class,
        "normalizedSummary": violation.summary,
        "eventId": violation.event_id,
        "eventOccurredAt": violation.observed_at,
        "handled": false,
        "retryable": false,
        "topFrame": POLICY_TOP_FRAME,
    })
}
