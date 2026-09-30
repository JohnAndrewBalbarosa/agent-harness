//! Observability operations produced by one bridge hook (`herdr.binding`, `agent-state.record`,
//! `turn-usage.record`, `pane-evidence.record`).
//!
//! Deliberate fix (spec "Deliberate deviations"): the Python bridge loaded a non-existent
//! `tools/observability-hub/obs.py`, so every enqueue failed and was only logged. Here the operations are
//! built with `obs_cli::operation::build_operation` and inserted into the real client spool exactly like
//! obs.py `enqueue(..., flush_now=False)` (INSERT OR IGNORE, then trim to `OBS_MAX_QUEUED_OPERATIONS`), then
//! one detached `<root>/bin/obs-rs.exe flush` delivers them without blocking the hook.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use obs_cli::operation::build_operation;
use obs_cli::python_compat::{python_utc_now, to_python_json, JsonLayout};
use obs_cli::settings::Settings;
use obs_cli::spool::begin_write;
use obs_cli::spool::open::open_spool_at;
use rusqlite::{params, Connection};
use serde_json::{json, Value};

use super::background_launch::spawn_detached;
use super::bridge_log::BridgeLog;

const SPOOL_ENV: &str = "AH_OBS_SPOOL";

#[derive(Debug, Default)]
pub struct ObservabilityOutbox {
    operations: Vec<Value>,
}

impl ObservabilityOutbox {
    /// `enqueue_observability(kind, data)`: builds the operation envelope now (its `occurredAt`).
    pub fn record(&mut self, kind: &str, data: Value) {
        self.operations.push(build_operation(kind, &data));
    }

    pub fn operations(&self) -> &[Value] {
        &self.operations
    }

    // Mental model: nothing queued → nothing; spool insert fails → one `observability.enqueue` failure per
    // operation (Python's log line); inserted → start the detached flusher.
    pub fn deliver(self, harness_root: &Path, env: &BTreeMap<String, String>, log: &BridgeLog) {
        if self.operations.is_empty() {
            return;
        }
        let spool = spool_path(harness_root, env);
        match persist_operations(&spool, &self.operations) {
            Ok(()) => start_flusher(harness_root, env, &spool, log),
            Err(error) => {
                for operation in &self.operations {
                    let kind = operation.get("kind").cloned().unwrap_or(Value::Null);
                    log.event(
                        "observability.enqueue",
                        "failed",
                        vec![("operation", kind), ("error", json!(error_name(&error)))],
                    );
                }
            }
        }
    }
}

/// `$AH_OBS_SPOOL`, else `<root>/tools/observability-client/var/spool.sqlite3`.
pub fn spool_path(harness_root: &Path, env: &BTreeMap<String, String>) -> PathBuf {
    env.get(SPOOL_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            harness_root
                .join("tools")
                .join("observability-client")
                .join("var")
                .join("spool.sqlite3")
        })
}

/// obs.py `enqueue`: one write transaction, `INSERT OR IGNORE`, trim to the newest cap rows.
pub fn persist_operations(spool: &Path, operations: &[Value]) -> anyhow::Result<()> {
    let _spool_access = crate::shared_state::spool_access::lock_spool_access();
    let conn = open_spool_at(spool)
        .map_err(|failure| anyhow::anyhow!("spool open failed: {failure:?}"))?;
    insert_and_trim(
        &conn,
        operations,
        Settings::from_env().max_queued_operations,
    )?;
    Ok(())
}

fn insert_and_trim(conn: &Connection, operations: &[Value], cap: i64) -> rusqlite::Result<()> {
    let transaction = begin_write(conn)?;
    let created_at = python_utc_now();
    for operation in operations {
        let operation_id = operation
            .get("operationId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let payload = to_python_json(operation, JsonLayout::Compact, true);
        transaction.execute(
            "INSERT OR IGNORE INTO queue(operation_id,payload,created_at) VALUES(?,?,?)",
            params![operation_id, payload, created_at],
        )?;
    }
    transaction.execute(
        "DELETE FROM queue WHERE operation_id IN (SELECT operation_id FROM queue ORDER BY created_at DESC,operation_id DESC LIMIT -1 OFFSET ?)",
        params![cap],
    )?;
    transaction.commit()
}

/// Detached `<root>/bin/obs-rs.exe flush` pinned to the same spool; absent binary → the spool waits for the
/// next flusher (obs consumer / services).
pub fn start_flusher(
    harness_root: &Path,
    env: &BTreeMap<String, String>,
    spool: &Path,
    log: &BridgeLog,
) {
    let flusher = harness_root.join("bin").join("obs-rs.exe");
    // Same throttle and stamp as the obs consumer; obs always flushes at turn/session end.
    if !crate::consumers::obs::flush_trigger::is_flush_due_for_spool("", spool) {
        return;
    }
    if !flusher.is_file() {
        return;
    }
    let mut child_env = env.clone();
    child_env.insert(SPOOL_ENV.to_owned(), spool.display().to_string());
    if let Err(error) = spawn_detached(&flusher, &["flush".to_owned()], harness_root, &child_env) {
        log.event(
            "observability.flush.launch",
            "failed",
            vec![("error", json!(error.kind().to_string()))],
        );
    }
}

#[inline]
fn error_name(error: &anyhow::Error) -> &'static str {
    if error.downcast_ref::<rusqlite::Error>().is_some() {
        "sqlite3.Error"
    } else {
        "SpoolError"
    }
}
