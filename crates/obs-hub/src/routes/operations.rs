//! `POST /v1/operations`: parse → validate → apply the whole batch in one transaction.

use std::time::Instant;

use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::Json;
use serde_json::{json, Value};

use super::SharedDatabase;
use crate::http_errors::HubError;
use crate::operations::json_body::parse_javascript_json;
use crate::operations::{apply_batch, client_error_message, parse_operation_batch, Operation};

const MAX_LOGGED_KINDS: usize = 20;

// Mental model:
// body within limit → JSON content → zod-equivalent shape → apply atomically → { accepted: n }
pub async fn accept_operation_batch(
    State(database): State<SharedDatabase>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Value>, HubError> {
    let body = body.map_err(body_rejection)?;
    let document = parse_json_body(&headers, &body)?;
    let operations = parse_operation_batch(&document).map_err(|issues| {
        tracing::warn!(
            event = "hub.operations.invalid",
            issues = issues.len(),
            "invalid operation batch"
        );
        HubError::Body(
            StatusCode::BAD_REQUEST,
            json!({ "error": "invalid operation batch", "issues": issues }),
        )
    })?;

    let kinds = logged_kinds(&operations);
    let started = Instant::now();
    let outcome = database
        .write(move |connection| apply_batch(connection, &operations))
        .await;

    match outcome {
        Ok(accepted) => {
            tracing::info!(
                event = "hub.operations.accepted",
                accepted,
                duration_ms = started.elapsed().as_millis() as u64,
                "operation batch applied"
            );
            Ok(Json(json!({ "accepted": accepted })))
        }
        Err(error) => {
            tracing::error!(
                event = "hub.operations.failed",
                kinds = %kinds,
                error = %format!("{error:#}"),
                "operation batch failed and was rolled back"
            );
            Err(HubError::bad_request(client_error_message(&error)))
        }
    }
}

/// Fastify's two default parsers: `application/json` (Node `JSON.parse`) and `text/plain`
/// (the raw string, which then fails zod validation with a 400).
fn parse_json_body(headers: &HeaderMap, body: &Bytes) -> Result<Value, HubError> {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("undefined");

    if is_media_type(content_type, "text/plain") {
        return Ok(Value::String(String::from_utf8_lossy(body).into_owned()));
    }
    if !is_media_type(content_type, "application/json") {
        return Err(HubError::Fastify {
            status: StatusCode::UNSUPPORTED_MEDIA_TYPE,
            code: "FST_ERR_CTP_INVALID_MEDIA_TYPE",
            message: format!("Unsupported Media Type: {content_type}"),
        });
    }
    if body.is_empty() {
        return Err(HubError::Fastify {
            status: StatusCode::BAD_REQUEST,
            code: "FST_ERR_CTP_EMPTY_JSON_BODY",
            message: "Body cannot be empty when content-type is set to 'application/json'".into(),
        });
    }
    parse_javascript_json(body).map_err(|error| HubError::Fastify {
        status: StatusCode::BAD_REQUEST,
        code: "FST_ERR_CTP_INVALID_JSON_BODY",
        message: error.to_string(),
    })
}

fn body_rejection(rejection: BytesRejection) -> HubError {
    if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
        return HubError::Fastify {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            code: "FST_ERR_CTP_BODY_TOO_LARGE",
            message: "Request body is too large".into(),
        };
    }
    HubError::Fastify {
        status: rejection.status(),
        code: "FST_ERR_CTP_INVALID_CONTENT_LENGTH",
        message: rejection.body_text(),
    }
}

#[inline]
fn is_media_type(content_type: &str, expected: &str) -> bool {
    content_type
        .split(';')
        .next()
        .is_some_and(|media| media.trim().eq_ignore_ascii_case(expected))
}

/// Kinds only (never data) so failures are diagnosable without logging user content.
fn logged_kinds(operations: &[Operation]) -> String {
    operations
        .iter()
        .take(MAX_LOGGED_KINDS)
        .map(|operation| operation.kind.as_str())
        .collect::<Vec<_>>()
        .join(",")
}
