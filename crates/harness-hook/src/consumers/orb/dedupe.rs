//! `claim_hook_event` (`status_common.py`): one lifecycle event is processed once even when duplicate
//! config layers fire the same hook. The fingerprint is the SHA-256 of Python's
//! `json.dumps(payload - {hook_source, hookSource}, sort_keys=True, separators=(",", ":"), ensure_ascii=True,
//! default=str)`; the same fingerprint within 3 s of the stored one is a duplicate.

use std::io;

use obs_cli::python_compat::{to_python_json, JsonLayout};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::first_truthy;
use super::session_store::{
    load_json, python_float, safe_session_id, save_json_atomic, OrbSessionStore,
};

pub const DUPLICATE_WINDOW_SECONDS: f64 = 3.0;
const SOURCE_KEYS: [&str; 2] = ["hook_source", "hookSource"];

/// Returns `false` when the same payload was claimed within the window; otherwise records it.
pub fn claim_hook_event(
    store: &OrbSessionStore,
    payload: &Map<String, Value>,
    now_seconds: f64,
) -> io::Result<bool> {
    let session_id = safe_session_id(first_truthy(payload, &["session_id", "sessionId"]));
    let fingerprint = payload_fingerprint(payload);
    let path = store.dedupe_path(&session_id);

    let _lock = store.lock_session(&session_id)?;
    if is_recent_duplicate(load_json(&path), &fingerprint, now_seconds) {
        return Ok(false);
    }
    save_json_atomic(
        &path,
        &json!({"fingerprint": fingerprint, "updated_at": now_seconds}),
    )?;
    Ok(true)
}

/// Hex SHA-256 of the canonical payload text.
pub fn payload_fingerprint(payload: &Map<String, Value>) -> String {
    hex::encode(Sha256::digest(canonical_payload_text(payload).as_bytes()))
}

/// The exact bytes Python hashes.
pub fn canonical_payload_text(payload: &Map<String, Value>) -> String {
    let canonical: Map<String, Value> = payload
        .iter()
        .filter(|(key, _)| !SOURCE_KEYS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    to_python_json(
        &with_sorted_keys(&Value::Object(canonical)),
        JsonLayout::Compact,
        true,
    )
}

/// `sort_keys=True`: every object, at every depth, re-inserted in code-point key order.
pub fn with_sorted_keys(value: &Value) -> Value {
    match value {
        Value::Object(entries) => {
            let mut keys: Vec<&String> = entries.keys().collect();
            keys.sort();
            let sorted: Map<String, Value> = keys
                .into_iter()
                .map(|key| (key.clone(), with_sorted_keys(&entries[key.as_str()])))
                .collect();
            Value::Object(sorted)
        }
        Value::Array(items) => Value::Array(items.iter().map(with_sorted_keys).collect()),
        other => other.clone(),
    }
}

/// `previous.fingerprint == fingerprint and now - float(previous.updated_at or 0) <= window`.
#[inline]
fn is_recent_duplicate(previous: Option<Value>, fingerprint: &str, now_seconds: f64) -> bool {
    let Some(Value::Object(previous)) = previous else {
        return false;
    };
    let is_same = previous.get("fingerprint").and_then(Value::as_str) == Some(fingerprint);
    is_same && now_seconds - python_float(previous.get("updated_at")) <= DUPLICATE_WINDOW_SECONDS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_text_sorts_keys_recursively_and_drops_hook_source() {
        let payload: Map<String, Value> = serde_json::from_str(
            r#"{"z": 1, "hook_source": "x", "a": {"y": [{"b": 2, "a": 1.5}], "x": "é"}}"#,
        )
        .unwrap();
        assert_eq!(
            canonical_payload_text(&payload),
            r#"{"a":{"x":"\u00e9","y":[{"a":1.5,"b":2}]},"z":1}"#
        );
    }

    #[test]
    fn fingerprint_matches_python_hashlib() {
        // python -c "import json,hashlib;print(hashlib.sha256(json.dumps({'session_id':'abc'},sort_keys=True,
        //   separators=(',',':'),ensure_ascii=True).encode()).hexdigest())"
        let payload: Map<String, Value> = serde_json::from_str(r#"{"session_id":"abc"}"#).unwrap();
        assert_eq!(
            payload_fingerprint(&payload),
            hex::encode(Sha256::digest(br#"{"session_id":"abc"}"#))
        );
    }
}
