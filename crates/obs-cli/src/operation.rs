//! obs.py:210 `operation(kind, data)`: the envelope every spooled operation carries.
//!
//! obs.py exempts raw fields (`rawPrompt`, `rawResult`, `rawPayload`, `snapshotToml`) from
//! redaction; none of the kinds obs-rs emits carry them, so the whole `data` is redacted.

use serde_json::{json, Value};

use crate::python_compat::{python_utc_now, redact, uuid7_text};

pub fn build_operation(kind: &str, data: &Value) -> Value {
    json!({
        "operationId": uuid7_text(),
        "kind": kind,
        "occurredAt": python_utc_now(),
        "data": redact(data),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_keeps_python_key_order_and_redacts() {
        let operation = build_operation("project.register", &json!({"slug": "token=abc"}));
        let keys: Vec<&str> = operation
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();

        assert_eq!(keys, ["operationId", "kind", "occurredAt", "data"]);
        assert_eq!(operation["data"]["slug"], "token=[REDACTED]");
    }
}
