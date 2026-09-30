//! Port of tests/unit/test_usage_otel.py (Claude/Codex whitelists, dispatch, project fallback).

#[path = "otlp_fixtures.rs"]
mod otlp_fixtures;

use harness_services::otlp_receiver::adapter_dispatch::{adapter_for, UsageAdapter};
use harness_services::otlp_receiver::claude_usage::map_api_request;
use harness_services::otlp_receiver::codex_usage::map_response_completed;
use harness_services::otlp_receiver::otlp_decoding::{decode_log_records, OtelAttributes};
use harness_services::otlp_receiver::usage_records::usage_records;
use obs_cli::python_compat::uuid5_url_text;
use otlp_fixtures::{claude_api, codex_done, payload, PERSONAL_VALUES};
use serde_json::{json, Value};

fn decoded_attributes(service: &str, record: &Value) -> OtelAttributes {
    let decoded = decode_log_records(&payload(service, std::slice::from_ref(record))).unwrap();
    decoded[0].attributes.clone()
}

fn attributes(value: Value) -> OtelAttributes {
    value
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, item)| (key.clone(), item.clone()))
        .collect()
}

fn global_project() -> String {
    uuid5_url_text("codex-observability:global-agent-project")
}

#[test]
fn claude_maps_api_request() {
    let data = map_api_request(&decoded_attributes("claude-code", &claude_api())).unwrap();

    assert_eq!(data["nativeSession"], "sess-1");
    assert_eq!(data["nativeTurn"], "prompt-1");
    assert_eq!(data["source"], "claude_otel");
    assert_eq!(data["inputTokens"], 10 + 21480 + 36624);
    assert_eq!(data["outputTokens"], 193);
    assert_eq!(data["cachedInputTokens"], 21480);
    assert_eq!(data["reasoningTokens"], Value::Null);
    assert_eq!(data["usageId"], uuid5_url_text("claude-otel:req_1"));
    assert_eq!(data["sourceEventId"], "req_1");
    assert_eq!(data["availability"], "exact");
}

#[test]
fn claude_record_keys_keep_python_order() {
    let data = map_api_request(&decoded_attributes("claude-code", &claude_api())).unwrap();
    let keys: Vec<&str> = data
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();

    assert_eq!(
        keys,
        [
            "usageId",
            "nativeSession",
            "nativeTurn",
            "promptId",
            "inputTokens",
            "outputTokens",
            "cachedInputTokens",
            "reasoningTokens",
            "availability",
            "source",
            "sourceEventId"
        ]
    );
}

#[test]
fn claude_other_events_ignored() {
    assert!(map_api_request(&attributes(json!({"event.name": "user_prompt"}))).is_none());
    assert!(map_api_request(&attributes(
        json!({"event.name": "api_request", "session.id": "s"})
    ))
    .is_none());
}

#[test]
fn claude_usage_id_deterministic_and_client_request_fallback() {
    let attrs = decoded_attributes("claude-code", &claude_api());
    assert_eq!(
        map_api_request(&attrs).unwrap()["usageId"],
        map_api_request(&attrs.clone()).unwrap()["usageId"]
    );

    let fallback = attributes(json!({"event.name": "claude_code.api_request",
                                     "client_request_id": "c-9", "session.id": ""}));
    let data = map_api_request(&fallback).unwrap();
    assert_eq!(data["sourceEventId"], "c-9");
    assert_eq!(data["nativeSession"], "unknown");
}

#[test]
fn claude_session_is_truncated_to_240_characters() {
    let long_session = "é".repeat(300);
    let attrs = attributes(json!({"event.name": "api_request", "request_id": "r",
                                  "session.id": long_session}));
    let session = map_api_request(&attrs).unwrap()["nativeSession"].clone();

    assert_eq!(session.as_str().unwrap().chars().count(), 240);
}

#[test]
fn codex_maps_response_completed_with_mixed_types() {
    let data = map_response_completed(&decoded_attributes("codex_exec", &codex_done())).unwrap();

    assert_eq!(data["nativeSession"], "conv-1");
    assert_eq!(data["source"], "codex_otel");
    assert_eq!(data["nativeTurn"], Value::Null);
    assert_eq!(data["inputTokens"], 12581);
    assert_eq!(data["outputTokens"], 40);
    assert_eq!(data["cachedInputTokens"], 12000);
    assert_eq!(data["reasoningTokens"], 7);
    let source_event = "conv-1:2026-09-27T20:30:50.847Z:12581:40";
    assert_eq!(data["sourceEventId"], source_event);
    assert_eq!(
        data["usageId"],
        uuid5_url_text(&format!("codex-otel:{source_event}"))
    );
}

#[test]
fn codex_missing_fields_render_as_python_none() {
    let attrs =
        attributes(json!({"event.name": "codex.sse_event", "event.kind": "response.completed"}));
    let data = map_response_completed(&attrs).unwrap();

    assert_eq!(data["sourceEventId"], "unknown:None:None:None");
    assert_eq!(data["inputTokens"], 0);
}

#[test]
fn codex_other_events_ignored() {
    let created = json!({"event.name": "codex.sse_event", "event.kind": "response.created"});
    assert!(map_response_completed(&attributes(created)).is_none());
    assert!(
        map_response_completed(&attributes(json!({"event.name": "codex.api_request"}))).is_none()
    );
}

#[test]
fn codex_distinct_responses_get_distinct_ids() {
    let first = map_response_completed(&attributes(codex_done())).unwrap()["usageId"].clone();
    let mut later = codex_done();
    later["event.timestamp"] = json!("2026-09-27T20:30:51.000Z");
    let second = map_response_completed(&attributes(later)).unwrap()["usageId"].clone();

    assert_ne!(first, second);
}

#[test]
fn dispatch_by_service_name_prefix() {
    assert_eq!(adapter_for("codex_exec"), Some(UsageAdapter::Codex));
    assert_eq!(adapter_for("codex_cli_rs"), Some(UsageAdapter::Codex));
    assert_eq!(adapter_for("claude-code"), Some(UsageAdapter::Claude));
    assert_eq!(adapter_for("something-else"), None);
}

#[test]
fn operations_carry_project_and_no_personal_data() {
    let body = payload(
        "claude-code",
        &[claude_api(), json!({"event.name": "user_prompt"})],
    );
    let records = usage_records(&body, |_| Some("project-9".to_owned())).unwrap();

    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["projectId"], "project-9");
    let keys: Vec<&str> = records[0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys.last(), Some(&"projectId"));
    let text = serde_json::to_string(&records).unwrap();
    for personal in PERSONAL_VALUES {
        assert!(!text.contains(personal), "{personal} leaked");
    }
}

#[test]
fn unknown_service_and_malformed_payloads_yield_nothing() {
    let project = |_: &str| Some("p".to_owned());
    assert!(usage_records(&payload("other", &[claude_api()]), project)
        .unwrap()
        .is_empty());
    assert!(usage_records(&json!({"resourceLogs": "nope"}), project)
        .unwrap()
        .is_empty());
    assert!(usage_records(&json!({}), project).unwrap().is_empty());
    assert!(usage_records(&json!([1, 2]), project).unwrap().is_empty());
}

#[test]
fn python_raising_shapes_are_errors() {
    let project = |_: &str| Some("p".to_owned());
    assert!(usage_records(&json!({"resourceLogs": ["nope"]}), project).is_err());
    assert!(usage_records(&json!({"resourceLogs": [{"resource": null}]}), project).is_err());
}

#[test]
fn project_falls_back_to_global() {
    let none = usage_records(&payload("codex_exec", &[codex_done()]), |_| None).unwrap();
    let empty = usage_records(&payload("codex_exec", &[codex_done()]), |_| {
        Some(String::new())
    })
    .unwrap();

    assert_eq!(none[0]["projectId"], global_project());
    assert_eq!(empty[0]["projectId"], global_project());
}
