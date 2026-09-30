//! `--project-id` lifecycle events (interval_supervisor.py:190-200, :229-238, :275-284, :305-337).
//!
//! Deliberate fix: Python imported `sdk.python.ObservabilityClient` (gone, so the tool crashed at
//! import). The same operations the SDK posted — `code-version.register`, `execution.start`,
//! `event.emit` (process.started / checkpoint / completed / failed), `execution.finish` — are now
//! enqueued into the obs spool through `obs_cli` (spool resolved like obs-rs: `AH_OBS_SPOOL`,
//! `OBS_HUB_DIR`, sibling `obs.py`, default harness spool) and flushed to the hub.
//!
//! Observability never blocks or fails the supervised command: delivery runs on one background
//! thread, failures are reported once on stderr, and the final wait is bounded.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use obs_cli::operation::build_operation;
use obs_cli::python_compat::{uuid5_url_text, uuid7_text};
use obs_cli::settings::Settings;
use obs_cli::spool::open::open_spool;
use obs_cli::spool::queue::persist_operations;
use serde_json::{json, Value};

const COMPONENT: &str = "codex.interval_supervisor";
/// SDK `execution_kind=2`.
const EXECUTION_KIND: i64 = 2;
/// How long the supervisor waits for queued events after the completion record is printed.
const FINAL_DELIVERY_WAIT: Duration = Duration::from_secs(5);

// SDK numeric codes (category / severity / outcome / execution status).
const CATEGORY_DEFAULT: i64 = 1;
const CATEGORY_FAILURE: i64 = 4;
const SEVERITY_DEFAULT: i64 = 1;
const SEVERITY_ERROR: i64 = 3;
const OUTCOME_SUCCEEDED: i64 = 1;
const OUTCOME_FAILED: i64 = 3;
const STATUS_COMPLETED: i64 = 2;
const STATUS_FAILED: i64 = 4;

/// One `client.emit(...)` call.
pub struct EventSpec {
    pub code: &'static str,
    pub category: i64,
    pub severity: i64,
    pub outcome: Option<i64>,
    pub message: String,
    pub attributes: Value,
}

/// Identity of the execution the events belong to (SDK `ObservabilityClient`).
#[derive(Debug, Clone)]
pub struct ExecutionIdentity {
    pub project_id: String,
    pub execution_id: String,
    pub trace_id: String,
}

impl ExecutionIdentity {
    pub fn generate(project_id: &str) -> Self {
        Self {
            project_id: project_id.to_owned(),
            execution_id: uuid7_text(),
            // SDK: 16 random bytes as hex; a uuid7's 32 hex digits carry the same width.
            trace_id: uuid7_text().replace('-', ""),
        }
    }
}

/// Lifecycle emitter; a no-op when `--project-id` was not given.
pub struct ObservabilityEvents {
    session: Option<Session>,
}

struct Session {
    identity: ExecutionIdentity,
    sequence: i64,
    sender: Sender<Vec<Value>>,
    worker_finished: Receiver<()>,
}

impl ObservabilityEvents {
    /// Mental model: empty project id → disabled; otherwise start the delivery worker and enqueue
    /// `code-version.register` + `execution.start` (SDK `ObservabilityClient.start`).
    pub fn start(project_id: &str) -> Self {
        if project_id.is_empty() {
            return Self { session: None };
        }
        let session = Session::start(ExecutionIdentity::generate(project_id));
        Self { session }
    }

    pub fn process_started(&mut self, safe_label: &str, interval_seconds: u64) {
        self.emit(EventSpec {
            code: "process.started",
            category: CATEGORY_DEFAULT,
            severity: SEVERITY_DEFAULT,
            outcome: None,
            message: format!("Started {safe_label}"),
            attributes: json!({"label": safe_label, "intervalSeconds": interval_seconds}),
        });
    }

    pub fn checkpoint(&mut self, safe_label: &str, pid: u32, interval_seconds: u64) {
        self.emit(EventSpec {
            code: "process.checkpoint",
            category: CATEGORY_DEFAULT,
            severity: SEVERITY_DEFAULT,
            outcome: None,
            message: format!("Still running: {safe_label}"),
            attributes: json!({"label": safe_label, "pid": pid, "intervalSeconds": interval_seconds}),
        });
    }

    pub fn start_failed(&mut self, safe_label: &str, exception_type: &str) {
        self.emit(failure_event(
            format!("Could not start {safe_label}"),
            json!({"label": safe_label, "exceptionType": exception_type}),
        ));
        self.finish(
            OUTCOME_FAILED,
            STATUS_FAILED,
            format!("{safe_label} failed"),
        );
    }

    pub fn completed(
        &mut self,
        safe_label: &str,
        exit_code: i64,
        record: &serde_json::Map<String, Value>,
    ) {
        self.emit(EventSpec {
            code: "process.completed",
            category: CATEGORY_DEFAULT,
            severity: SEVERITY_DEFAULT,
            outcome: Some(OUTCOME_SUCCEEDED),
            message: format!("Completed {safe_label}"),
            attributes: json!({
                "label": safe_label,
                "exitCode": exit_code,
                "durationSeconds": record.get("duration_seconds").cloned().unwrap_or(Value::Null),
                "summary": record.get("summary").cloned().unwrap_or(Value::Null),
            }),
        });
        self.finish(
            OUTCOME_SUCCEEDED,
            STATUS_COMPLETED,
            format!("{safe_label} completed"),
        );
    }

    pub fn failed(
        &mut self,
        safe_label: &str,
        exit_code: i64,
        record: &serde_json::Map<String, Value>,
    ) {
        self.emit(failure_event(
            format!("Failed {safe_label}; inspect bounded diagnostic tail"),
            json!({
                "label": safe_label,
                "exitCode": exit_code,
                "durationSeconds": record.get("duration_seconds").cloned().unwrap_or(Value::Null),
                "diagnosticTail": record.get("tail").cloned().unwrap_or(Value::Null),
            }),
        ));
        self.finish(
            OUTCOME_FAILED,
            STATUS_FAILED,
            format!("{safe_label} failed"),
        );
    }

    /// Stops accepting events and waits (bounded) for the worker to persist/flush them.
    pub fn close(self) {
        if let Some(session) = self.session {
            session.close();
        }
    }

    fn emit(&mut self, event: EventSpec) {
        if let Some(session) = self.session.as_mut() {
            session.sequence += 1;
            let operation = event_operation(&session.identity, session.sequence, &event);
            session.send(vec![operation]);
        }
    }

    fn finish(&mut self, outcome: i64, status: i64, result_summary: String) {
        if let Some(session) = self.session.as_ref() {
            let operation = finish_operation(&session.identity, outcome, status, &result_summary);
            session.send(vec![operation]);
        }
    }
}

impl Session {
    fn start(identity: ExecutionIdentity) -> Option<Self> {
        let (sender, receiver) = mpsc::channel::<Vec<Value>>();
        let (finished_sender, worker_finished) = mpsc::channel::<()>();
        let spawned = thread::Builder::new()
            .name("interval-supervisor-obs".to_owned())
            .spawn(move || {
                deliver_until_closed(&receiver);
                let _ = finished_sender.send(());
            });
        if let Err(error) = spawned {
            report_delivery_problem(&format!("worker could not start: {error}"));
            return None;
        }

        let session = Self {
            identity,
            sequence: 0,
            sender,
            worker_finished,
        };
        session.send(start_operations(&session.identity));
        Some(session)
    }

    /// The worker only stops after the sender is dropped, so a send never fails while it runs.
    #[inline]
    fn send(&self, operations: Vec<Value>) {
        let _ = self.sender.send(operations);
    }

    fn close(self) {
        let Self {
            sender,
            worker_finished,
            ..
        } = self;
        drop(sender);
        if worker_finished.recv_timeout(FINAL_DELIVERY_WAIT).is_err() {
            report_delivery_problem(
                "delivery unfinished after 5 s; persisted events stay queued in the spool",
            );
        }
    }
}

/// Mental model: open the spool once → for each wake-up: persist every pending batch, trim,
/// flush once (obs.py `enqueue`).
fn deliver_until_closed(receiver: &Receiver<Vec<Value>>) {
    let settings = Settings::from_env();
    let connection = match open_spool() {
        Ok(connection) => connection,
        Err(error) => {
            report_delivery_problem(&format!("spool unavailable: {error:?}"));
            // Drain so senders never notice; events are dropped.
            for _ in receiver.iter() {}
            return;
        }
    };
    let mut reported = false;

    // Batches that arrived while the previous write ran are persisted together.
    while let Ok(first_batch) = receiver.recv() {
        let mut operations = first_batch;
        operations.extend(receiver.try_iter().flatten());
        // Enqueue only: a synchronous flush would stall on a down hub (~2 s per refused connect on
        // Windows); the harness-services daemon drains the spool.
        let delivered = persist_operations(&connection, &settings, &operations);
        if let (Err(error), false) = (delivered, reported) {
            report_delivery_problem(&format!("spool write failed: {error:?}"));
            reported = true;
        }
    }
}

/// One bounded stderr line; stdout stays reserved for the completion record.
#[inline]
fn report_delivery_problem(detail: &str) {
    eprintln!("interval-supervisor: observability {detail}");
}

#[inline]
fn failure_event(message: String, attributes: Value) -> EventSpec {
    EventSpec {
        code: "process.failed",
        category: CATEGORY_FAILURE,
        severity: SEVERITY_ERROR,
        outcome: Some(OUTCOME_FAILED),
        message,
        attributes,
    }
}

/// SDK `ObservabilityClient.start` operations.
pub fn start_operations(identity: &ExecutionIdentity) -> Vec<Value> {
    let code_version_id = uuid5_url_text(&format!("{}||", identity.project_id));
    vec![
        build_operation(
            "code-version.register",
            &json!({
                "codeVersionId": code_version_id,
                "projectId": identity.project_id,
                "commitSha": null,
                "branchName": null,
                "dirtyFingerprint": null,
                "buildVersion": null,
            }),
        ),
        build_operation(
            "execution.start",
            &json!({
                "executionId": identity.execution_id,
                "projectId": identity.project_id,
                "codeVersionId": code_version_id,
                "executionKind": EXECUTION_KIND,
                "attemptNo": 1,
                "traceId": identity.trace_id,
            }),
        ),
    ]
}

/// SDK `emit`: one `event.emit` operation.
pub fn event_operation(identity: &ExecutionIdentity, sequence: i64, event: &EventSpec) -> Value {
    build_operation(
        "event.emit",
        &json!({
            "eventId": uuid7_text(),
            "executionId": identity.execution_id,
            "projectId": identity.project_id,
            "traceId": identity.trace_id,
            "sequenceNo": sequence,
            "code": event.code,
            "category": event.category,
            "severity": event.severity,
            "outcome": event.outcome,
            "component": COMPONENT,
            "messageSummary": event.message,
            "attributes": event.attributes,
        }),
    )
}

/// SDK `finish`: one `execution.finish` operation.
pub fn finish_operation(
    identity: &ExecutionIdentity,
    outcome: i64,
    status: i64,
    result_summary: &str,
) -> Value {
    build_operation(
        "execution.finish",
        &json!({
            "executionId": identity.execution_id,
            "outcome": outcome,
            "status": status,
            "resultSummary": result_summary,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> ExecutionIdentity {
        ExecutionIdentity {
            project_id: "11111111-1111-1111-1111-111111111111".to_owned(),
            execution_id: "exec-1".to_owned(),
            trace_id: "trace-1".to_owned(),
        }
    }

    #[test]
    fn start_registers_code_version_then_execution() {
        let operations = start_operations(&identity());
        let kinds: Vec<&str> = operations
            .iter()
            .map(|operation| operation["kind"].as_str().unwrap())
            .collect();

        assert_eq!(kinds, ["code-version.register", "execution.start"]);
        assert_eq!(
            operations[0]["data"]["codeVersionId"],
            operations[1]["data"]["codeVersionId"]
        );
        assert_eq!(operations[1]["data"]["executionKind"], 2);
    }

    #[test]
    fn event_carries_sdk_fields_in_order() {
        let event = failure_event("Failed job".to_owned(), json!({"label": "job"}));
        let operation = event_operation(&identity(), 3, &event);
        let data = operation["data"].as_object().unwrap();
        let keys: Vec<&str> = data.keys().map(String::as_str).collect();

        assert_eq!(operation["kind"], "event.emit");
        assert_eq!(
            keys,
            [
                "eventId",
                "executionId",
                "projectId",
                "traceId",
                "sequenceNo",
                "code",
                "category",
                "severity",
                "outcome",
                "component",
                "messageSummary",
                "attributes"
            ]
        );
        assert_eq!(data["code"], "process.failed");
        assert_eq!(data["sequenceNo"], 3);
        assert_eq!(data["outcome"], 3);
        assert_eq!(data["component"], "codex.interval_supervisor");
    }

    #[test]
    fn disabled_without_project_id() {
        let mut events = ObservabilityEvents::start("");
        events.process_started("job", 30);
        assert!(events.session.is_none());
        events.close();
    }
}
