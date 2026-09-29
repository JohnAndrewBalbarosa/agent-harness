//! Error responses in the shapes Fastify and server.mjs produced.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use crate::routes::wire::js_json_response;

#[derive(Debug)]
pub enum HubError {
    /// Handler-level `{ "error": message }` with an explicit status (404/400).
    Client(StatusCode, String),
    /// Fastify's default body for an uncaught handler exception; `code` is the Postgres SQLSTATE
    /// when the original error came from Postgres (e.g. `22P02` for a malformed uuid).
    Internal(String, Option<&'static str>),
    /// Fastify's content-type parser / body limit errors.
    Fastify {
        status: StatusCode,
        code: &'static str,
        message: String,
    },
    /// Any other prepared JSON body.
    Body(StatusCode, Value),
}

impl HubError {
    pub fn not_found(message: &str) -> Self {
        Self::Client(StatusCode::NOT_FOUND, message.to_owned())
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::Client(StatusCode::BAD_REQUEST, message.into())
    }
}

impl From<anyhow::Error> for HubError {
    fn from(error: anyhow::Error) -> Self {
        tracing::error!(event = "hub.request_failed", error = %format!("{error:#}"), "request failed");
        Self::Internal(format!("{error:#}"), None)
    }
}

impl IntoResponse for HubError {
    fn into_response(self) -> Response {
        match self {
            Self::Client(status, message) => js_json_response(status, &json!({ "error": message })),
            Self::Internal(message, code) => {
                fastify_body(StatusCode::INTERNAL_SERVER_ERROR, code, &message)
            }
            Self::Fastify {
                status,
                code,
                message,
            } => fastify_body(status, Some(code), &message),
            Self::Body(status, body) => (status, Json(body)).into_response(),
        }
    }
}

/// `{"statusCode":..,"code"?:..,"error":"<reason phrase>","message":..}`.
pub fn fastify_body(status: StatusCode, code: Option<&str>, message: &str) -> Response {
    let mut body = serde_json::Map::new();
    body.insert("statusCode".into(), json!(status.as_u16()));
    if let Some(code) = code {
        body.insert("code".into(), json!(code));
    }
    body.insert(
        "error".into(),
        json!(status.canonical_reason().unwrap_or("Error")),
    );
    body.insert("message".into(), json!(message));
    (status, Json(Value::Object(body))).into_response()
}

/// Fastify's 404 for unknown routes (it also answers wrong methods with 404).
pub fn route_not_found(method: &str, path: &str) -> Response {
    let body = json!({
        "message": format!("Route {method}:{path} not found"),
        "error": "Not Found",
        "statusCode": 404,
    });
    (StatusCode::NOT_FOUND, Json(body)).into_response()
}
