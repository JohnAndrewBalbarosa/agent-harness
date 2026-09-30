//! receiver.py:70 `usage_records(payload, resolve_project)`: decoded log records → adapter-mapped
//! `turn-usage.record` data, each with `projectId` appended last (Python `{**data, "projectId": ...}`).

use harness_hook::consumers::usage_fallback::usage_record::global_project_id;
use serde_json::Value;

use super::adapter_dispatch::adapter_for;
use super::otlp_decoding::decode_log_records;
use super::python_semantics::ShapeError;

/// `resolve_project` returns the registered project of a native session, or `None`/empty for unknown
/// sessions (→ global project).
pub fn usage_records<F>(payload: &Value, mut resolve_project: F) -> Result<Vec<Value>, ShapeError>
where
    F: FnMut(&str) -> Option<String>,
{
    // Mental model: decode (may raise like Python) → skip unknown services and unmapped events →
    // project per record.
    let mut records = Vec::new();
    for decoded in decode_log_records(payload)? {
        let Some(adapter) = adapter_for(&decoded.service_name) else {
            continue;
        };
        let Some(data) = adapter.map_attributes(&decoded.attributes) else {
            continue;
        };
        records.push(with_project(data, &mut resolve_project));
    }
    Ok(records)
}

#[inline]
fn with_project<F>(mut data: Value, resolve_project: &mut F) -> Value
where
    F: FnMut(&str) -> Option<String>,
{
    let session = data
        .get("nativeSession")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let project = resolve_project(&session)
        .filter(|project| !project.is_empty())
        .unwrap_or_else(global_project_id);
    if let Some(fields) = data.as_object_mut() {
        fields.insert("projectId".to_owned(), Value::String(project));
    }
    data
}
