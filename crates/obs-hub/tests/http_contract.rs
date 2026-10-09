//! End-to-end HTTP contract of the hub over an in-memory database: one realistic operation
//! stream, replay idempotency, batch rollback, and every read route's response shape.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use obs_hub::{build_router, Database};
use serde_json::{json, Value};
use tower::ServiceExt;

const PROJECT: &str = "01a0d736-8ed8-76dc-9c20-c342229f9a82";
const AGENT: &str = "01928c5e-7d3a-7b2c-9d4e-000000000001";
const SESSION: &str = "01928c5e-7d3a-7b2c-9d4e-000000000002";
const PROMPT: &str = "01928c5e-7d3a-7b2c-9d4e-000000000003";
const EXECUTION: &str = "01928c5e-7d3a-7b2c-9d4e-000000000004";
const EVENT: &str = "01928c5e-7d3a-7b2c-9d4e-000000000005";
const ERROR_GROUP: &str = "01928c5e-7d3a-7b2c-9d4e-000000000006";
const USAGE: &str = "01928c5e-7d3a-7b2c-9d4e-000000000007";
const TOOL: &str = "01928c5e-7d3a-7b2c-9d4e-000000000008";
const HANDOFF: &str = "01928c5e-7d3a-7b2c-9d4e-000000000009";
const CONTEXT: &str = "01928c5e-7d3a-7b2c-9d4e-00000000000a";
const CODE_VERSION: &str = "01928c5e-7d3a-7b2c-9d4e-00000000000b";
const TRACE: &str = "0123456789abcdef0123456789abcdef";
const HASH: &str = "aa00000000000000000000000000000000000000000000000000000000000001";
const FINGERPRINT: &str = "bb00000000000000000000000000000000000000000000000000000000000002";

struct Hub {
    router: axum::Router,
    next_operation: u32,
}

impl Hub {
    fn new() -> Self {
        let database = Arc::new(Database::open_in_memory().expect("in-memory database"));
        Self {
            router: build_router(database),
            next_operation: 0,
        }
    }

    fn operation(&mut self, kind: &str, occurred_at: &str, data: Value) -> Value {
        self.next_operation += 1;
        json!({
            "operationId": format!("01928c5e-7d3a-7b2c-9d4e-{:012x}", 0x100000 + self.next_operation),
            "kind": kind,
            "occurredAt": occurred_at,
            "data": data,
        })
    }

    async fn post(&self, body: Value) -> (StatusCode, Value) {
        let request = Request::post("/v1/operations")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .expect("request");
        self.send(request).await
    }

    async fn get(&self, path: &str) -> (StatusCode, Value) {
        let request = Request::get(path).body(Body::empty()).expect("request");
        self.send(request).await
    }

    async fn send(&self, request: Request<Body>) -> (StatusCode, Value) {
        let response = self
            .router
            .clone()
            .oneshot(request)
            .await
            .expect("response");
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
}

fn realistic_stream(hub: &mut Hub) -> Vec<Value> {
    let t0 = "2026-09-30T01:00:00.000Z";
    let t1 = "2026-09-30T01:00:02.500Z";
    vec![
        hub.operation("project.register", t0, json!({
            "projectId": PROJECT, "displayName": "codex-system-harness", "slug": "codex-system-harness",
            "aliases": [{"kind": 1, "hash": HASH}]
        })),
        hub.operation("agent.register", t0, json!({
            "agentInstanceId": AGENT, "instanceKey": "claude", "sourceKind": 2, "displayName": "Claude"
        })),
        hub.operation("session.start", t0, json!({
            "sessionId": SESSION, "projectId": PROJECT, "agentInstanceId": AGENT, "sessionFingerprint": HASH
        })),
        hub.operation("prompt.begin", t0, json!({
            "promptId": PROMPT, "sessionId": SESSION, "ordinal": 1, "promptHmac": HASH, "rawPrompt": "gawin ito"
        })),
        hub.operation("code-version.register", t0, json!({
            "codeVersionId": CODE_VERSION, "projectId": PROJECT, "commitSha": "f143490", "branchName": "main"
        })),
        hub.operation("execution.start", t0, json!({
            "executionId": EXECUTION, "projectId": PROJECT, "sessionId": SESSION, "promptId": PROMPT,
            "traceId": TRACE, "codeVersionId": CODE_VERSION
        })),
        hub.operation("event.emit", t1, json!({
            "eventId": EVENT, "executionId": EXECUTION, "projectId": PROJECT, "code": "tool.failed",
            "severity": 4, "traceId": TRACE, "sequenceNo": 1, "component": "hook-router",
            "messageSummary": "tool failed", "attributes": {"tool": "Bash", "ratio": 2.0}
        })),
        hub.operation("error.record", t1, json!({
            "errorGroupId": ERROR_GROUP, "projectId": PROJECT, "fingerprint": FINGERPRINT,
            "exceptionClass": "ToolError", "normalizedSummary": "tool failed", "eventId": EVENT,
            "eventOccurredAt": t1, "handled": true
        })),
        hub.operation("execution.finish", t1, json!({"executionId": EXECUTION, "resultSummary": "done"})),
        hub.operation("prompt.native-link", t1, json!({
            "projectId": PROJECT, "nativeSession": "native-1", "nativeTurn": "turn-1", "promptId": PROMPT
        })),
        hub.operation("turn-usage.record", t1, json!({
            "usageId": USAGE, "projectId": PROJECT, "nativeSession": "native-1", "nativeTurn": "turn-1",
            "inputTokens": 1200, "outputTokens": 300, "availability": "exact", "source": "claude_hook",
            "sourceEventId": "evt-1"
        })),
        hub.operation("tool-use.record", t1, json!({
            "toolRecordId": TOOL, "projectId": PROJECT, "promptId": PROMPT, "nativeSession": "native-1",
            "toolUseId": "tu-1", "toolName": "Bash", "origin": "built-in", "durationMs": 41, "failed": true
        })),
        hub.operation("context.record", t1, json!({
            "contextId": CONTEXT, "projectId": PROJECT, "visionSummary": "Rust hub", "constraints": ["low RAM"]
        })),
        hub.operation("handoff.record", t1, json!({
            "handoffId": HANDOFF, "projectId": PROJECT, "handoffKind": "progress", "profileName": "personal",
            "content": "hub written", "baseVersion": 0
        })),
        hub.operation("prompt.finish", t1, json!({"promptId": PROMPT, "rawResult": "ok"})),
    ]
}

#[tokio::test]
async fn accepts_a_realistic_stream_and_answers_every_read_route() {
    let mut hub = Hub::new();
    let stream = realistic_stream(&mut hub);

    let (status, body) = hub.post(json!({"operations": stream})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({"accepted": 15}));

    let (status, replay) = hub.post(json!({"operations": stream})).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "replay must be idempotent: {replay}"
    );

    let (_, health) = hub.get("/health").await;
    assert_eq!(health, json!({"status": "ok", "schemaVersion": 1}));

    let (_, errors) = hub.get(&format!("/v1/projects/{PROJECT}/errors")).await;
    let group = &errors["items"][0];
    assert_eq!(group["fingerprint"], FINGERPRINT);
    assert_eq!(
        group["occurrence_count"], "2",
        "replayed error.record counts again, like Postgres"
    );
    assert_eq!(group["status"], 1);

    let (_, bootstrap) = hub
        .get(&format!(
            "/v1/projects/{PROJECT}/context/bootstrap?profile=cy"
        ))
        .await;
    assert_eq!(bootstrap["profile"], "cy");
    assert_eq!(bootstrap["source"], "startup");
    assert_eq!(bootstrap["context"]["version_no"], 1);
    assert_eq!(bootstrap["context"]["constraints"], json!(["low RAM"]));
    assert_eq!(bootstrap["handoffs"][0]["version_no"], "1");
    assert_eq!(bootstrap["code_version"]["branch_name"], "main");

    let (_, diagnosis) = hub.get(&format!("/v1/errors/{ERROR_GROUP}")).await;
    assert_eq!(
        diagnosis["errorGroup"]["project_name"],
        "codex-system-harness"
    );
    let occurrence = &diagnosis["occurrences"][0];
    assert_eq!(occurrence["handled"], true);
    assert_eq!(occurrence["trace_id"], TRACE);
    assert_eq!(occurrence["code_version_dirty"], false);
    assert_eq!(
        occurrence["attributes"],
        json!({"tool": "Bash", "ratio": 2})
    );

    let (_, prompt) = hub.get(&format!("/v1/prompts/{PROMPT}")).await;
    assert_eq!(prompt["prompt"]["raw_result"], "ok");
    assert_eq!(prompt["prompt"]["prompt_hmac"]["type"], "Buffer");
    assert_eq!(prompt["executions"][0]["status"], 2);

    let (_, trace) = hub.get(&format!("/v1/traces/{TRACE}")).await;
    assert_eq!(trace["items"][0]["code"], "tool.failed");
    assert_eq!(trace["items"][0]["sequence_no"], "1");
}

#[tokio::test]
async fn execution_duration_and_usage_linking_follow_postgres_semantics() {
    let mut hub = Hub::new();
    let stream = realistic_stream(&mut hub);
    hub.post(json!({"operations": stream})).await;

    let (_, analytics) = hub
        .get(&format!("/v1/projects/{PROJECT}/analytics?days=3650"))
        .await;
    assert_eq!(analytics["days"], 30);
    let bash = &analytics["tools"][0];
    assert_eq!(bash["calls"], "1");
    assert_eq!(bash["successes"], "0");
    assert_eq!(bash["failures"], "1");
    assert_eq!(bash["outcome_unknown"], "0");
    assert_eq!(bash["dynamic_discovery_calls"], "0");

    let (_, prompt) = hub.get(&format!("/v1/prompts/{PROMPT}")).await;
    let execution = &prompt["executions"][0];
    assert_eq!(execution["ended_at"], "2026-09-30T01:00:02.500Z");
}

#[tokio::test]
async fn a_failing_operation_rolls_back_the_whole_batch() {
    let mut hub = Hub::new();
    let register = hub.operation(
        "project.register",
        "2026-09-30T01:00:00Z",
        json!({
            "projectId": PROJECT, "displayName": "x", "slug": "valid-slug"
        }),
    );
    let orphan_session = hub.operation("session.start", "2026-09-30T01:00:00Z", json!({
        "sessionId": SESSION, "projectId": PROJECT, "agentInstanceId": AGENT, "sessionFingerprint": HASH
    }));

    let (status, body) = hub
        .post(json!({"operations": [register, orphan_session]}))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"]
        .as_str()
        .is_some_and(|message| !message.is_empty()));

    let (status, _) = hub
        .get(&format!("/v1/projects/{PROJECT}/context/bootstrap"))
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "project insert must have been rolled back"
    );
}

#[tokio::test]
async fn rejects_malformed_batches_like_zod_and_fastify() {
    let hub = Hub::new();

    let (status, body) = hub.post(json!({"operations": []})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid operation batch");
    assert_eq!(
        body["issues"][0],
        json!({"path": ["operations"], "code": "too_small"})
    );

    let (status, body) = hub.post(json!({"operations": [{"operationId": "x", "kind": "k", "occurredAt": "now", "data": {}}]})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["issues"].as_array().map(Vec::len), Some(3));

    let request = Request::post("/v1/operations")
        .header("content-type", "application/json")
        .body(Body::from("{not json"))
        .expect("request");
    let (status, body) = hub.send(request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["statusCode"], 400);

    let oversized = "x".repeat(obs_hub::routes::BODY_LIMIT_BYTES + 1);
    let request = Request::post("/v1/operations")
        .header("content-type", "application/json")
        .body(Body::from(oversized))
        .expect("request");
    let (status, _) = hub.send(request).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn unknown_routes_and_bad_identifiers_match_the_node_hub() {
    let hub = Hub::new();

    let (status, body) = hub.get("/nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["message"], "Route GET:/nope not found");

    let (status, body) = hub.get("/v1/traces/zz").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["message"], "expected 16-byte hex value");

    let (status, body) = hub.get("/v1/projects/not-a-uuid/analytics").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body, json!({"error": "invalid project id"}));

    let (status, body) = hub.get(&format!("/v1/prompts/{PROMPT}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, json!({"error": "prompt not found"}));
}
