//! Spool access for `turn-usage.record` (`core/usage/obs_client.py`): read-only project lookup,
//! enqueue without an inline flush (obs.py `enqueue(..., flush_now=False)`), then one detached flush.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::Context;
use obs_cli::operation::build_operation;
use obs_cli::python_compat::{parse_python_int, python_utc_now, to_python_json, JsonLayout};
use obs_cli::spool::begin_write;
use obs_cli::spool::open::open_spool_at;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde_json::Value;

pub const OPERATION_KIND: &str = "turn-usage.record";
/// `sqlite3.connect(..., timeout=2)` for the read-only lookup.
const LOOKUP_BUSY_TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_MAX_QUEUED: i64 = 10_000;
const MIN_OPERATION_CAP: i64 = 100;
#[cfg(windows)]
const OBS_RS_FILE: &str = "obs-rs.exe";
#[cfg(not(windows))]
const OBS_RS_FILE: &str = "obs-rs";
#[cfg(windows)]
const DETACHED_NO_WINDOW: u32 = 0x0800_0000 | 0x0000_0008;

/// `AH_OBS_SPOOL` or `<root>/tools/observability-client/var/spool.sqlite3`.
pub fn spool_path(env: &BTreeMap<String, String>, harness_root: &Path) -> PathBuf {
    env.get("AH_OBS_SPOOL")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            observability_client_dir(harness_root)
                .join("var")
                .join("spool.sqlite3")
        })
}

#[inline]
fn observability_client_dir(harness_root: &Path) -> PathBuf {
    harness_root.join("tools").join("observability-client")
}

/// `spool_project(native_session)`: the registered project of a session, read-only; any SQLite
/// failure (missing spool, busy, schema) is `None`.
pub fn spool_project(spool: &Path, native_session: &str) -> Option<String> {
    let conn = Connection::open_with_flags(
        spool,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    conn.busy_timeout(LOOKUP_BUSY_TIMEOUT).ok()?;
    conn.query_row(
        "SELECT project_id FROM context WHERE native_session=?",
        params![native_session],
        |row| row.get::<_, Option<String>>(0),
    )
    .optional()
    .ok()
    .flatten()
    .flatten()
}

/// `obs.enqueue([obs.operation("turn-usage.record", record)], flush_now=False)`.
pub fn enqueue_usage(
    spool: &Path,
    record: &Value,
    env: &BTreeMap<String, String>,
) -> anyhow::Result<()> {
    let operation = build_operation(OPERATION_KIND, record);
    let operation_id = operation
        .get("operationId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let payload = to_python_json(&operation, JsonLayout::Compact, true);

    let conn = open_spool_at(spool)
        .map_err(|failure| anyhow::anyhow!("spool open failed: {failure:?}"))?;
    let transaction = begin_write(&conn).context("spool write transaction")?;
    transaction.execute(
        "INSERT OR IGNORE INTO queue(operation_id,payload,created_at) VALUES(?,?,?)",
        params![operation_id, payload, python_utc_now()],
    )?;
    transaction.execute(
        "DELETE FROM queue WHERE operation_id IN (SELECT operation_id FROM queue ORDER BY created_at DESC,operation_id DESC LIMIT -1 OFFSET ?)",
        params![max_queued_operations(env)],
    )?;
    transaction.commit()?;
    Ok(())
}

/// `max(100, int(OBS_MAX_QUEUED_OPERATIONS or 10000))`.
#[inline]
fn max_queued_operations(env: &BTreeMap<String, String>) -> i64 {
    env.get("OBS_MAX_QUEUED_OPERATIONS")
        .and_then(|text| parse_python_int(text.as_str()))
        .unwrap_or(DEFAULT_MAX_QUEUED)
        .max(MIN_OPERATION_CAP)
}

/// `hook.schedule_flush()`: one detached flusher. Prefers `<root>/bin/obs-rs(.exe) flush` (pointed at
/// the same spool); falls back to the Python `flush_spool.py` worker. Launch failures are ignored: the
/// operation is already durable in the spool and the next flush delivers it.
pub fn schedule_flush(env: &BTreeMap<String, String>, harness_root: &Path, spool: &Path) {
    let obs_rs = harness_root.join("bin").join(OBS_RS_FILE);
    if obs_rs.is_file() {
        let mut command = Command::new(&obs_rs);
        command.arg("flush").env("AH_OBS_SPOOL", spool);
        if let Some(bin_dir) = obs_rs.parent() {
            command.current_dir(bin_dir);
        }
        spawn_detached(command);
        return;
    }

    let worker = observability_client_dir(harness_root).join("flush_spool.py");
    if !worker.is_file() {
        return;
    }
    let mut command = Command::new(python_executable(env, harness_root));
    command.arg(&worker);
    if let Some(worker_dir) = worker.parent() {
        command.current_dir(worker_dir);
    }
    spawn_detached(command);
}

/// `harness-hook.cmd` resolution: `HARNESS_PYTHON`, else the first line of `var/python.path`, else the
/// pinned uv interpreter; a candidate that does not exist falls back to `python` on PATH.
fn python_executable(env: &BTreeMap<String, String>, harness_root: &Path) -> PathBuf {
    let configured = env
        .get("HARNESS_PYTHON")
        .filter(|path| !path.is_empty())
        .cloned()
        .or_else(|| python_path_file(harness_root))
        .or_else(|| pinned_uv_python(env));
    configured
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from("python"))
}

#[inline]
fn python_path_file(harness_root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(harness_root.join("var").join("python.path")).ok()?;
    let first_line = text.lines().next()?.trim();
    (!first_line.is_empty()).then(|| first_line.to_owned())
}

#[inline]
fn pinned_uv_python(env: &BTreeMap<String, String>) -> Option<String> {
    let app_data = env.get("APPDATA").filter(|path| !path.is_empty())?;
    Some(format!(
        "{app_data}\\uv\\python\\cpython-3.14.7-windows-x86_64-none\\python.exe"
    ))
}

fn spawn_detached(mut command: Command) {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    detach(&mut command);
    let _ = command.spawn();
}

#[cfg(windows)]
#[inline]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(DETACHED_NO_WINDOW);
}

#[cfg(not(windows))]
#[inline]
fn detach(_command: &mut Command) {}
