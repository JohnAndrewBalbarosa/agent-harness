//! Typed access to an operation's `data` object. Each accessor names the exact JavaScript
//! expression it reproduces from server.mjs and then applies the coercion Postgres performed
//! on the node-pg parameter, so invalid input fails the batch just as it did before.

use anyhow::{anyhow, bail, Result};
use serde_json::{Map, Value};

use super::js_values::{
    escape_lone_surrogates, is_nullish, is_truthy, js_number, js_string, js_text,
    postgres_parameter_text, postgres_text,
};

static ABSENT: Value = Value::Null;

#[derive(Clone, Copy)]
pub struct Fields<'a> {
    map: Option<&'a Map<String, Value>>,
}

impl<'a> Fields<'a> {
    pub fn new(map: &'a Map<String, Value>) -> Self {
        Self { map: Some(map) }
    }

    /// Nested client objects (`reference`, `metric`, `alias`, `usage`); non-objects read as empty.
    pub fn of(value: &'a Value) -> Self {
        Self {
            map: value.as_object(),
        }
    }

    pub fn raw(&self, key: &str) -> &'a Value {
        self.map.and_then(|map| map.get(key)).unwrap_or(&ABSENT)
    }

    /// `jsonArray(d.key)` items.
    pub fn array(&self, key: &str) -> &'a [Value] {
        self.raw(key).as_array().map(Vec::as_slice).unwrap_or(&[])
    }

    // ---- truthiness and booleans ----

    /// `Boolean(d.key)` / `if (d.key)`.
    pub fn is_truthy(&self, key: &str) -> bool {
        is_truthy(self.raw(key))
    }

    /// `d.key !== false`.
    pub fn is_not_false(&self, key: &str) -> bool {
        !matches!(self.raw(key), Value::Bool(false))
    }

    /// `d.key ?? null` into a Postgres boolean.
    pub fn nullable_boolean(&self, key: &str) -> Result<Option<bool>> {
        match self.raw(key) {
            value if is_nullish(value) => Ok(None),
            value => postgres_boolean(value).map(Some),
        }
    }

    // ---- text ----

    /// `text(d.key, max)`.
    pub fn text(&self, key: &str, max: usize) -> String {
        js_text(self.raw(key), max)
    }

    /// `text(d.key, max) || null`.
    pub fn text_or_null(&self, key: &str, max: usize) -> Option<String> {
        Some(self.text(key, max)).filter(|text| !text.is_empty())
    }

    /// `text(d.key || fallback, max)`.
    pub fn text_or(&self, key: &str, fallback: &str, max: usize) -> String {
        if self.is_truthy(key) {
            self.text(key, max)
        } else {
            js_text(&Value::String(fallback.to_owned()), max)
        }
    }

    /// `d.key ? text(d.key, max) : null`.
    pub fn text_if_truthy(&self, key: &str, max: usize) -> Option<String> {
        self.is_truthy(key).then(|| self.text(key, max))
    }

    /// `String(d.key ?? "")`, untruncated.
    pub fn full_string(&self, key: &str) -> String {
        postgres_text(&js_string(self.raw(key)))
    }

    /// `d.key || null` passed straight to a varchar column (no JS truncation).
    pub fn raw_text_if_truthy(&self, key: &str) -> Option<String> {
        self.is_truthy(key)
            .then(|| postgres_parameter_text(self.raw(key)))
    }

    /// `d.key` passed straight to a text column; `undefined`/`null` become SQL NULL.
    pub fn raw_text(&self, key: &str) -> Option<String> {
        let value = self.raw(key);
        (!is_nullish(value)).then(|| postgres_parameter_text(value))
    }

    // ---- identifiers ----

    /// `d.key` into a uuid column.
    pub fn uuid(&self, key: &str) -> Result<Option<String>> {
        match self.raw(key) {
            value if is_nullish(value) => Ok(None),
            value => postgres_uuid(value).map(Some),
        }
    }

    /// `d.key || null` into a uuid column.
    pub fn uuid_if_truthy(&self, key: &str) -> Result<Option<String>> {
        if self.is_truthy(key) {
            postgres_uuid(self.raw(key)).map(Some)
        } else {
            Ok(None)
        }
    }

    /// `bytes(d.key, length)`.
    pub fn hex_bytes(&self, key: &str, length: usize) -> Result<Vec<u8>> {
        let text = self
            .raw(key)
            .as_str()
            .ok_or_else(|| anyhow!("expected {length}-byte hex value"))?;
        Ok(obs_common::decode_hex_bytes(text, length)?)
    }

    /// `d.key ? bytes(d.key, length) : null`.
    pub fn hex_bytes_if_truthy(&self, key: &str, length: usize) -> Result<Option<Vec<u8>>> {
        if self.is_truthy(key) {
            self.hex_bytes(key, length).map(Some)
        } else {
            Ok(None)
        }
    }

    /// `d.key || null` into a `char(64)` column guarded by `~ '^[a-f0-9]{64}$'`.
    pub fn sha256_hex_if_truthy(&self, key: &str) -> Result<Option<String>> {
        match self.raw_text_if_truthy(key) {
            Some(text) if is_lowercase_sha256(&text) => Ok(Some(text)),
            Some(_) => bail!("value for {key} violates the sha256 hex check constraint"),
            None => Ok(None),
        }
    }

    /// `d.key` (required) into a `char(64)` column guarded by the sha256 hex check.
    pub fn sha256_hex(&self, key: &str) -> Result<String> {
        self.sha256_hex_if_truthy(key)?
            .ok_or_else(|| anyhow!("null value in column for {key} violates not-null constraint"))
    }

    // ---- numbers ----

    /// `d.key` into an integer column; `undefined`/`null` become SQL NULL.
    pub fn integer(&self, key: &str) -> Result<Option<i64>> {
        match self.raw(key) {
            value if is_nullish(value) => Ok(None),
            value => postgres_integer(value).map(Some),
        }
    }

    /// `d.key || fallback` into an integer column.
    pub fn integer_or(&self, key: &str, fallback: i64) -> Result<i64> {
        if self.is_truthy(key) {
            postgres_integer(self.raw(key))
        } else {
            Ok(fallback)
        }
    }

    /// `d.key || null` into an integer column.
    pub fn integer_if_truthy(&self, key: &str) -> Result<Option<i64>> {
        if self.is_truthy(key) {
            postgres_integer(self.raw(key)).map(Some)
        } else {
            Ok(None)
        }
    }

    /// `Math.min(max, Math.max(0, d.key || 0))` into an integer column.
    pub fn clamped_integer(&self, key: &str, max: i64) -> Result<i64> {
        let number = if self.is_truthy(key) {
            js_number(self.raw(key))
        } else {
            0.0
        };
        let clamped = js_min(max as f64, js_max(0.0, number));
        float_to_postgres_integer(clamped)
    }

    /// `d.key ?? null` into a double column.
    pub fn real(&self, key: &str) -> Result<Option<f64>> {
        match self.raw(key) {
            value if is_nullish(value) => Ok(None),
            value => postgres_real(value).map(Some),
        }
    }

    /// `d.key ?? fallback` into a double column.
    pub fn real_or(&self, key: &str, fallback: f64) -> Result<f64> {
        Ok(self.real(key)?.unwrap_or(fallback))
    }

    // ---- timestamps ----

    /// `d.key` into a timestamptz column; `undefined`/`null` become SQL NULL.
    pub fn timestamp(&self, key: &str) -> Result<Option<String>> {
        match self.raw(key) {
            value if is_nullish(value) => Ok(None),
            value => postgres_timestamp(value).map(Some),
        }
    }

    /// `d.key || fallback` into a timestamptz column.
    pub fn timestamp_or(&self, key: &str, fallback: &str) -> Result<String> {
        if self.is_truthy(key) {
            postgres_timestamp(self.raw(key))
        } else {
            Ok(fallback.to_owned())
        }
    }

    /// `d.key || null` into a timestamptz column.
    pub fn timestamp_if_truthy(&self, key: &str) -> Result<Option<String>> {
        if self.is_truthy(key) {
            postgres_timestamp(self.raw(key)).map(Some)
        } else {
            Ok(None)
        }
    }

    // ---- JSON documents ----

    /// `jsonText(jsonArray(d.key))`.
    pub fn json_array_text(&self, key: &str) -> String {
        match self.raw(key) {
            value @ Value::Array(_) => js_json_text(value),
            _ => "[]".to_owned(),
        }
    }

    /// `jsonText(jsonObject(d.key))`.
    pub fn json_object_text(&self, key: &str) -> String {
        match self.raw(key) {
            value @ Value::Object(_) => js_json_text(value),
            _ => "{}".to_owned(),
        }
    }

    /// `jsonText(d.key ?? {})`.
    pub fn json_text_or_empty_object(&self, key: &str) -> String {
        match self.raw(key) {
            Value::Null => "{}".to_owned(),
            value => js_json_text(value),
        }
    }
}

/// `JSON.stringify` output: integral doubles lose their fraction (`5.0` → `5`).
pub fn js_json_text(value: &Value) -> String {
    with_js_numbers(value).to_string()
}

/// `jsonSafe` + `JSON.stringify` value rendering: lone surrogates escaped, JS number text.
fn with_js_numbers(value: &Value) -> Value {
    match value {
        Value::Number(number) => js_number_value(number),
        Value::String(text) => Value::String(escape_lone_surrogates(text)),
        Value::Array(items) => Value::Array(items.iter().map(with_js_numbers).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| (escape_lone_surrogates(key), with_js_numbers(item)))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[inline]
fn js_number_value(number: &serde_json::Number) -> Value {
    match number.as_f64() {
        Some(float) if number.is_f64() && is_integral_safe(float) => Value::from(float as i64),
        _ => Value::Number(number.clone()),
    }
}

#[inline]
fn is_integral_safe(float: f64) -> bool {
    float.fract() == 0.0 && float.abs() < 9_007_199_254_740_992.0
}

// ---- Postgres input coercions of node-pg parameters ----

pub fn postgres_integer(value: &Value) -> Result<i64> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().and_then(exact_integer))
            .ok_or_else(|| invalid_input("integer", &js_string(value))),
        Value::String(text) => text
            .trim()
            .parse::<i64>()
            .map_err(|_| invalid_input("integer", text)),
        other => Err(invalid_input("integer", &postgres_parameter_text(other))),
    }
}

pub fn postgres_real(value: &Value) -> Result<f64> {
    match value {
        Value::Number(number) => number
            .as_f64()
            .ok_or_else(|| invalid_input("double precision", &number.to_string())),
        Value::String(text) => parse_postgres_real(text),
        other => Err(invalid_input(
            "double precision",
            &postgres_parameter_text(other),
        )),
    }
}

pub fn postgres_boolean(value: &Value) -> Result<bool> {
    match value {
        Value::Bool(flag) => Ok(*flag),
        Value::String(text) => parse_postgres_boolean(text),
        Value::Number(_) => parse_postgres_boolean(&js_string(value)),
        other => Err(invalid_input("boolean", &postgres_parameter_text(other))),
    }
}

pub fn postgres_uuid(value: &Value) -> Result<String> {
    Ok(obs_common::normalize_uuid(&postgres_parameter_text(value))?)
}

pub fn postgres_timestamp(value: &Value) -> Result<String> {
    let text = postgres_parameter_text(value);
    obs_common::normalize_timestamp(&text)
        .map_err(|_| anyhow!("invalid input syntax for type timestamp with time zone: \"{text}\""))
}

fn parse_postgres_real(text: &str) -> Result<f64> {
    let trimmed = text.trim();
    match trimmed.to_ascii_lowercase().as_str() {
        "nan" => Ok(f64::NAN),
        "infinity" | "+infinity" | "inf" => Ok(f64::INFINITY),
        "-infinity" | "-inf" => Ok(f64::NEG_INFINITY),
        _ => trimmed
            .parse::<f64>()
            .map_err(|_| invalid_input("double precision", text)),
    }
}

fn parse_postgres_boolean(text: &str) -> Result<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "t" | "true" | "y" | "yes" | "on" | "1" => Ok(true),
        "f" | "false" | "n" | "no" | "off" | "0" => Ok(false),
        _ => Err(invalid_input("boolean", text)),
    }
}

fn float_to_postgres_integer(float: f64) -> Result<i64> {
    exact_integer(float).ok_or_else(|| invalid_input("integer", &float.to_string()))
}

#[inline]
fn exact_integer(float: f64) -> Option<i64> {
    (float.is_finite() && float.fract() == 0.0 && float.abs() < 9.2e18).then_some(float as i64)
}

/// `Math.max` / `Math.min` propagate NaN.
#[inline]
fn js_max(left: f64, right: f64) -> f64 {
    if left.is_nan() || right.is_nan() {
        f64::NAN
    } else {
        left.max(right)
    }
}

#[inline]
fn js_min(left: f64, right: f64) -> f64 {
    if left.is_nan() || right.is_nan() {
        f64::NAN
    } else {
        left.min(right)
    }
}

#[inline]
fn is_lowercase_sha256(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

#[inline]
fn invalid_input(type_name: &str, text: &str) -> anyhow::Error {
    anyhow!("invalid input syntax for type {type_name}: \"{text}\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fields(value: &Value) -> Fields<'_> {
        Fields::of(value)
    }

    #[test]
    fn or_fallback_uses_javascript_falsiness() {
        let data = json!({"zero": 0, "three": 3, "text": "4"});
        let data = fields(&data);
        assert_eq!(data.integer_or("zero", 1).unwrap(), 1);
        assert_eq!(data.integer_or("missing", 1).unwrap(), 1);
        assert_eq!(data.integer_or("three", 1).unwrap(), 3);
        assert_eq!(data.integer_or("text", 1).unwrap(), 4);
    }

    #[test]
    fn integers_reject_fractions_like_postgres() {
        assert!(postgres_integer(&json!(1.5)).is_err());
        assert_eq!(postgres_integer(&json!(2.0)).unwrap(), 2);
        assert!(postgres_integer(&json!(true)).is_err());
    }

    #[test]
    fn clamped_integer_matches_math_min_max() {
        let data = json!({"big": 500, "negative": -4, "text": "12", "bad": "x"});
        let data = fields(&data);
        assert_eq!(data.clamped_integer("big", 80).unwrap(), 80);
        assert_eq!(data.clamped_integer("negative", 80).unwrap(), 0);
        assert_eq!(data.clamped_integer("text", 80).unwrap(), 12);
        assert_eq!(data.clamped_integer("missing", 80).unwrap(), 0);
        assert!(data.clamped_integer("bad", 80).is_err());
    }

    #[test]
    fn text_helpers_follow_server_mjs() {
        let data = json!({"empty": "", "long": "abcdef", "number": 7});
        let data = fields(&data);
        assert_eq!(data.text_or_null("empty", 10), None);
        assert_eq!(data.text("long", 3), "abc");
        assert_eq!(data.text_or("empty", "tl-en", 16), "tl-en");
        assert_eq!(data.text("number", 10), "7");
    }

    #[test]
    fn json_text_uses_javascript_number_rendering() {
        assert_eq!(
            js_json_text(&json!({"a": 5.0, "b": [1.5, 2.0]})),
            r#"{"a":5,"b":[1.5,2]}"#
        );
    }

    #[test]
    fn booleans_follow_postgres_input_rules() {
        assert!(postgres_boolean(&json!("yes")).unwrap());
        assert!(!postgres_boolean(&json!(0)).unwrap());
        assert!(postgres_boolean(&json!("maybe")).is_err());
    }
}
