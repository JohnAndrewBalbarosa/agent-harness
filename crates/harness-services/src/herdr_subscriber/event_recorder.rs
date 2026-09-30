//! subscriber.py `record_event(value, sequence)`: log every Herdr event (`herdr.event`) and, for panes bound to a
//! project, enqueue `herdr-event.record` into the observability spool.
//!
//! Deliberate fix: the operation is inserted into the real client spool with the hook's outbox helpers
//! (`persist_operations`, same INSERT OR IGNORE + trim as obs.py `enqueue(flush_now=False)`), then the hook's
//! throttled detached `obs-rs flush` is requested (`start_flusher`). Python's `obs.py` import always failed.

use std::collections::BTreeMap;
use std::path::PathBuf;

use harness_hook::consumers::herdr_bridge::bridge_log::BridgeLog;
use harness_hook::consumers::herdr_bridge::observability_outbox::{
    persist_operations, start_flusher,
};
use harness_hook::consumers::herdr_bridge::session_state::session_root;
use obs_cli::operation::build_operation;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use super::event_payload::{event_body, event_name, find_pane_in_object};
use super::pane_binding::{binding_for_pane, PaneBinding};
use super::sanitize::sanitize_event_body;
use crate::service_context::ServiceContext;

pub const HERDR_EVENT_OPERATION: &str = "herdr-event.record";

/// Everything one Herdr event produces before any I/O.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedEvent {
    pub event_name: String,
    /// `""` when the event names no pane.
    pub pane_id: String,
    pub sequence: u64,
    pub payload: Map<String, Value>,
}

impl RecordedEvent {
    // Mental model: body (`event` object or the whole message) → name → pane → sanitized payload.
    pub fn from_message(message: &Map<String, Value>, sequence: u64) -> Self {
        let body = event_body(message);
        Self {
            event_name: event_name(message, body),
            pane_id: find_pane_in_object(body),
            sequence,
            payload: sanitize_event_body(body),
        }
    }

    /// `pane_id or None`.
    #[inline]
    fn pane_value(&self) -> Value {
        if self.pane_id.is_empty() {
            Value::Null
        } else {
            Value::from(self.pane_id.as_str())
        }
    }

    /// The `herdr-event.record` data, key order as subscriber.py.
    pub fn operation_data(&self, binding: &PaneBinding) -> Value {
        json!({
            "herdrEventId": Uuid::new_v4().to_string(),
            "projectId": binding.project_id,
            "nativeSession": binding.native_session,
            "paneId": self.pane_value(),
            "eventName": self.event_name,
            "sequenceNo": self.sequence,
            "payload": Value::Object(self.payload.clone()),
        })
    }
}

pub struct EventRecorder<'log> {
    harness_root: PathBuf,
    env: BTreeMap<String, String>,
    spool: PathBuf,
    session_root: PathBuf,
    log: &'log BridgeLog,
}

impl<'log> EventRecorder<'log> {
    pub fn new(context: &ServiceContext, log: &'log BridgeLog) -> Self {
        Self {
            harness_root: context.harness_root.clone(),
            env: context.env.clone(),
            spool: context.spool_path(),
            session_root: session_root(&context.harness_root),
            log,
        }
    }

    // Mental model: build → log `herdr.event` → unbound pane stops here → persist into the spool.
    pub fn record(&self, message: &Map<String, Value>, sequence: u64) {
        let event = RecordedEvent::from_message(message, sequence);
        let binding = binding_for_pane(&self.session_root, &event.pane_id);
        self.log.event(
            "herdr.event",
            "succeeded",
            vec![
                ("event_name", json!(event.event_name)),
                ("pane", event.pane_value()),
                ("sequence", json!(event.sequence)),
                ("payload", Value::Object(event.payload.clone())),
            ],
        );
        if let Some(binding) = binding {
            self.persist(&event, &binding);
        }
    }

    /// Spool insert, then the throttled detached flusher; a failed insert is logged as `herdr.event.persist`.
    fn persist(&self, event: &RecordedEvent, binding: &PaneBinding) {
        let operation = build_operation(HERDR_EVENT_OPERATION, &event.operation_data(binding));
        match persist_operations(&self.spool, &[operation]) {
            Ok(()) => start_flusher(&self.harness_root, &self.env, &self.spool, self.log),
            Err(error) => self.log.event(
                "herdr.event.persist",
                "failed",
                vec![
                    ("event_name", json!(event.event_name)),
                    ("error", json!(persist_error_name(&error))),
                ],
            ),
        }
    }
}

/// A Python-style exception type name for the log (`type(exc).__name__`).
#[inline]
fn persist_error_name(error: &anyhow::Error) -> &'static str {
    if error.downcast_ref::<rusqlite::Error>().is_some() {
        "sqlite3.Error"
    } else {
        "SpoolError"
    }
}
