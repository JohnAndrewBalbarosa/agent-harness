//! Per-session pending flags and the global rate window under `AH_NOTIFY_DIR` (`core/notify/consumer.py`).
//!
//! - `<agent>-<session≤120>.json` = `{"pending": <bool>}`
//! - `_rate.json`                  = `[<epoch seconds>, ...]`
//!
//! Both are `json.dumps` text (default separators, ASCII) written through `<stem>.tmp` + replace, and
//! read-modify-written by the caller under the `notify-state.lock` sidecar.

use std::io;
use std::path::Path;

use obs_cli::python_compat::{to_python_json, JsonLayout};
use serde_json::{json, Value};

use super::decide::NotifyState;
use crate::events::is_python_truthy;
use crate::shared_state::atomic_write::replace_file;

pub const RATE_FILE: &str = "_rate.json";
const SESSION_NAME_LIMIT: usize = 120;

/// `f"{_UNSAFE.sub('_', agent)}-{_UNSAFE.sub('_', session_id)[:120]}.json"` (also used by the tasks
/// store and the context guard).
pub fn session_file_name(agent: &str, session_id: &str) -> String {
    let session: String = sanitize(session_id)
        .chars()
        .take(SESSION_NAME_LIMIT)
        .collect();
    format!("{}-{session}.json", sanitize(agent))
}

/// `re.sub(r"[^A-Za-z0-9_-]", "_", text)`, one `_` per character.
#[inline]
pub fn sanitize(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                character
            } else {
                '_'
            }
        })
        .collect()
}

/// Loads the session flag and the rate window; unreadable or malformed files are defaults.
pub fn load_state(session_path: &Path, rate_path: &Path) -> NotifyState {
    let pending = read_json(session_path)
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|state| state.get("pending"))
        .is_some_and(is_python_truthy);
    let recent_toasts = read_json(rate_path)
        .as_ref()
        .and_then(Value::as_array)
        .map(|stamps| stamps.iter().filter_map(python_int_stamp).collect())
        .unwrap_or_default();
    NotifyState {
        pending,
        recent_toasts,
    }
}

/// Writes both files (session first, like Python).
pub fn save_state(session_path: &Path, rate_path: &Path, state: &NotifyState) -> io::Result<()> {
    write_json_replacing(session_path, &json!({"pending": state.pending}))?;
    write_json_replacing(rate_path, &json!(state.recent_toasts))
}

/// `path.with_suffix(".tmp").write_text(json.dumps(value))` then `os.replace`.
pub fn write_json_replacing(path: &Path, value: &Value) -> io::Result<()> {
    let text = to_python_json(value, JsonLayout::Spaced, true);
    replace_file(path, &temp_name_for(path), text.as_bytes(), false)
}

/// `path.with_suffix(".tmp").name`.
#[inline]
pub fn temp_name_for(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!("{stem}.tmp")
}

/// `json.loads(path.read_text("utf-8"))`, `None` on any read/decode/parse failure.
#[inline]
pub fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// `int(t) for t in stamps if isinstance(t, (int, float))` (bool is an int in Python).
#[inline]
fn python_int_stamp(stamp: &Value) -> Option<i64> {
    match stamp {
        Value::Bool(flag) => Some(i64::from(*flag)),
        Value::Number(number) => number.as_i64().or_else(|| {
            number
                .as_f64()
                .filter(|float| float.is_finite())
                .map(|float| float.trunc() as i64)
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_sanitized_and_truncated() {
        assert_eq!(session_file_name("claude", "a.b/c"), "claude-a_b_c.json");
        let long = "x".repeat(200);
        assert_eq!(
            session_file_name("codex", &long).len(),
            "codex-".len() + 120 + ".json".len()
        );
        assert_eq!(
            temp_name_for(Path::new("/n/claude-s1.json")),
            "claude-s1.tmp"
        );
        assert_eq!(
            temp_name_for(Path::new("/n/notify-events.jsonl")),
            "notify-events.tmp"
        );
    }

    #[test]
    fn state_round_trips_in_python_text() {
        let dir = tempfile::tempdir().unwrap();
        let session = dir.path().join("claude-s1.json");
        let rate = dir.path().join(RATE_FILE);
        let state = NotifyState {
            pending: true,
            recent_toasts: vec![1, 2],
        };
        save_state(&session, &rate, &state).unwrap();
        assert_eq!(
            std::fs::read_to_string(&session).unwrap(),
            r#"{"pending": true}"#
        );
        assert_eq!(std::fs::read_to_string(&rate).unwrap(), "[1, 2]");
        assert_eq!(load_state(&session, &rate), state);
    }

    #[test]
    fn malformed_files_load_as_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let session = dir.path().join("s.json");
        let rate = dir.path().join(RATE_FILE);
        std::fs::write(&session, "[1]").unwrap();
        std::fs::write(&rate, r#"[1.9, "x", true]"#).unwrap();
        let state = load_state(&session, &rate);
        assert!(!state.pending);
        assert_eq!(state.recent_toasts, vec![1, 1]);
    }
}
