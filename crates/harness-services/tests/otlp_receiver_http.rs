//! Port of tests/unit/test_usage_receiver_http.py plus the byte-level reply contract, driven through
//! the axum router (`tower::ServiceExt::oneshot`) against a temporary spool.

#[path = "otlp_fixtures.rs"]
mod otlp_fixtures;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use harness_services::otlp_receiver::exclusive_bind::bind_exclusive;
use harness_services::otlp_receiver::{listen_port, router_with_flush_requester, FlushRequester};
use harness_services::service_context::ServiceContext;
use http_body_util::BodyExt;
use obs_cli::python_compat::uuid5_url_text;
use otlp_fixtures::{claude_api, payload, PERSONAL_VALUES};
use tower::ServiceExt;

#[derive(Default)]
struct CountingFlush(AtomicUsize);

impl FlushRequester for CountingFlush {
    fn request_flush(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct Harness {
    _root: tempfile::TempDir,
    spool: PathBuf,
    context: ServiceContext,
    flush: Arc<CountingFlush>,
}

fn harness() -> Harness {
    let root = tempfile::tempdir().unwrap();
    let spool = root.path().join("var").join("spool.sqlite3");
    let env = BTreeMap::from([(
        "AH_OBS_SPOOL".to_owned(),
        spool.to_string_lossy().into_owned(),
    )]);
    let context = ServiceContext {
        harness_root: root.path().to_path_buf(),
        env,
    };
    Harness {
        _root: root,
        spool,
        context,
        flush: Arc::new(CountingFlush::default()),
    }
}

async fn send(harness: &Harness, request: Request<Body>) -> (StatusCode, String, String) {
    let app = router_with_flush_requester(harness.context.clone(), harness.flush.clone());
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .map(|value| value.to_str().unwrap().to_owned())
        .unwrap_or_default();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        content_type,
        String::from_utf8(body.to_vec()).unwrap(),
    )
}

fn post(path: &str, body: Vec<u8>) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CONTENT_LENGTH, body.len())
        .body(Body::from(body))
        .unwrap()
}

fn get(path: &str) -> Request<Body> {
    Request::builder().uri(path).body(Body::empty()).unwrap()
}

fn queued_payloads(spool: &Path) -> Vec<serde_json::Value> {
    if !spool.exists() {
        return Vec::new();
    }
    let conn = rusqlite::Connection::open(spool).unwrap();
    let mut statement = conn
        .prepare("SELECT payload FROM queue ORDER BY created_at")
        .unwrap();
    let mut payloads = Vec::new();
    for text in statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
    {
        payloads.push(serde_json::from_str(&text.unwrap()).unwrap());
    }
    payloads
}

fn claude_body() -> Vec<u8> {
    serde_json::to_vec(&payload("claude-code", &[claude_api()])).unwrap()
}

#[tokio::test]
async fn health_replies_python_bytes() {
    let harness = harness();
    let (status, content_type, body) = send(&harness, get("/health")).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "application/json");
    assert_eq!(body, r#"{"status": "ok"}"#);
}

#[tokio::test]
async fn logs_are_enqueued_without_personal_data() {
    let harness = harness();
    let (status, _, body) = send(&harness, post("/v1/logs", claude_body())).await;

    assert_eq!((status, body.as_str()), (StatusCode::OK, "{}"));
    let queued = queued_payloads(&harness.spool);
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0]["kind"], "turn-usage.record");
    let data = &queued[0]["data"];
    assert_eq!(
        data["projectId"],
        uuid5_url_text("codex-observability:global-agent-project")
    );
    assert_eq!(data["inputTokens"], 10 + 21480 + 36624);
    let text = serde_json::to_string(&queued).unwrap();
    for personal in PERSONAL_VALUES {
        assert!(!text.contains(personal), "{personal} leaked");
    }
    assert_eq!(harness.flush.0.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn project_comes_from_the_spool_context_table() {
    let harness = harness();
    let conn = obs_cli::spool::open::open_spool_at(&harness.spool).unwrap();
    conn.execute(
        "INSERT INTO context(native_session,project_id,session_id,cwd,updated_at) VALUES('sess-1','project-9','s','/','t')",
        [],
    )
    .unwrap();
    drop(conn);

    send(&harness, post("/v1/logs", claude_body())).await;

    assert_eq!(
        queued_payloads(&harness.spool)[0]["data"]["projectId"],
        "project-9"
    );
}

#[tokio::test]
async fn garbage_body_still_returns_200() {
    let harness = harness();
    let (status, content_type, body) = send(&harness, post("/v1/logs", b"not json".to_vec())).await;

    assert_eq!((status, body.as_str()), (StatusCode::OK, "{}"));
    assert_eq!(content_type, "application/json");
    assert!(queued_payloads(&harness.spool).is_empty());
    assert_eq!(harness.flush.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unmapped_events_do_not_request_a_flush() {
    let harness = harness();
    let body = serde_json::to_vec(&payload("other-service", &[claude_api()])).unwrap();
    let (status, _, _) = send(&harness, post("/v1/logs", body)).await;

    assert_eq!(status, StatusCode::OK);
    assert!(queued_payloads(&harness.spool).is_empty());
    assert_eq!(harness.flush.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn oversized_declared_body_is_413() {
    let harness = harness();
    let request = Request::builder()
        .method("POST")
        .uri("/v1/logs")
        .header(header::CONTENT_LENGTH, 10 * 1024 * 1024 + 1)
        .body(Body::empty())
        .unwrap();
    let (status, _, body) = send(&harness, request).await;

    assert_eq!(
        (status, body.as_str()),
        (StatusCode::PAYLOAD_TOO_LARGE, "{}")
    );
}

#[tokio::test]
async fn other_paths_and_methods_are_404() {
    let harness = harness();
    for request in [
        get("/"),
        get("/v1/logs"),
        get("/health?probe=1"),
        post("/v1/traces", b"{}".to_vec()),
        post("/health", b"{}".to_vec()),
    ] {
        let (status, content_type, body) = send(&harness, request).await;
        assert_eq!((status, body.as_str()), (StatusCode::NOT_FOUND, "{}"));
        assert_eq!(content_type, "application/json");
    }
}

#[test]
fn port_comes_from_the_harness_env_file() {
    let root = tempfile::tempdir().unwrap();
    let env_file = root.path().join(".env");
    std::fs::write(&env_file, "OTLP_PORT=4399\n").unwrap();
    let env = harness_hook::runtime_env::runtime_env(&env_file, &BTreeMap::new());
    let context = ServiceContext {
        harness_root: root.path().to_path_buf(),
        env,
    };

    assert_eq!(listen_port(&context).unwrap(), 4399);
}

#[test]
fn port_defaults_to_4320_and_rejects_garbage() {
    let mut context = ServiceContext {
        harness_root: PathBuf::from("."),
        env: BTreeMap::new(),
    };
    assert_eq!(listen_port(&context).unwrap(), 4320);

    context
        .env
        .insert("OTLP_PORT".to_owned(), "not-a-port".to_owned());
    assert!(listen_port(&context).is_err());
}

#[test]
fn second_server_cannot_bind_the_same_port() {
    let first = bind_exclusive(0).unwrap();
    let port = first.local_addr().unwrap().port();

    assert!(bind_exclusive(port).is_err());
}
