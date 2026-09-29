//! Per-session orb state shared with `orb.py` (`status_common.py`): `sessions/<sid>.json`, the dedupe file,
//! and the `sessions/.<sid>.lock` file both sides lock.
//!
//! - JSON: `json.dump(indent=2, ensure_ascii=False)` + `"\n"`, LF line endings (`newline="\n"`), fsynced,
//!   written as `.<name>.<pid>.<ns>.tmp` then `os.replace`d.
//! - Lock: Python seeds the lock file with `"0"` and `msvcrt.locking`s byte 0; the Rust side takes an
//!   exclusive `LockFileEx` over the whole file (see `shared_state::file_lock`), which overlaps byte 0.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use obs_cli::python_compat::{parse_python_int, to_python_json, JsonLayout};
use serde_json::{Map, Value};

use crate::events::{is_python_truthy, python_str};
use crate::shared_state::atomic_write::replace_file;
use crate::shared_state::file_lock::{lock_sidecar, sidecar_path, SidecarLock};

const SESSION_ID_LIMIT: usize = 160;
const UNKNOWN_SESSION: &str = "unknown";
const LOCK_SEED: &[u8] = b"0";

pub struct OrbSessionStore {
    sessions_dir: PathBuf,
}

impl OrbSessionStore {
    pub fn new(sessions_dir: PathBuf) -> Self {
        Self { sessions_dir }
    }

    pub fn sessions_dir(&self) -> &Path {
        &self.sessions_dir
    }

    /// `state_path(session_id)`.
    pub fn state_path(&self, session_id: &str) -> PathBuf {
        self.sessions_dir
            .join(format!("{}.json", sanitize_session_id(session_id)))
    }

    /// `sessions/.<sid>.dedupe`.
    pub fn dedupe_path(&self, session_id: &str) -> PathBuf {
        self.sessions_dir.join(format!(".{session_id}.dedupe"))
    }

    /// `sessions/.<sid>.lock`.
    pub fn lock_path(&self, session_id: &str) -> PathBuf {
        sidecar_path(&self.lock_anchor(session_id))
    }

    /// `session_lock(session_id)`: seed the lock file with `"0"`, then hold it exclusively.
    pub fn lock_session(&self, session_id: &str) -> io::Result<SidecarLock> {
        std::fs::create_dir_all(&self.sessions_dir)?;
        seed_lock_file(&self.lock_path(session_id))?;
        lock_sidecar(&self.lock_anchor(session_id))
    }

    /// `write_session(event)`: rejects an event older than the stored one; returns whether it was written.
    pub fn write_session(&self, mut event: Map<String, Value>) -> io::Result<bool> {
        let session_id = safe_session_id(event.get("session_id"));
        event.insert("session_id".into(), Value::from(session_id.as_str()));
        let path = self.state_path(&session_id);

        let _lock = self.lock_session(&session_id)?;
        if is_older_than_stored(&path, &event) {
            return Ok(false);
        }
        save_json_atomic(&path, &Value::Object(event))?;
        Ok(true)
    }

    /// `remove_session(session_id)`.
    pub fn remove_session(&self, session_id: &str) -> io::Result<()> {
        let _lock = self.lock_session(session_id)?;
        match std::fs::remove_file(self.state_path(session_id)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    /// `sessions/.<sid>`, whose sidecar `.lock` is the Python lock file.
    #[inline]
    fn lock_anchor(&self, session_id: &str) -> PathBuf {
        self.sessions_dir
            .join(format!(".{}", sanitize_session_id(session_id)))
    }
}

/// `int(existing.get("event_ns", 0)) > int(event.get("event_ns", 0))`.
fn is_older_than_stored(path: &Path, event: &Map<String, Value>) -> bool {
    let Some(Value::Object(existing)) = load_json(path) else {
        return false;
    };
    python_int(existing.get("event_ns")) > python_int(event.get("event_ns"))
}

/// `safe_session_id(value)`: `str(value or "unknown")`, keep alphanumerics, `-` and `_`, first 160 chars.
pub fn safe_session_id(value: Option<&Value>) -> String {
    let raw = match value {
        Some(value) if is_python_truthy(value) => python_str(value),
        _ => UNKNOWN_SESSION.to_owned(),
    };
    sanitize_session_id(&raw)
}

#[inline]
fn sanitize_session_id(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|character| character.is_alphanumeric() || matches!(character, '-' | '_'))
        .take(SESSION_ID_LIMIT)
        .collect();
    if cleaned.is_empty() {
        UNKNOWN_SESSION.to_owned()
    } else {
        cleaned
    }
}

/// `save_json_atomic(path, value)`.
pub fn save_json_atomic(path: &Path, value: &Value) -> io::Result<()> {
    let mut text = to_python_json(value, JsonLayout::Indented, false);
    text.push('\n');
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp_name = format!(".{name}.{}.{}.tmp", std::process::id(), super::now_ns());
    replace_file(path, &temp_name, text.as_bytes(), true)
}

/// `load_json(path, default)`: `None` for a missing or unparseable file.
pub fn load_json(path: &Path) -> Option<Value> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Python `int(value)` for JSON values; anything `int()` would reject counts as 0.
pub fn python_int(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Number(number)) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|float| float.trunc() as i64))
            .unwrap_or(0),
        Some(Value::Bool(flag)) => i64::from(*flag),
        Some(Value::String(text)) => parse_python_int(text).unwrap_or(0),
        _ => 0,
    }
}

/// Python `float(value)` for JSON values; anything `float()` would reject counts as 0.0.
pub fn python_float(value: Option<&Value>) -> f64 {
    match value {
        Some(Value::Number(number)) => number.as_f64().unwrap_or(0.0),
        Some(Value::Bool(flag)) => f64::from(u8::from(*flag)),
        Some(Value::String(text)) => text.trim().parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// `open("a+b")` + write `"0"` when empty, so byte 0 exists for `msvcrt.locking`.
fn seed_lock_file(lock_path: &Path) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(lock_path)?;
    if file.metadata()?.len() == 0 {
        file.write_all(LOCK_SEED)?;
        file.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn session_id_is_filename_safe() {
        assert_eq!(safe_session_id(Some(&json!("../bad:id"))), "badid");
        assert_eq!(safe_session_id(Some(&json!(""))), "unknown");
        assert_eq!(safe_session_id(None), "unknown");
        assert_eq!(safe_session_id(Some(&json!("::"))), "unknown");
    }

    #[test]
    fn lock_file_is_seeded_with_zero_and_named_like_python() {
        let dir = tempfile::tempdir().unwrap();
        let store = OrbSessionStore::new(dir.path().join("sessions"));
        drop(store.lock_session("abc").unwrap());
        let lock = dir.path().join("sessions").join(".abc.lock");
        assert_eq!(std::fs::read(lock).unwrap(), b"0");
    }

    #[test]
    fn saved_json_uses_indent_two_and_a_trailing_newline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.json");
        save_json_atomic(&path, &json!({"a": 1, "é": [true]})).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\n  \"a\": 1,\n  \"é\": [\n    true\n  ]\n}\n"
        );
        let leftovers = std::fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(leftovers, 1);
    }
}
