//! `var/herdr-bridge/sessions/<safe_id>.json` (`common.py` `read_json`/`atomic_json`, `hook.py` record merge),
//! read by the Python subscriber and the Herdr plugin.
//!
//! Format: `json.dumps(indent=2, ensure_ascii=True)` written with `Path.write_text` (text mode: platform
//! newlines, no trailing newline) to `<name>.json.<pid>.tmp`, then `os.replace`d.

use std::io;
use std::path::{Path, PathBuf};

use obs_cli::python_compat::{
    parse_python_int, to_python_json, with_platform_newlines, JsonLayout,
};
use serde_json::{Map, Value};

use super::state_machine::StateDecision;
use crate::shared_state::atomic_write::replace_file;

/// Everything the merged record needs besides the previous state.
#[derive(Debug, Clone)]
pub struct SessionIdentity {
    pub native_session: String,
    pub native_turn: String,
    pub project_id: String,
    pub profile: String,
    pub cwd: String,
    pub runtime_scope: &'static str,
    pub workspace_id: String,
    pub tab_id: String,
    pub pane_id: String,
}

/// `SESSION_ROOT = <root>/var/herdr-bridge/sessions`.
#[inline]
pub fn session_root(harness_root: &Path) -> PathBuf {
    harness_root
        .join("var")
        .join("herdr-bridge")
        .join("sessions")
}

/// `SESSION_ROOT / f"{native_session}.json"`.
#[inline]
pub fn session_state_path(harness_root: &Path, native_session: &str) -> PathBuf {
    session_root(harness_root).join(format!("{native_session}.json"))
}

/// `read_json(path)`: an object, else `{}`.
pub fn read_state(path: &Path) -> Map<String, Value> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

/// `atomic_json(path, value)`.
pub fn write_state(path: &Path, record: &Map<String, Value>) -> io::Result<()> {
    let text = with_platform_newlines(&to_python_json(
        &Value::Object(record.clone()),
        JsonLayout::Indented,
        true,
    ));
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp_name = format!("{name}.{}.tmp", std::process::id());
    replace_file(path, &temp_name, text.as_bytes(), false)
}

/// `int(previous.get("sequence_no", 0)) + 1`.
#[inline]
pub fn next_sequence(previous: &Map<String, Value>) -> i64 {
    let current = match previous.get("sequence_no") {
        Some(Value::Number(number)) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|float| float.trunc() as i64)),
        Some(Value::String(text)) => parse_python_int(text),
        Some(Value::Bool(flag)) => Some(i64::from(*flag)),
        _ => None,
    };
    current.unwrap_or(0) + 1
}

// Mental model: `{**previous, <identity fields>}` (existing keys keep their position), then the decision's
// fields; `state_entered_at` only moves when the canonical state changes.
pub fn merged_record(
    previous: &Map<String, Value>,
    identity: &SessionIdentity,
    decision: Option<&StateDecision>,
    sequence: i64,
    now: f64,
) -> Map<String, Value> {
    let mut record = previous.clone();
    let fields = [
        ("session_id", Value::from(identity.native_session.as_str())),
        ("turn_id", Value::from(identity.native_turn.as_str())),
        ("project_id", Value::from(identity.project_id.as_str())),
        ("profile", Value::from(identity.profile.as_str())),
        ("cwd", Value::from(identity.cwd.as_str())),
        ("runtime_scope", Value::from(identity.runtime_scope)),
        ("workspace_id", Value::from(identity.workspace_id.as_str())),
        ("tab_id", Value::from(identity.tab_id.as_str())),
        ("pane_id", Value::from(identity.pane_id.as_str())),
        ("sequence_no", Value::from(sequence)),
        ("updated_at", Value::from(now)),
    ];
    for (key, value) in fields {
        record.insert(key.to_owned(), value);
    }
    if let Some(decision) = decision {
        apply_decision(&mut record, previous, decision, now);
    }
    record
}

fn apply_decision(
    record: &mut Map<String, Value>,
    previous: &Map<String, Value>,
    decision: &StateDecision,
    now: f64,
) {
    let state_entered_at = if is_state_change(previous, decision) {
        Value::from(now)
    } else {
        previous
            .get("state_entered_at")
            .cloned()
            .unwrap_or_else(|| Value::from(now))
    };
    let fields = [
        ("canonical_state", Value::from(decision.canonical)),
        ("status", Value::from(decision.status())),
        ("projected_state", Value::from(decision.projected)),
        ("reason", Value::from(decision.reason.as_str())),
        ("authority", Value::from(decision.authority)),
        ("signal", Value::from(decision.signal.as_str())),
        ("state_entered_at", state_entered_at),
    ];
    for (key, value) in fields {
        record.insert(key.to_owned(), value);
    }
}

/// `previous.get("canonical_state") != decision.canonical`.
#[inline]
pub fn is_state_change(previous: &Map<String, Value>, decision: &StateDecision) -> bool {
    previous.get("canonical_state").and_then(Value::as_str) != Some(decision.canonical)
}

/// Python `float(value)`, with `fallback` where `float()` would raise.
#[inline]
pub fn python_float_or(value: &Value, fallback: f64) -> f64 {
    match value {
        Value::Number(number) => number.as_f64().unwrap_or(fallback),
        Value::Bool(flag) => f64::from(u8::from(*flag)),
        Value::String(text) => text.trim().parse().unwrap_or(fallback),
        _ => fallback,
    }
}
