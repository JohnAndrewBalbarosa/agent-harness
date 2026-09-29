//! obs.py:210 `operation(kind, data, occurred_at)`: the spooled envelope, with obs.py's lossless
//! raw-field exemptions (obs-cli's `build_operation` redacts everything, so it is not reused here).

use obs_cli::python_compat::{python_utc_now, redact, uuid7_text};
use serde_json::{json, Value};

/// obs.py `raw_fields`: copied verbatim after redaction (key position is kept).
const RAW_FIELDS: [(&str, &str); 4] = [
    ("prompt.begin", "rawPrompt"),
    ("prompt.finish", "rawResult"),
    ("event.emit", "rawPayload"),
    ("benchmark.init", "snapshotToml"),
];

pub fn operation(kind: &str, data: Value, occurred_at: Option<&str>) -> Value {
    json!({
        "operationId": uuid7_text(),
        "kind": kind,
        "occurredAt": occurred_at.map_or_else(python_utc_now, str::to_owned),
        "data": redact_except_raw_fields(kind, &data),
    })
}

/// Mental model: redact the whole object, then restore the kind's raw field from the original.
fn redact_except_raw_fields(kind: &str, data: &Value) -> Value {
    let mut safe_data = redact(data);
    let raw_fields = RAW_FIELDS.iter().filter(|(raw_kind, _)| *raw_kind == kind);

    for (_, field) in raw_fields {
        if let (Some(original), Some(target)) = (data.get(*field), safe_data.as_object_mut()) {
            target.insert((*field).to_owned(), original.clone());
        }
    }
    safe_data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_prompt_is_lossless_while_other_fields_are_redacted() {
        let long = "x".repeat(1500);
        let op = operation(
            "prompt.begin",
            json!({"promptId": "p", "rawPrompt": long, "note": "token=abc", "languageCode": "tl-en"}),
            None,
        );
        let data = op["data"].as_object().unwrap();
        assert_eq!(data["rawPrompt"].as_str().unwrap().len(), 1500);
        assert_eq!(data["note"], "token=[REDACTED]");
        let keys: Vec<&str> = data.keys().map(String::as_str).collect();
        assert_eq!(keys, ["promptId", "rawPrompt", "note", "languageCode"]);
    }

    #[test]
    fn raw_fields_are_exempt_only_for_their_kind() {
        let op = operation(
            "execution.finish",
            json!({"rawPayload": "token=abc"}),
            Some("2026-01-01T00:00:00Z"),
        );
        assert_eq!(op["data"]["rawPayload"], "token=[REDACTED]");
        assert_eq!(op["occurredAt"], "2026-01-01T00:00:00Z");
    }
}
