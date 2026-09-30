//! Python value semantics the receiver's parity depends on: truthiness (`x or default`), `str(x)` /
//! f-string text, `adapters/base.py:73 count()`, and the implicit iteration/`.get` rules that make
//! receiver.py:42 `decode_attrs` raise on some malformed shapes.

use std::borrow::Cow;

use obs_cli::python_compat::{parse_python_int, python_float_repr, python_repr_str};
use serde_json::Value;

/// A payload shape on which Python's `decode_attrs` raises (the whole POST then enqueues nothing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeError {
    /// `.get` on a truthy non-dict (`AttributeError`).
    NotAMapping,
    /// `for x in <true or non-zero number>` (`TypeError`).
    NotIterable,
    /// A list or dict used as an attribute key (`TypeError: unhashable type`).
    UnhashableKey,
}

impl ShapeError {
    /// Python's exception type name, the only detail receiver.py writes to stderr.
    pub fn python_type_name(self) -> &'static str {
        match self {
            ShapeError::NotAMapping => "AttributeError",
            ShapeError::NotIterable | ShapeError::UnhashableKey => "TypeError",
        }
    }
}

/// Python `bool(value)` for JSON values.
pub fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|float| float != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(fields) => !fields.is_empty(),
    }
}

/// `a or b` where an absent key reads as `None`.
#[inline]
pub fn first_truthy<'a>(values: &[Option<&'a Value>]) -> Option<&'a Value> {
    values
        .iter()
        .flatten()
        .copied()
        .find(|value| is_truthy(value))
}

/// Python `str(value)` (also what an f-string `{value}` renders for these types).
pub fn python_str(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "None".to_owned(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => python_repr(other),
    }
}

/// Python `repr(value)` for JSON-decoded values (strings quoted, containers recursive).
pub fn python_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::Number(number) => python_number_text(number),
        Value::String(text) => python_repr_str(text),
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(python_repr).collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Object(fields) => {
            let parts: Vec<String> = fields
                .iter()
                .map(|(key, item)| format!("{}: {}", python_repr_str(key), python_repr(item)))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
    }
}

#[inline]
fn python_number_text(number: &serde_json::Number) -> String {
    if number.is_i64() || number.is_u64() {
        return number.to_string();
    }
    number.as_f64().map(python_float_repr).unwrap_or_default()
}

/// adapters/base.py:73 `count(value)`: `int(value)` for ints, bools, floats (truncated) and numeric
/// strings; anything else, or a negative result, is 0.
pub fn count(value: Option<&Value>) -> i64 {
    let number = match value {
        Some(Value::Bool(flag)) => Some(i64::from(*flag)),
        Some(Value::Number(number)) => number_as_python_int(number),
        Some(Value::String(text)) => parse_python_int(text),
        _ => None,
    };
    number.filter(|parsed| *parsed >= 0).unwrap_or(0)
}

/// `int(number)`: exact for integers (u64 above `i64::MAX` saturates), truncation toward zero for
/// floats; NaN/inf (unreachable from serde_json) read as not-a-count.
#[inline]
fn number_as_python_int(number: &serde_json::Number) -> Option<i64> {
    if let Some(integer) = number.as_i64() {
        return Some(integer);
    }
    if number.is_u64() {
        return Some(i64::MAX);
    }
    number
        .as_f64()
        .filter(|float| float.is_finite())
        .map(|float| float.trunc() as i64)
}

/// `(container or {}).get(key)`: a falsy container reads as `{}`, a truthy non-dict raises.
/// `Ok(None)` means the key is missing; a present JSON `null` is `Ok(Some(Value::Null))`.
pub fn get_or_empty<'a>(container: &'a Value, key: &str) -> Result<Option<&'a Value>, ShapeError> {
    if !is_truthy(container) {
        return Ok(None);
    }
    strict_get(container, key)
}

/// `container.get(key)` with no `or {}` guard: anything but a dict raises.
#[inline]
pub fn strict_get<'a>(container: &'a Value, key: &str) -> Result<Option<&'a Value>, ShapeError> {
    container
        .as_object()
        .map(|fields| fields.get(key))
        .ok_or(ShapeError::NotAMapping)
}

/// `for item in (value or [])`: lists yield items, dicts yield their keys, strings yield characters;
/// `True` and non-zero numbers raise.
pub fn iterate_or_empty(value: Option<&Value>) -> Result<Vec<Cow<'_, Value>>, ShapeError> {
    let Some(value) = value.filter(|value| is_truthy(value)) else {
        return Ok(Vec::new());
    };
    match value {
        Value::Array(items) => Ok(items.iter().map(Cow::Borrowed).collect()),
        Value::Object(fields) => Ok(fields
            .keys()
            .map(|key| Cow::Owned(Value::String(key.clone())))
            .collect()),
        Value::String(text) => Ok(text
            .chars()
            .map(|character| Cow::Owned(Value::String(character.to_string())))
            .collect()),
        _ => Err(ShapeError::NotIterable),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn count_follows_python_int() {
        assert_eq!(count(Some(&json!("12581"))), 12581);
        assert_eq!(count(Some(&json!(" 7 "))), 7);
        assert_eq!(count(Some(&json!(7))), 7);
        assert_eq!(count(Some(&json!(3.9))), 3);
        assert_eq!(count(Some(&json!(true))), 1);
        assert_eq!(count(Some(&json!(-4))), 0);
        assert_eq!(count(Some(&json!("-4"))), 0);
        assert_eq!(count(Some(&json!("1.5"))), 0);
        assert_eq!(count(Some(&json!("abc"))), 0);
        assert_eq!(count(Some(&json!(null))), 0);
        assert_eq!(count(Some(&json!([1]))), 0);
        assert_eq!(count(None), 0);
    }

    #[test]
    fn str_matches_python_text() {
        assert_eq!(python_str(None), "None");
        assert_eq!(python_str(Some(&json!("x"))), "x");
        assert_eq!(python_str(Some(&json!(12))), "12");
        assert_eq!(python_str(Some(&json!(true))), "True");
        assert_eq!(python_str(Some(&json!(["a", 1]))), "['a', 1]");
        assert_eq!(python_str(Some(&json!({"k": null}))), "{'k': None}");
    }

    #[test]
    fn iteration_matches_python_rules() {
        assert!(iterate_or_empty(Some(&json!(0))).unwrap().is_empty());
        assert_eq!(iterate_or_empty(Some(&json!("ab"))).unwrap().len(), 2);
        assert_eq!(
            iterate_or_empty(Some(&json!({"k": 1}))).unwrap()[0].as_ref(),
            &json!("k")
        );
        assert_eq!(
            iterate_or_empty(Some(&json!(3))),
            Err(ShapeError::NotIterable)
        );
        assert_eq!(get_or_empty(&json!("x"), "a"), Err(ShapeError::NotAMapping));
        assert_eq!(get_or_empty(&json!(""), "a"), Ok(None));
    }
}
