//! receiver.py:98 `do_POST` body handling (blocking part): `json.loads(body or b"{}")` →
//! `usage_records` → `enqueue_usage`. Any failure is reported to the caller, which logs it and still
//! answers 200 (telemetry must never break an agent's exporter loop).

use std::fmt;

use harness_hook::shared_state::spool_access::lock_spool_access;
use serde_json::Value;

use super::project_lookup::registered_project;
use super::python_semantics::ShapeError;
use super::usage_enqueue::enqueue_usage_records;
use super::usage_records::usage_records;
use crate::service_context::ServiceContext;

/// Why a POST enqueued nothing (or less than everything).
#[derive(Debug)]
pub enum IngestFailure {
    InvalidJson,
    UnexpectedShape(ShapeError),
    SpoolWrite(anyhow::Error),
}

impl fmt::Display for IngestFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IngestFailure::InvalidJson => formatter.write_str("JSONDecodeError"),
            IngestFailure::UnexpectedShape(shape) => formatter.write_str(shape.python_type_name()),
            IngestFailure::SpoolWrite(error) => write!(formatter, "SpoolWriteError: {error:#}"),
        }
    }
}

/// Parses, maps and enqueues one `/v1/logs` body; returns the number of enqueued records.
pub fn ingest_logs_body(context: &ServiceContext, body: &[u8]) -> Result<usize, IngestFailure> {
    // Mental model: parse → (spool lock) project lookup per record → enqueue batch → count.
    let payload = parse_logs_body(body)?;
    let spool = context.spool_path();
    let _spool_access = lock_spool_access();
    let records = usage_records(&payload, |session| registered_project(&spool, session))
        .map_err(IngestFailure::UnexpectedShape)?;
    enqueue_usage_records(&spool, &records, &context.env).map_err(IngestFailure::SpoolWrite)
}

/// `json.loads(body or b"{}")`.
#[inline]
fn parse_logs_body(body: &[u8]) -> Result<Value, IngestFailure> {
    if body.is_empty() {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    serde_json::from_slice(body).map_err(|_| IngestFailure::InvalidJson)
}
