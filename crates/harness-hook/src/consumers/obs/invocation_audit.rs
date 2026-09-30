//! hook.py `record_hook_invocation`: a bounded, payload-free health record per hook run in
//! `<var>/hook-invocations.jsonl` (newest 500 records, rewritten via `hook-invocations.tmp` + replace).
//! Audit failures are ignored, like Python's `except OSError: pass`.

use std::path::Path;
use std::time::Instant;

use obs_cli::python_compat::{
    python_utc_now, to_python_json, truncate_chars, with_platform_newlines, JsonLayout,
};
use serde_json::{json, Value};

use crate::shared_state::atomic_write::{replace_file, unique_temp_name};
use crate::shared_state::file_lock::with_sidecar_lock_within;

pub const AUDIT_FILE_NAME: &str = "hook-invocations.jsonl";
/// `HOOK_AUDIT_PATH.with_suffix(".tmp")`.
const AUDIT_TEMP_BASE: &str = "hook-invocations";
const AUDIT_LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
const MAX_RECORDS: usize = 500;
const ERROR_TYPE_LIMIT: usize = 120;

/// Mental model: build the record → keep the newest 499 existing lines → append → replace the file.
pub fn record(var_dir: &Path, hook_event: &str, started: Instant, error: Option<&anyhow::Error>) {
    let line = to_python_json(
        &audit_record(hook_event, started, error),
        JsonLayout::Compact,
        true,
    );
    let audit_path = var_dir.join(AUDIT_FILE_NAME);

    // Read-modify-write under a short sidecar lock so concurrent hooks do not drop each other's lines;
    // the audit is best effort, so a busy lock skips the record instead of delaying the hook.
    let _ = with_sidecar_lock_within(&audit_path, AUDIT_LOCK_TIMEOUT, || {
        let mut retained = retained_lines(&audit_path);
        retained.push(line);
        // Python `write_text` translates "\n" to the platform line ending (CRLF on Windows).
        let text = with_platform_newlines(&(retained.join("\n") + "\n"));
        replace_file(
            &audit_path,
            &unique_temp_name(AUDIT_TEMP_BASE),
            text.as_bytes(),
            false,
        )
    });
}

fn audit_record(hook_event: &str, started: Instant, error: Option<&anyhow::Error>) -> Value {
    let mut record = json!({
        "triggeredAt": python_isoformat_with_offset(),
        "hookEvent": if hook_event.is_empty() { "unknown" } else { hook_event },
        "outcome": if error.is_some() { "failed" } else { "succeeded" },
        "durationMs": rounded_millis(started),
    });
    if let (Some(error), Some(fields)) = (error, record.as_object_mut()) {
        fields.insert(
            "errorType".into(),
            Value::String(truncate_chars(python_error_type(error), ERROR_TYPE_LIMIT)),
        );
    }
    record
}

/// The newest `MAX_RECORDS - 1` non-blank lines (`deque(maxlen=499)`), right-stripped of `\n`.
fn retained_lines(audit_path: &Path) -> Vec<String> {
    let Ok(existing) = std::fs::read_to_string(audit_path) else {
        return Vec::new();
    };
    let lines: Vec<&str> = existing
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let skip = lines.len().saturating_sub(MAX_RECORDS - 1);
    lines[skip..]
        .iter()
        .map(|line| (*line).to_owned())
        .collect()
}

/// `datetime.now(timezone.utc).isoformat()` (`+00:00`, not `Z`).
#[inline]
fn python_isoformat_with_offset() -> String {
    let now = python_utc_now();
    match now.strip_suffix('Z') {
        Some(stem) => format!("{stem}+00:00"),
        None => now,
    }
}

/// `round((perf_counter() - started) * 1000, 3)`.
#[inline]
fn rounded_millis(started: Instant) -> f64 {
    let millis = started.elapsed().as_secs_f64() * 1000.0;
    (millis * 1000.0).round() / 1000.0
}

/// The Python exception class name hook.py would have recorded for the equivalent failure.
fn python_error_type(error: &anyhow::Error) -> &'static str {
    if error.downcast_ref::<rusqlite::Error>().is_some() {
        "OperationalError"
    } else if error.downcast_ref::<std::io::Error>().is_some() {
        "OSError"
    } else {
        "RuntimeError"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_newest_records_and_never_copies_payload() {
        let dir = tempfile::tempdir().unwrap();
        let old: Vec<String> = (0..MAX_RECORDS)
            .map(|index| format!("{{\"n\":{index}}}"))
            .collect();
        std::fs::write(dir.path().join(AUDIT_FILE_NAME), old.join("\n") + "\n").unwrap();

        record(dir.path(), "SessionStart", Instant::now(), None);

        let text = std::fs::read_to_string(dir.path().join(AUDIT_FILE_NAME)).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), MAX_RECORDS);
        assert_eq!(lines[0], "{\"n\":1}");
        let last: Value = serde_json::from_str(lines[MAX_RECORDS - 1]).unwrap();
        assert_eq!(last["hookEvent"], "SessionStart");
        assert_eq!(last["outcome"], "succeeded");
        assert!(last["durationMs"].as_f64().unwrap() >= 0.0);
        assert!(last["triggeredAt"].as_str().unwrap().ends_with("+00:00"));
    }

    #[test]
    fn failures_record_a_bounded_error_type() {
        let dir = tempfile::tempdir().unwrap();
        let error = anyhow::Error::new(std::io::Error::other("denied"));
        record(dir.path(), "", Instant::now(), Some(&error));
        let text = std::fs::read_to_string(dir.path().join(AUDIT_FILE_NAME)).unwrap();
        let last: Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(
            (
                last["hookEvent"].clone(),
                last["outcome"].clone(),
                last["errorType"].clone()
            ),
            (json!("unknown"), json!("failed"), json!("OSError"))
        );
    }
}
