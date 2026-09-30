//! receiver.py:42 `decode_attrs`: an OTLP/JSON logs payload → one `(service.name, attributes)` pair per
//! log record (`resourceLogs[].scopeLogs[].logRecords[]`). Shapes on which Python raises return a
//! `ShapeError` so the whole POST enqueues nothing, exactly as the Python `except` did.

use std::collections::HashMap;

use serde_json::Value;

use super::python_semantics::{
    get_or_empty, is_truthy, iterate_or_empty, python_str, strict_get, ShapeError,
};

/// Flattened record attributes. Only string keys are kept: Python's non-string keys (numbers, `None`,
/// bools) can never match a lookup, and unhashable keys raise.
pub type OtelAttributes = HashMap<String, Value>;

/// One decoded log record.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedLogRecord {
    pub service_name: String,
    pub attributes: OtelAttributes,
}

const ANY_VALUE_KEYS: [&str; 4] = ["stringValue", "intValue", "doubleValue", "boolValue"];

/// Every log record of the payload, in payload order.
pub fn decode_log_records(payload: &Value) -> Result<Vec<DecodedLogRecord>, ShapeError> {
    // Mental model: only a top-level dict whose `resourceLogs` is a real list is walked; each resource
    // contributes its service name, then every scope's log records.
    let Some(resource_logs) = payload.get("resourceLogs").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut decoded = Vec::new();
    for resource_log in resource_logs {
        decode_resource_log(resource_log, &mut decoded)?;
    }
    Ok(decoded)
}

fn decode_resource_log(
    resource_log: &Value,
    decoded: &mut Vec<DecodedLogRecord>,
) -> Result<(), ShapeError> {
    let service_name = resource_service_name(resource_log)?;
    let scope_logs = get_or_empty(resource_log, "scopeLogs")?;
    for scope_log in iterate_or_empty(scope_logs)? {
        let log_records = get_or_empty(&scope_log, "logRecords")?;
        for log_record in iterate_or_empty(log_records)? {
            let attributes = flatten_attributes(strict_get(&log_record, "attributes")?)?;
            decoded.push(DecodedLogRecord {
                service_name: service_name.clone(),
                attributes,
            });
        }
    }
    Ok(())
}

/// `str(resource.get("service.name") or "")` where `resource` comes from
/// `(resource_log or {}).get("resource", {}).get("attributes") or []`.
fn resource_service_name(resource_log: &Value) -> Result<String, ShapeError> {
    let attributes = match get_or_empty(resource_log, "resource")? {
        None => None,
        Some(resource) => strict_get(resource, "attributes")?,
    };
    let resource = flatten_attributes(attributes)?;
    let service = resource
        .get("service.name")
        .filter(|value| is_truthy(value));
    Ok(service.map_or_else(String::new, |value| python_str(Some(value))))
}

/// `{a.get("key"): _any_value(a.get("value")) for a in attributes or [] if isinstance(a, dict)}`.
fn flatten_attributes(attributes: Option<&Value>) -> Result<OtelAttributes, ShapeError> {
    let mut flattened = OtelAttributes::new();
    for attribute in iterate_or_empty(attributes)? {
        let Some(fields) = attribute.as_object() else {
            continue;
        };
        let value = any_value(fields.get("value"));
        match fields.get("key") {
            Some(Value::String(key)) => {
                flattened.insert(key.clone(), value);
            }
            Some(Value::Array(_) | Value::Object(_)) => return Err(ShapeError::UnhashableKey),
            _ => {}
        }
    }
    Ok(flattened)
}

/// receiver.py:34 `_any_value`: the first present of stringValue|intValue|doubleValue|boolValue
/// (raw, even `null`); arrays, kvlists, bytes and non-dicts are `None`.
#[inline]
pub fn any_value(value: Option<&Value>) -> Value {
    let Some(fields) = value.and_then(Value::as_object) else {
        return Value::Null;
    };
    ANY_VALUE_KEYS
        .iter()
        .find_map(|key| fields.get(*key))
        .cloned()
        .unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn any_value_precedence_and_containers() {
        assert_eq!(
            any_value(Some(&json!({"intValue": "5", "stringValue": "s"}))),
            json!("s")
        );
        assert_eq!(
            any_value(Some(&json!({"arrayValue": {"values": []}}))),
            Value::Null
        );
        assert_eq!(any_value(Some(&json!("raw"))), Value::Null);
    }

    #[test]
    fn truthy_non_dict_resource_log_raises() {
        let payload = json!({"resourceLogs": ["nope"]});
        assert_eq!(decode_log_records(&payload), Err(ShapeError::NotAMapping));
    }

    #[test]
    fn null_resource_log_is_skipped() {
        let payload = json!({"resourceLogs": [null, {}]});
        assert_eq!(decode_log_records(&payload), Ok(Vec::new()));
    }

    #[test]
    fn log_record_without_dict_raises() {
        let payload = json!({"resourceLogs": [{"scopeLogs": [{"logRecords": [null]}]}]});
        assert_eq!(decode_log_records(&payload), Err(ShapeError::NotAMapping));
    }
}
