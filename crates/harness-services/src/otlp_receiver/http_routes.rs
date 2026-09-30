//! receiver.py:82 `Handler`, byte-compatible replies: `GET /health` → 200 `{"status": "ok"}`,
//! `POST /v1/logs` → 413 `{}` when the declared body exceeds 10 MiB, else always 200 `{}`; any other
//! method/path → 404 `{}`. Bodies are `json.dumps` text (note the space after `:`), JSON content type.
//!
//! Deviations: a body without `Content-Length` (chunked) is read up to the limit instead of being
//! treated as empty; an unparsable `Content-Length` reads as 0 instead of dropping the connection;
//! unsupported methods get 404 `{}` instead of http.server's 501 page.

use std::sync::Arc;

use axum::body::{to_bytes, Bytes};
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use obs_cli::python_compat::parse_python_int;

use super::logs_ingestion::ingest_logs_body;
use super::{ReceiverState, MAX_BODY_BYTES};

pub const HEALTH_PATH: &str = "/health";
pub const LOGS_PATH: &str = "/v1/logs";
/// `json.dumps({"status": "ok"})`.
pub const HEALTH_BODY: &str = r#"{"status": "ok"}"#;
/// `json.dumps({})`.
pub const EMPTY_BODY: &str = "{}";
const JSON_CONTENT_TYPE: &str = "application/json";

/// One fallback handler so every method/path pair (including `GET /v1/logs`) follows Python's
/// exact-target dispatch instead of axum's 405 rules.
pub fn routes(state: Arc<ReceiverState>) -> Router {
    Router::new().fallback(dispatch).with_state(state)
}

async fn dispatch(State(state): State<Arc<ReceiverState>>, request: Request) -> Response {
    // Mental model: Python compares `self.path` (path + query) verbatim.
    let target = request
        .uri()
        .path_and_query()
        .map_or(request.uri().path(), |target| target.as_str());
    let is_health = request.method() == Method::GET && target == HEALTH_PATH;
    let is_logs = request.method() == Method::POST && target == LOGS_PATH;

    if is_health {
        return json_reply(StatusCode::OK, HEALTH_BODY);
    }
    if is_logs {
        return ingest_logs(state, request).await;
    }
    json_reply(StatusCode::NOT_FOUND, EMPTY_BODY)
}

async fn ingest_logs(state: Arc<ReceiverState>, request: Request) -> Response {
    // Mental model: declared-size gate (413) → bounded body read → blocking ingest → drain signal → 200.
    if declared_length(&request) > MAX_BODY_BYTES as i64 {
        return json_reply(StatusCode::PAYLOAD_TOO_LARGE, EMPTY_BODY);
    }
    match to_bytes(request.into_body(), MAX_BODY_BYTES).await {
        Ok(body) => record_and_signal_flush(state, body).await,
        Err(error) => report_failure(&format!("BodyReadError: {error}")),
    }
    json_reply(StatusCode::OK, EMPTY_BODY)
}

/// Runs the SQLite work off the async workers; requests a drain flush when ≥1 record was enqueued.
async fn record_and_signal_flush(state: Arc<ReceiverState>, body: Bytes) {
    let worker_state = Arc::clone(&state);
    let outcome =
        tokio::task::spawn_blocking(move || ingest_logs_body(&worker_state.context, &body)).await;
    match outcome {
        Ok(Ok(0)) => {}
        Ok(Ok(_enqueued)) => state.flush.request_flush(),
        Ok(Err(failure)) => report_failure(&failure.to_string()),
        Err(join_error) => report_failure(&format!("IngestWorkerError: {join_error}")),
    }
}

/// `int(self.headers.get("Content-Length") or 0)`; unparsable → 0.
#[inline]
fn declared_length(request: &Request) -> i64 {
    request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_python_int)
        .unwrap_or(0)
}

/// receiver.py:104 writes only a short failure name to stderr; never to the client.
#[inline]
fn report_failure(detail: &str) {
    eprintln!("usage receiver: {detail}");
}

#[inline]
fn json_reply(status: StatusCode, body: &'static str) -> Response {
    (
        status,
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static(JSON_CONTENT_TYPE),
        )],
        body,
    )
        .into_response()
}
