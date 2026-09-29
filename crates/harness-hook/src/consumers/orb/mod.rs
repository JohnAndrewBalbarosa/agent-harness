//! Status-orb hook side (`tools/codex-status-orb/hook.py`): records each session's status under the orb's
//! data root so the (still Python) `orb.py` GUI can render it, and starts the orb on `SessionStart`.
//!
//! Mental model:
//! native payload → claim (3 s duplicate window) → `SessionEnd` deletes the session file, a known status event
//! writes `sessions/<sid>.json` (+ `SessionStart` launches `orb.py`) → neutral stdout (`Stop` only).

pub mod data_paths;
pub mod dedupe;
pub mod orb_launcher;
pub mod orb_log;
pub mod process_context;
pub mod session_store;

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use obs_cli::python_compat::{python_repr_str, with_platform_newlines};
use serde_json::{Map, Value};

use crate::consumer::{Consumer, ConsumerResult};
use crate::events::{is_known_event, is_python_truthy, python_str};
use crate::hook_context::HookContext;

use data_paths::OrbDataPaths;
use orb_log::OrbLog;
use session_store::{safe_session_id, OrbSessionStore};

const NAME: &str = "orb";
/// hook.py:12 `STATUS_BY_EVENT`.
const STATUS_BY_EVENT: [(&str, &str); 5] = [
    ("SessionStart", "idle"),
    ("UserPromptSubmit", "running"),
    ("PermissionRequest", "needs_input"),
    ("PostToolUse", "running"),
    ("Stop", "ready"),
];
/// hook.py:30 `neutral_output` (only `Stop` answers; text-mode stdout writes CRLF on Windows).
const STOP_OUTPUT: &str = "{\"continue\":true}\n";

pub struct OrbConsumer;

impl Consumer for OrbConsumer {
    fn name(&self) -> &'static str {
        NAME
    }

    /// `core/router/consumers.py` `_orb`: every common event except the subagent pair.
    fn subscribes_to(&self, event: &str) -> bool {
        is_known_event(event) && !event.starts_with("subagent.")
    }

    /// Same install gate as the Python router: the orb's venv interpreter must exist.
    fn is_enabled(&self, context: &HookContext) -> bool {
        orb_python(&context.harness_root).is_file()
    }

    fn run(&self, context: &HookContext) -> anyhow::Result<ConsumerResult> {
        Ok(ConsumerResult::succeeded(NAME, handle_hook(context)))
    }
}

/// hook.py `main()`; returns the hook's stdout. Never fails: errors are logged like `hook-error ...`.
pub fn handle_hook(context: &HookContext) -> String {
    let hook_received_ns = now_ns();
    let payload = payload_object(&context.native_payload);
    let event_name = hook_event_name(&payload, context);
    let session_id = safe_session_id(first_truthy(&payload, &["session_id", "sessionId"]));
    let paths = OrbDataPaths::from_env(&context.env);
    let log = OrbLog::new(paths.log_path());

    if let Err(error) = process_event(
        context,
        &paths,
        &log,
        &payload,
        &event_name,
        &session_id,
        hook_received_ns,
    ) {
        log.info(&format!(
            "hook-error event={} session={} error={}",
            python_repr_str(&event_name),
            python_repr_str(&session_id),
            python_repr_str(&format!("{error:#}")),
        ));
    }

    let stdout = neutral_output(&event_name);
    log.checkpoint(
        "hook_returning",
        &[
            ("event", Value::from(event_name.as_str())),
            ("session", Value::from(session_id.as_str())),
            ("cli_resume_ms", Value::from(elapsed_ms(hook_received_ns))),
        ],
    );
    stdout
}

// Mental model: duplicate → checkpoint only; SessionEnd → delete; known status → write (+ launch on start).
fn process_event(
    context: &HookContext,
    paths: &OrbDataPaths,
    log: &OrbLog,
    payload: &Map<String, Value>,
    event_name: &str,
    session_id: &str,
    hook_received_ns: i64,
) -> anyhow::Result<()> {
    let store = OrbSessionStore::new(paths.sessions_dir());

    if !dedupe::claim_hook_event(&store, payload, now_seconds())? {
        log.checkpoint(
            "hook_duplicate_skipped",
            &event_fields(event_name, session_id),
        );
        return Ok(());
    }
    if event_name == "SessionEnd" {
        store.remove_session(session_id)?;
        return Ok(());
    }
    let Some(status) = status_for(event_name) else {
        return Ok(());
    };

    let record = session_record(
        context,
        payload,
        event_name,
        session_id,
        status,
        hook_received_ns,
        log,
    );
    store.write_session(record)?;

    let mut fields = event_fields(event_name, session_id);
    fields.push(("status", Value::from(status)));
    log.checkpoint("hook_processed", &fields);

    if event_name == "SessionStart" {
        orb_launcher::launch_orb(&context.harness_root, &context.env, log);
    }
    Ok(())
}

/// hook.py:48-63: the `current` dict, in Python key order.
fn session_record(
    context: &HookContext,
    payload: &Map<String, Value>,
    event_name: &str,
    session_id: &str,
    status: &str,
    hook_received_ns: i64,
    log: &OrbLog,
) -> Map<String, Value> {
    let pane_id = context
        .env
        .get("HERDR_PANE_ID")
        .cloned()
        .unwrap_or_default();
    let is_herdr_managed =
        context.env.get("HERDR_ENV").map(String::as_str) == Some("1") && !pane_id.is_empty();

    let mut record = Map::new();
    record.insert("session_id".into(), Value::from(session_id));
    record.insert("event".into(), Value::from(event_name));
    record.insert("status".into(), Value::from(status));
    record.insert("cwd".into(), Value::from(payload_cwd(payload)));
    record.insert(
        "transcript_path".into(),
        Value::from(
            first_truthy(payload, &["transcript_path", "transcriptPath"])
                .map(python_str)
                .unwrap_or_default(),
        ),
    );
    record.insert("event_ns".into(), Value::from(now_ns()));
    record.insert("updated_at".into(), Value::from(now_seconds()));
    record.insert("hook_received_ns".into(), Value::from(hook_received_ns));
    record.insert("herdr_managed".into(), Value::Bool(is_herdr_managed));
    record.insert("herdr_pane_id".into(), Value::from(pane_id));

    let wt_session = context.env.get("WT_SESSION").cloned().unwrap_or_default();
    process_context::current_process_context(&wt_session, log).append_to(&mut record);
    record.insert("state_written_ns".into(), Value::from(now_ns()));
    record
}

/// `<root>/tools/codex-status-orb/.venv/Scripts/python.exe` (router install gate).
#[inline]
pub fn orb_python(harness_root: &Path) -> PathBuf {
    orb_tool_root(harness_root)
        .join(".venv")
        .join("Scripts")
        .join("python.exe")
}

#[inline]
pub fn orb_tool_root(harness_root: &Path) -> PathBuf {
    harness_root.join("tools").join("codex-status-orb")
}

#[inline]
fn status_for(event_name: &str) -> Option<&'static str> {
    STATUS_BY_EVENT
        .iter()
        .find(|(name, _)| *name == event_name)
        .map(|(_, status)| *status)
}

#[inline]
fn neutral_output(event_name: &str) -> String {
    if event_name == "Stop" {
        with_platform_newlines(STOP_OUTPUT)
    } else {
        String::new()
    }
}

/// `payload.get("hook_event_name") or payload.get("hookEventName") or os.environ.get("CODEX_STATUS_EVENT") or ""`.
fn hook_event_name(payload: &Map<String, Value>, context: &HookContext) -> String {
    first_truthy(payload, &["hook_event_name", "hookEventName"])
        .map(python_str)
        .or_else(|| context.env_value("CODEX_STATUS_EVENT").map(str::to_owned))
        .unwrap_or_default()
}

/// `str(payload.get("cwd") or os.getcwd())`.
fn payload_cwd(payload: &Map<String, Value>) -> String {
    first_truthy(payload, &["cwd"])
        .map(python_str)
        .unwrap_or_else(|| {
            std::env::current_dir()
                .map(|path| path.display().to_string())
                .unwrap_or_default()
        })
}

/// `payload.get(a) or payload.get(b) or ...` (Python truthiness).
#[inline]
pub(crate) fn first_truthy<'a>(
    payload: &'a Map<String, Value>,
    names: &[&str],
) -> Option<&'a Value> {
    names
        .iter()
        .filter_map(|name| payload.get(*name))
        .find(|value| is_python_truthy(value))
}

#[inline]
fn payload_object(payload: &Value) -> Map<String, Value> {
    payload.as_object().cloned().unwrap_or_default()
}

#[inline]
fn event_fields(event_name: &str, session_id: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("event", Value::from(event_name)),
        ("session", Value::from(session_id)),
    ]
}

/// `time.time_ns()`.
#[inline]
pub(crate) fn now_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_nanos()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// `time.time()`.
#[inline]
pub(crate) fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs_f64())
        .unwrap_or(0.0)
}

/// `round((time.time_ns() - start) / 1_000_000, 3)`.
#[inline]
fn elapsed_ms(start_ns: i64) -> f64 {
    let milliseconds = (now_ns() - start_ns) as f64 / 1_000_000.0;
    (milliseconds * 1000.0).round() / 1000.0
}
