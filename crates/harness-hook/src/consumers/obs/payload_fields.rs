//! Python expression shapes hook.py uses on the native payload: `value.get(a) or value.get(b) or ...`,
//! `str(...)`, and `[:limit]` slicing.

use obs_cli::python_compat::truncate_chars;
use serde_json::{Map, Value};

use crate::events::{is_python_truthy, python_str};

pub type Payload = Map<String, Value>;

/// The first truthy `payload[key]` in order (Python `a or b or ...` when some operand is truthy).
pub fn first_truthy<'a>(payload: &'a Payload, keys: &[&str]) -> Option<&'a Value> {
    keys.iter()
        .filter_map(|key| payload.get(*key))
        .find(|value| is_python_truthy(value))
}

/// `value.get(a) or value.get(b)`: the first truthy value, else the LAST lookup (possibly falsy or None).
pub fn python_or(payload: &Payload, keys: &[&str]) -> Value {
    first_truthy(payload, keys)
        .or_else(|| keys.last().and_then(|key| payload.get(*key)))
        .cloned()
        .unwrap_or(Value::Null)
}

/// `str(value.get(a) or ... or fallback)[:limit]`.
pub fn text_or(payload: &Payload, keys: &[&str], fallback: &str, limit: usize) -> String {
    let text = first_truthy(payload, keys).map_or_else(|| fallback.to_owned(), python_str);
    truncate_chars(&text, limit)
}

/// `Optional[str]` → JSON (`None` → null).
#[inline]
pub fn optional_json(value: Option<&str>) -> Value {
    value.map_or(Value::Null, |text| Value::String(text.to_owned()))
}

/// Python truthiness of an optional text (`None` and `""` are falsy).
#[inline]
pub fn is_present(value: Option<&str>) -> bool {
    value.is_some_and(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn payload(value: Value) -> Payload {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn python_or_returns_last_operand_when_all_are_falsy() {
        let value = payload(json!({"tool_input": "", "toolInput": {}}));
        assert_eq!(python_or(&value, &["tool_input", "toolInput"]), json!({}));
        assert_eq!(python_or(&payload(json!({})), &["a", "b"]), Value::Null);
        assert_eq!(
            python_or(&payload(json!({"a": "x", "b": "y"})), &["a", "b"]),
            json!("x")
        );
    }

    #[test]
    fn text_or_truncates_by_code_point_and_falls_back() {
        let value = payload(json!({"turn_id": "", "turnId": "héllo"}));
        assert_eq!(text_or(&value, &["turn_id", "turnId"], "", 2), "hé");
        assert_eq!(text_or(&value, &["missing"], "unknown", 240), "unknown");
    }
}
