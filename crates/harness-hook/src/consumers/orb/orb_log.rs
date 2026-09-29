//! `status-orb.log` (`status_common.py` `get_logger`/`log`/`checkpoint`): the same file `orb.py` appends to.
//! Python's `RotatingFileHandler(maxBytes=1_000_000, backupCount=3)` with format
//! `%(asctime)s %(levelname)s %(message)s` (local time, `YYYY-MM-DD HH:MM:SS,mmm`) in text mode, so each line
//! ends with the platform newline. Logging never fails the hook.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use obs_cli::python_compat::{to_python_json, with_platform_newlines, JsonLayout};
use serde_json::Value;

pub const MAX_BYTES: u64 = 1_000_000;
pub const BACKUP_COUNT: usize = 3;

pub struct OrbLog {
    path: PathBuf,
}

impl OrbLog {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// `log(message)` at INFO.
    pub fn info(&self, message: &str) {
        let line = format!("{} INFO {message}", python_asctime());
        let _ = append_record(&self.path, &line);
    }

    /// `checkpoint(name, **detail)`: `checkpoint=<name> key=<json.dumps(value, ensure_ascii=True)> ...`.
    pub fn checkpoint(&self, name: &str, fields: &[(&str, Value)]) {
        self.info(&checkpoint_message(name, fields));
    }
}

pub fn checkpoint_message(name: &str, fields: &[(&str, Value)]) -> String {
    let rendered: Vec<String> = fields
        .iter()
        .map(|(key, value)| format!("{key}={}", to_python_json(value, JsonLayout::Spaced, true)))
        .collect();
    if rendered.is_empty() {
        format!("checkpoint={name}")
    } else {
        format!("checkpoint={name} {}", rendered.join(" "))
    }
}

/// `logging.Formatter` default `asctime`.
#[inline]
fn python_asctime() -> String {
    chrono::Local::now()
        .format("%Y-%m-%d %H:%M:%S,%3f")
        .to_string()
}

fn append_record(path: &Path, formatted: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    rotate_when_full(path, formatted)?;
    let line = with_platform_newlines(&format!("{formatted}\n"));
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(line.as_bytes())
}

// Mental model: `shouldRollover` = size + len("%s\n") >= maxBytes; `doRollover` shifts .2→.3, .1→.2, live→.1.
fn rotate_when_full(path: &Path, formatted: &str) -> std::io::Result<()> {
    let size = std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    let message_chars = formatted.chars().count() as u64 + 1;
    if size + message_chars < MAX_BYTES {
        return Ok(());
    }
    for index in (1..BACKUP_COUNT).rev() {
        let source = backup_path(path, index);
        if source.exists() {
            let target = backup_path(path, index + 1);
            let _ = std::fs::remove_file(&target);
            std::fs::rename(&source, &target)?;
        }
    }
    let first = backup_path(path, 1);
    let _ = std::fs::remove_file(&first);
    if path.exists() {
        std::fs::rename(path, &first)?;
    }
    Ok(())
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
    use serde_json::json;

    #[test]
    fn checkpoint_fields_use_python_json_dumps() {
        let message = checkpoint_message(
            "hook_processed",
            &[
                ("event", json!("Stop")),
                ("ms", json!(1.5)),
                ("x", json!({"a": "é"})),
            ],
        );
        assert_eq!(
            message,
            r#"checkpoint=hook_processed event="Stop" ms=1.5 x={"a": "\u00e9"}"#
        );
        assert_eq!(checkpoint_message("bare", &[]), "checkpoint=bare");
    }

    #[test]
    fn rotates_at_one_megabyte_keeping_three_backups() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("status-orb.log");
        std::fs::write(&log_path, vec![b'a'; MAX_BYTES as usize]).unwrap();
        OrbLog::new(log_path.clone()).info("hello");
        assert!(backup_path(&log_path, 1).exists());
        assert!(std::fs::read_to_string(&log_path)
            .unwrap()
            .contains(" INFO hello"));
    }
}
