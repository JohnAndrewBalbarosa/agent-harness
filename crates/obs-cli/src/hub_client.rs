//! HTTP calls to the observability hub, with obs.py's error texts:
//! - reads  (obs.py:858 `fetch_json`): 5 s timeout, JSON object or a `SystemExit` message.
//! - writes (obs.py:248 `post`): `POST /v1/operations`, rejected batches become
//!   `hub returned HTTP <status>: <reason>`.
//!
//! Python distinguishes `URLError` (connection-phase failures: bisect + dead-letter after 4
//! attempts) from other exceptions (read timeouts, dropped connections: charge an attempt).
//! `DeliveryError::Unreachable` and `DeliveryError::Interrupted` carry that distinction.

use std::error::Error as StdError;
use std::fmt;
use std::io::{self, Read};
use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::failure::{CliFailure, CliResult};
use crate::python_compat::{
    python_observed_time, redact, to_python_json, truncate_chars, JsonLayout,
};
use crate::settings::{Settings, HUB_READ_TIMEOUT};

const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_REJECTION_BODY_BYTES: u64 = 2048;
const MAX_REJECTION_REASON_CHARS: usize = 240;
const DEFAULT_REJECTION_REASON: &str = "request rejected";

#[derive(Debug)]
pub enum DeliveryError {
    /// obs.py `HubDeliveryError(status, reason)`.
    Rejected { status: u16, reason: String },
    /// obs.py `urllib.error.URLError` / `ConnectionAbortedError`.
    Unreachable(String),
    /// Any other exception raised while posting.
    Interrupted(String),
}

impl fmt::Display for DeliveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected { status, reason } => write!(
                formatter,
                "hub returned HTTP {status}: {}",
                truncate_chars(reason, MAX_REJECTION_REASON_CHARS)
            ),
            Self::Unreachable(text) | Self::Interrupted(text) => formatter.write_str(text),
        }
    }
}

/// obs.py:858 `fetch_json(path)`.
pub fn fetch_hub_object(settings: &Settings, path: &str) -> CliResult<Value> {
    let url = format!("{}{path}", settings.endpoint);
    let agent = agent_with_timeout(HUB_READ_TIMEOUT);

    match agent.get(&url).call() {
        Ok(response) => parse_object_response(response),
        Err(ureq::Error::Status(status, response)) => Err(hub_unavailable(format!(
            "HTTP Error {status}: {}",
            response.status_text()
        ))),
        Err(ureq::Error::Transport(transport)) => Err(hub_unavailable(format!(
            "<urlopen error {}>",
            describe_transport(&transport)
        ))),
    }
}

#[inline]
fn hub_unavailable(detail: String) -> CliFailure {
    CliFailure::refusal(format!("Observability hub unavailable: {detail}"))
}

fn parse_object_response(response: ureq::Response) -> CliResult<Value> {
    let mut body = Vec::new();
    let read = response
        .into_reader()
        .take(MAX_RESPONSE_BYTES)
        .read_to_end(&mut body);
    let parsed = read
        .ok()
        .and_then(|_| serde_json::from_slice::<Value>(&body).ok());

    match parsed {
        Some(value) if value.is_object() => Ok(value),
        _ => Err(CliFailure::refusal(
            "Observability hub returned an invalid response.",
        )),
    }
}

/// obs.py:248 `post(ops)`: every `occurredAt` is re-normalized before sending.
pub fn post_operations(settings: &Settings, operations: &[Value]) -> Result<(), DeliveryError> {
    let body = operations_request_body(operations)?;
    let url = format!("{}/v1/operations", settings.endpoint);
    let agent = agent_with_timeout(settings.http_timeout);

    let outcome = agent
        .post(&url)
        .set("Content-Type", "application/json")
        .send_bytes(body.as_bytes());

    match outcome {
        Ok(response) if response.status() >= 300 => Err(DeliveryError::Rejected {
            status: response.status(),
            reason: DEFAULT_REJECTION_REASON.to_owned(),
        }),
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(status, response)) => Err(DeliveryError::Rejected {
            status,
            reason: rejection_reason(response),
        }),
        Err(ureq::Error::Transport(transport)) => Err(classify_transport_failure(&transport)),
    }
}

fn operations_request_body(operations: &[Value]) -> Result<String, DeliveryError> {
    let wire_operations = operations
        .iter()
        .map(with_normalized_occurred_at)
        .collect::<Result<Vec<Value>, DeliveryError>>()?;
    let body = json!({ "operations": wire_operations });
    Ok(to_python_json(&body, JsonLayout::Compact, true))
}

/// obs.py `dict(operation)` + `observed_time(operation.get("occurredAt"))`.
fn with_normalized_occurred_at(operation: &Value) -> Result<Value, DeliveryError> {
    let Some(fields) = operation.as_object() else {
        return Err(DeliveryError::Interrupted(
            "queued operation payload is not a JSON object".to_owned(),
        ));
    };

    let mut wire: Map<String, Value> = fields.clone();
    let occurred_at = python_observed_time(fields.get("occurredAt"));
    wire.insert("occurredAt".to_owned(), Value::String(occurred_at));
    Ok(Value::Object(wire))
}

/// obs.py:262-267: first 2048 body bytes; a truthy `error` field replaces the default reason.
fn rejection_reason(response: ureq::Response) -> String {
    let mut body = Vec::new();
    let _ = response
        .into_reader()
        .take(MAX_REJECTION_BODY_BYTES)
        .read_to_end(&mut body);

    serde_json::from_str::<Value>(&String::from_utf8_lossy(&body))
        .ok()
        .and_then(|parsed| {
            parsed
                .get("error")
                .filter(|error| is_python_truthy(error))
                .cloned()
        })
        .map(|error| truncate_chars(&python_str(&redact(&error)), MAX_REJECTION_REASON_CHARS))
        .unwrap_or_else(|| DEFAULT_REJECTION_REASON.to_owned())
}

#[inline]
fn is_python_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|float| float != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(entries) => !entries.is_empty(),
    }
}

/// Python `str(value)`: strings verbatim; other shapes approximated by their JSON text.
#[inline]
fn python_str(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => to_python_json(other, JsonLayout::Spaced, false),
    }
}

fn classify_transport_failure(transport: &ureq::Transport) -> DeliveryError {
    let described = describe_transport(transport);

    if is_connection_phase_failure(transport.kind()) {
        DeliveryError::Unreachable(format!("<urlopen error {described}>"))
    } else {
        DeliveryError::Interrupted(described)
    }
}

#[inline]
fn is_connection_phase_failure(kind: ureq::ErrorKind) -> bool {
    matches!(
        kind,
        ureq::ErrorKind::Dns
            | ureq::ErrorKind::ConnectionFailed
            | ureq::ErrorKind::InvalidUrl
            | ureq::ErrorKind::UnknownScheme
            | ureq::ErrorKind::InvalidProxyUrl
            | ureq::ErrorKind::ProxyConnect
            | ureq::ErrorKind::ProxyUnauthorized
    )
}

/// Python's text for the underlying socket error (`[WinError 10061] ...`, `timed out`).
fn describe_transport(transport: &ureq::Transport) -> String {
    let root: &(dyn StdError + 'static) = transport;

    match find_io_error(root) {
        Some(io_error) => python_os_error_text(io_error),
        None => transport.to_string(),
    }
}

fn find_io_error<'a>(error: &'a (dyn StdError + 'static)) -> Option<&'a io::Error> {
    let mut current = Some(error);
    while let Some(candidate) = current {
        if let Some(io_error) = candidate.downcast_ref::<io::Error>() {
            return Some(io_error);
        }
        current = candidate.source();
    }
    None
}

fn python_os_error_text(error: &io::Error) -> String {
    if matches!(
        error.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    ) {
        return "timed out".to_owned();
    }

    let Some(code) = error.raw_os_error() else {
        return error.to_string();
    };
    let full = error.to_string();
    let message = full
        .strip_suffix(&format!(" (os error {code})"))
        .unwrap_or(&full)
        .trim_end_matches(|c: char| c == '.' || c.is_whitespace());

    if cfg!(windows) {
        format!("[WinError {code}] {message}")
    } else {
        format!("[Errno {code}] {message}")
    }
}

fn agent_with_timeout(timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(timeout)
        .timeout_read(timeout)
        .timeout_write(timeout)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejection_display_matches_hub_delivery_error() {
        let error = DeliveryError::Rejected {
            status: 400,
            reason: "invalid operation batch".to_owned(),
        };
        assert_eq!(
            error.to_string(),
            "hub returned HTTP 400: invalid operation batch"
        );
    }

    #[test]
    fn request_body_normalizes_occurred_at_and_keeps_order() {
        let operation = json!({"operationId": "x", "kind": "k", "occurredAt": "2026-09-03T00:00:00+08:00", "data": {}});
        let body = operations_request_body(&[operation]).unwrap();

        assert_eq!(
            body,
            "{\"operations\":[{\"operationId\":\"x\",\"kind\":\"k\",\"occurredAt\":\"2026-09-02T16:00:00Z\",\"data\":{}}]}"
        );
    }

    #[test]
    fn non_object_payload_interrupts_delivery() {
        assert!(matches!(
            operations_request_body(&[json!([1])]),
            Err(DeliveryError::Interrupted(_))
        ));
    }

    #[test]
    fn truthiness_follows_python() {
        assert!(!is_python_truthy(&json!("")));
        assert!(!is_python_truthy(&json!(0)));
        assert!(is_python_truthy(&json!("bad")));
    }
}
