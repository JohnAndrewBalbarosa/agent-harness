//! `~/.codex-shared/var/logs/console-window.lifecycle.jsonl` writer with 1 MiB x 2 rotation (Python `emit`).

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use super::monitor_records::MonitorRecord;

pub const COMPONENT: &str = "console-window-monitor";
pub const LOG_FILE_NAME: &str = "console-window.lifecycle.jsonl";
pub const MAX_LOG_BYTES: u64 = 1_048_576;
pub const BACKUP_COUNT: u32 = 2;

/// Append-only JSONL log for the monitor. Writing never panics; failures come back as `io::Error`.
#[derive(Debug, Clone)]
pub struct MonitorLog {
    path: PathBuf,
}

impl MonitorLog {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// `<home>/.codex-shared/var/logs/console-window.lifecycle.jsonl`.
    pub fn under_home(home: &Path) -> Self {
        Self::new(
            home.join(".codex-shared")
                .join("var")
                .join("logs")
                .join(LOG_FILE_NAME),
        )
    }

    /// Resolves home from `USERPROFILE`, then `HOME` (empty values are ignored); `None` when neither is set.
    pub fn from_env(lookup: impl Fn(&str) -> Option<String>) -> Option<Self> {
        for key in ["USERPROFILE", "HOME"] {
            match lookup(key) {
                Some(home) if !home.is_empty() => return Some(Self::under_home(Path::new(&home))),
                _ => continue,
            }
        }
        None
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Writes one record stamped with the current UTC second.
    pub fn emit(&self, record: &MonitorRecord) -> io::Result<()> {
        let timestamp = utc_timestamp_now();
        let line = render_line(&timestamp, record)?;
        self.append_line(&line)
    }

    /// Mental model: ensure the directory → rotate when full → append one line.
    fn append_line(&self, line: &str) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        rotate_if_needed(&self.path, MAX_LOG_BYTES, BACKUP_COUNT)?;
        let mut stream = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        stream.write_all(line.as_bytes())
    }
}

/// `YYYY-MM-DDTHH:MM:SSZ` (seconds precision, like Python `time.strftime`).
#[inline]
pub fn utc_timestamp_now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// The full record object, keys in Python order: timestamp, severity, component, operation, outcome, details.
pub fn build_record(timestamp: &str, record: &MonitorRecord) -> Value {
    let mut object = Map::new();
    object.insert("timestamp".into(), Value::from(timestamp));
    object.insert("severity".into(), Value::from(record.severity));
    object.insert("component".into(), Value::from(COMPONENT));
    object.insert("operation".into(), Value::from(record.operation));
    object.insert("outcome".into(), Value::from(record.outcome));
    object.insert("details".into(), Value::Object(record.details.clone()));
    Value::Object(object)
}

/// Compact JSON (no spaces, non-ASCII kept) plus `\n`.
pub fn render_line(timestamp: &str, record: &MonitorRecord) -> io::Result<String> {
    let mut line = serde_json::to_string(&build_record(timestamp, record))?;
    line.push('\n');
    Ok(line)
}

/// When `path` is at least `max_bytes`: shift `.{n-1}` → `.{n}` down to `.1` → `.2`, then `path` → `.1`.
pub fn rotate_if_needed(path: &Path, max_bytes: u64, backups: u32) -> io::Result<()> {
    let size = match fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if size < max_bytes || backups == 0 {
        return Ok(());
    }
    for index in (2..=backups).rev() {
        let older = backup_path(path, index - 1);
        if older.exists() {
            fs::rename(&older, backup_path(path, index))?;
        }
    }
    fs::rename(path, backup_path(path, 1))
}

/// `<path>.<index>`.
#[inline]
pub fn backup_path(path: &Path, index: u32) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{index}"));
    PathBuf::from(name)
}
