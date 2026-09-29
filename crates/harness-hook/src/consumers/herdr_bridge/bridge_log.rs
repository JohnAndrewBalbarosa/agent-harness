//! `var/logs/herdr-bridge.lifecycle.jsonl` (`tools/herdr-bridge/common.py` `log_event`), shared with the
//! Python subscriber: compact `ensure_ascii` JSON `{timestamp, severity, component, event, outcome, **fields}`,
//! text-mode (platform newline), rotated to `.jsonl.1` once the file exceeds 5 MiB. Logging never fails.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use obs_cli::python_compat::{to_python_json, with_platform_newlines, JsonLayout};
use serde_json::{Map, Value};

pub const ROTATE_ABOVE_BYTES: u64 = 5 * 1024 * 1024;
const COMPONENT: &str = "herdr-bridge";

pub struct BridgeLog {
    path: PathBuf,
}

impl BridgeLog {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// `<root>/var/logs/herdr-bridge.lifecycle.jsonl`.
    pub fn under_root(harness_root: &Path) -> Self {
        Self::new(
            harness_root
                .join("var")
                .join("logs")
                .join("herdr-bridge.lifecycle.jsonl"),
        )
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `log_event(event, outcome, **fields)`; `fields` keep their order.
    pub fn event(&self, event: &str, outcome: &str, fields: Vec<(&str, Value)>) {
        let line = render_record(&utc_now_seconds_text(), event, outcome, fields);
        let _ = append_line(&self.path, &line);
    }
}

/// The JSON line (without newline) Python writes.
pub fn render_record(
    timestamp: &str,
    event: &str,
    outcome: &str,
    fields: Vec<(&str, Value)>,
) -> String {
    let mut record = Map::new();
    record.insert("timestamp".into(), Value::from(timestamp));
    let severity = if outcome == "failed" { "error" } else { "info" };
    record.insert("severity".into(), Value::from(severity));
    record.insert("component".into(), Value::from(COMPONENT));
    record.insert("event".into(), Value::from(event));
    record.insert("outcome".into(), Value::from(outcome));
    for (key, value) in fields {
        record.insert(key.to_owned(), value);
    }
    to_python_json(&Value::Object(record), JsonLayout::Compact, true)
}

/// `time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())`.
#[inline]
pub fn utc_now_seconds_text() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    rotate_when_oversized(path)?;
    let text = with_platform_newlines(&format!("{line}\n"));
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(text.as_bytes())
}

// Mental model: size > 5 MiB → drop the old `.jsonl.1`, move the live file there.
fn rotate_when_oversized(path: &Path) -> std::io::Result<()> {
    let is_oversized = std::fs::metadata(path).is_ok_and(|meta| meta.len() > ROTATE_ABOVE_BYTES);
    if !is_oversized {
        return Ok(());
    }
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".1");
    let rotated = path.with_file_name(name);
    let _ = std::fs::remove_file(&rotated);
    std::fs::rename(path, rotated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn record_keeps_python_key_order_and_ascii_escaping() {
        let line = render_record(
            "2026-09-30T01:02:03Z",
            "state.projected",
            "failed",
            vec![("session", json!("s1")), ("detail", json!("é"))],
        );
        assert_eq!(
            line,
            r#"{"timestamp":"2026-09-30T01:02:03Z","severity":"error","component":"herdr-bridge","event":"state.projected","outcome":"failed","session":"s1","detail":"\u00e9"}"#
        );
    }

    #[test]
    fn rotates_above_five_mebibytes() {
        let dir = tempfile::tempdir().unwrap();
        let log = BridgeLog::new(dir.path().join("herdr-bridge.lifecycle.jsonl"));
        std::fs::write(log.path(), vec![b'a'; ROTATE_ABOVE_BYTES as usize + 1]).unwrap();
        log.event("x", "succeeded", vec![]);
        assert!(dir.path().join("herdr-bridge.lifecycle.jsonl.1").exists());
        assert!(std::fs::metadata(log.path()).unwrap().len() < 1024);
    }
}
