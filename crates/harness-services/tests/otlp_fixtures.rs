//! Shared OTLP fixtures (port of tests/unit/test_usage_otel.py `kv`, `payload`, `CLAUDE_API`,
//! `CODEX_DONE`). Included by the other `otlp_*` test crates via `#[path]`.
#![allow(dead_code)]

use serde_json::{json, Value};

/// `kv(key, value)`: bool → boolValue, int → intValue (string), float → doubleValue, else stringValue.
pub fn kv(key: &str, value: &Value) -> Value {
    let any_value = match value {
        Value::Bool(flag) => json!({"boolValue": flag}),
        Value::Number(number) if number.is_i64() || number.is_u64() => {
            json!({"intValue": number.to_string()})
        }
        Value::Number(number) => json!({"doubleValue": number}),
        other => json!({"stringValue": other}),
    };
    json!({"key": key, "value": any_value})
}

/// `payload(service, *records)`.
pub fn payload(service: &str, records: &[Value]) -> Value {
    let log_records: Vec<Value> = records
        .iter()
        .map(|record| {
            let attributes: Vec<Value> = record
                .as_object()
                .expect("fixture record is an object")
                .iter()
                .map(|(key, value)| kv(key, value))
                .collect();
            json!({"attributes": attributes})
        })
        .collect();
    json!({"resourceLogs": [{
        "resource": {"attributes": [kv("service.name", &json!(service))]},
        "scopeLogs": [{"logRecords": log_records}],
    }]})
}

pub fn claude_api() -> Value {
    json!({"event.name": "api_request", "session.id": "sess-1", "prompt.id": "prompt-1",
           "model": "claude-opus-5-5", "input_tokens": 10, "output_tokens": 193,
           "cache_read_tokens": 21480, "cache_creation_tokens": 36624, "request_id": "req_1",
           "user.email": "someone@example.com", "user.id": "u-1", "organization.id": "org-1"})
}

pub fn codex_done() -> Value {
    json!({"event.name": "codex.sse_event", "event.kind": "response.completed",
           "input_token_count": "12581", "output_token_count": "40", "cached_token_count": 12000,
           "cache_write_token_count": 0, "reasoning_token_count": 7, "conversation.id": "conv-1",
           "event.timestamp": "2026-09-27T20:30:50.847Z", "model": "gpt-6-astra",
           "user.email": "someone@example.com", "user.account_id": "acct-1"})
}

pub const PERSONAL_VALUES: [&str; 4] = ["someone@example.com", "u-1", "org-1", "acct-1"];
