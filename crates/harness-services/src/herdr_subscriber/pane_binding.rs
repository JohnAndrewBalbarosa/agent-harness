//! subscriber.py `binding_for_pane(pane_id)`: which bridge session (and project) owns a Herdr pane, read from
//! `var/herdr-bridge/sessions/*.json` written by the hook's herdr-bridge consumer.

use std::path::Path;

use harness_hook::consumers::herdr_bridge::session_state::{python_float_or, read_state};
use harness_hook::events::is_python_truthy;
use serde_json::{Map, Value};

const SESSION_FILE_EXTENSION: &str = "json";

/// The session record chosen for a pane.
#[derive(Debug, Clone, PartialEq)]
pub struct PaneBinding {
    /// `binding["project_id"]` as stored (normally a string).
    pub project_id: Value,
    /// `binding.get("session_id")`, `null` when absent.
    pub native_session: Value,
}

// Mental model: empty pane or no session dir → none; else every `*.json` object whose `pane_id` equals the pane
// and whose `project_id` is truthy; the newest `updated_at` wins (first seen on ties, like `max`).
pub fn binding_for_pane(session_root: &Path, pane_id: &str) -> Option<PaneBinding> {
    if pane_id.is_empty() {
        return None;
    }
    let entries = std::fs::read_dir(session_root).ok()?;
    let mut newest: Option<(f64, Map<String, Value>)> = None;
    for path in entries.flatten().map(|entry| entry.path()) {
        if !is_session_file(&path) {
            continue;
        }
        let record = read_state(&path);
        if !is_bound_to_pane(&record, pane_id) {
            continue;
        }
        let updated_at = updated_at_of(&record);
        let is_newer = newest.as_ref().is_none_or(|(best, _)| updated_at > *best);
        if is_newer {
            newest = Some((updated_at, record));
        }
    }
    newest.map(|(_, record)| binding_from(&record))
}

/// `SESSION_ROOT.glob("*.json")` (files only; `read_json` of a directory would give `{}` anyway).
#[inline]
fn is_session_file(path: &Path) -> bool {
    path.extension().and_then(|extension| extension.to_str()) == Some(SESSION_FILE_EXTENSION)
        && path.is_file()
}

/// `value.get("pane_id") == pane_id and value.get("project_id")`.
#[inline]
fn is_bound_to_pane(record: &Map<String, Value>, pane_id: &str) -> bool {
    let is_same_pane = record.get("pane_id").and_then(Value::as_str) == Some(pane_id);
    is_same_pane && record.get("project_id").is_some_and(is_python_truthy)
}

/// `float(item.get("updated_at", 0))`; a non-numeric value counts as 0 (Python would raise and reconnect).
#[inline]
fn updated_at_of(record: &Map<String, Value>) -> f64 {
    record
        .get("updated_at")
        .map_or(0.0, |value| python_float_or(value, 0.0))
}

#[inline]
fn binding_from(record: &Map<String, Value>) -> PaneBinding {
    PaneBinding {
        project_id: record.get("project_id").cloned().unwrap_or(Value::Null),
        native_session: record.get("session_id").cloned().unwrap_or(Value::Null),
    }
}
