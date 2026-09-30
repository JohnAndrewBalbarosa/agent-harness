//! `<label>.stdout.log`, `<label>.stderr.log` and `<label>.state.json`
//! (interval_supervisor.py:180-185, :241-250, :301-302).
//!
//! The running state keeps Python's key order: label, pid, status, intervalSeconds, stdout,
//! stderr, startedAt. The final state is `state.update(record)`: existing keys keep their place,
//! record-only keys are appended.

use std::path::{Path, PathBuf};

use obs_cli::python_compat::{to_python_json, JsonLayout};
use serde_json::{Map, Value};

use crate::bounded_log::write_text;
use crate::completion_record::path_text;
use crate::python_text::is_python_alnum;

const REPLACEMENT_CHARACTER: char = '_';

/// Where one supervised run keeps its diagnostics.
#[derive(Debug, Clone)]
pub struct LogFiles {
    pub stdout: PathBuf,
    pub stderr: PathBuf,
    pub state: PathBuf,
}

impl LogFiles {
    pub fn for_label(log_dir: &Path, safe_label: &str) -> Self {
        Self {
            stdout: log_dir.join(format!("{safe_label}.stdout.log")),
            stderr: log_dir.join(format!("{safe_label}.stderr.log")),
            state: log_dir.join(format!("{safe_label}.state.json")),
        }
    }
}

/// `ch if ch.isalnum() or ch in "-_" else "_"`.
pub fn sanitize_label(label: &str) -> String {
    label
        .chars()
        .map(|character| {
            if is_python_alnum(character) || character == '-' || character == '_' {
                character
            } else {
                REPLACEMENT_CHARACTER
            }
        })
        .collect()
}

/// The `status: running` document written at start and on every checkpoint.
pub fn running_state(
    safe_label: &str,
    pid: u32,
    interval_seconds: u64,
    files: &LogFiles,
    started_at_epoch_seconds: f64,
) -> Map<String, Value> {
    let mut state = Map::new();
    state.insert("label".into(), Value::from(safe_label));
    state.insert("pid".into(), Value::from(pid));
    state.insert("status".into(), Value::from("running"));
    state.insert("intervalSeconds".into(), Value::from(interval_seconds));
    state.insert("stdout".into(), Value::from(path_text(&files.stdout)));
    state.insert("stderr".into(), Value::from(path_text(&files.stderr)));
    state.insert("startedAt".into(), Value::from(started_at_epoch_seconds));
    state
}

/// Python `state.update(record)` (insertion order preserved by serde_json `preserve_order`).
pub fn merged_final_state(
    running: &Map<String, Value>,
    record: &Map<String, Value>,
) -> Map<String, Value> {
    let mut merged = running.clone();
    for (key, value) in record {
        merged.insert(key.clone(), value.clone());
    }
    merged
}

/// `json.dumps(document, indent=2)` written in Python text mode.
pub fn write_state(path: &Path, document: &Map<String, Value>) -> anyhow::Result<()> {
    let text = to_python_json(&Value::Object(document.clone()), JsonLayout::Indented, true);
    write_text(path, &text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_is_sanitized_like_python() {
        assert_eq!(sanitize_label("npm test:unit/a b"), "npm_test_unit_a_b");
        assert_eq!(sanitize_label("build-1_x"), "build-1_x");
    }

    #[test]
    fn final_state_keeps_running_key_order_then_appends_record_keys() {
        let files = LogFiles::for_label(Path::new("logs"), "job");
        let running = running_state("job", 42, 30, &files, 1.5);
        let mut record = Map::new();
        record.insert("label".into(), Value::from("job"));
        record.insert("status".into(), Value::from("succeeded"));
        record.insert("exit_code".into(), Value::from(0));

        let merged = merged_final_state(&running, &record);
        let keys: Vec<&str> = merged.keys().map(String::as_str).collect();

        assert_eq!(
            keys,
            [
                "label",
                "pid",
                "status",
                "intervalSeconds",
                "stdout",
                "stderr",
                "startedAt",
                "exit_code"
            ]
        );
        assert_eq!(merged["status"], "succeeded");
    }
}
