//! Bounded, line-atomic lifecycle JSONL (`core/lifecycle.py`): compact JSON
//! `{timestamp, component, operation, outcome, details}`, written under the sidecar lock, rotated at
//! 1 MiB keeping 2 backups. Logging never fails the hook.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use obs_cli::python_compat::json_text::{to_python_json, JsonLayout};
use serde_json::{json, Map, Value};

use super::file_lock::with_sidecar_lock;
use crate::events::utc_now_millis;

pub const MAX_BYTES: u64 = 1_048_576;
pub const BACKUPS: usize = 2;

static THREAD_LOCK: Mutex<()> = Mutex::new(());

/// `lifecycle.emit(path, component, operation, outcome, **details)`; `details` keeps insertion order.
pub fn emit(
    path: &Path,
    component: &str,
    operation: &str,
    outcome: &str,
    details: Map<String, Value>,
) {
    let record = json!({
        "timestamp": utc_now_millis(),
        "component": component,
        "operation": operation,
        "outcome": outcome,
        "details": Value::Object(details),
    });
    let mut line = to_python_json(&record, JsonLayout::Compact, false);
    line.push('\n');

    let _thread_guard = THREAD_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _ = append_line(path, line.as_bytes());
}

/// Builds the `details` map from `(key, value)` pairs in order.
pub fn details<const N: usize>(pairs: [(&str, Value); N]) -> Map<String, Value> {
    pairs
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

fn append_line(path: &Path, line: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    with_sidecar_lock(path, || {
        rotate_when_full(path)?;
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?
            .write_all(line)
    })
}

// Mental model: at MAX_BYTES shift `.1` → `.2` (dropping the oldest), then the live file → `.1`.
fn rotate_when_full(path: &Path) -> std::io::Result<()> {
    let is_full = std::fs::metadata(path).is_ok_and(|meta| meta.len() >= MAX_BYTES);
    if !is_full {
        return Ok(());
    }
    for index in (2..=BACKUPS).rev() {
        let older = backup_path(path, index - 1);
        if older.exists() {
            std::fs::rename(&older, backup_path(path, index))?;
        }
    }
    std::fs::rename(path, backup_path(path, 1))
}

#[inline]
fn backup_path(path: &Path, index: usize) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(format!(".{index}"));
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_compact_records_in_python_key_order() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("hook-router.lifecycle.jsonl");
        emit(
            &log,
            "router",
            "hook.dispatch",
            "succeeded",
            details([("agent", json!("claude")), ("durationMs", json!(1.5))]),
        );
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains(r#""component":"router","operation":"hook.dispatch","outcome":"succeeded","details":{"agent":"claude","durationMs":1.5}}"#));
        assert!(text.ends_with('\n'));
    }

    #[test]
    fn rotates_at_the_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("x.jsonl");
        std::fs::write(&log, vec![b'a'; MAX_BYTES as usize]).unwrap();
        emit(&log, "c", "o", "succeeded", Map::new());
        assert!(backup_path(&log, 1).exists());
        assert!(std::fs::metadata(&log).unwrap().len() < 1024);
    }
}
