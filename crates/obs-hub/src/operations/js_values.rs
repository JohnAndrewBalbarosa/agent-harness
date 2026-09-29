//! JavaScript value semantics that server.mjs relied on (`||`, `??`, `String()`, `Boolean()`,
//! `Number()`, `.slice(0, max)`), so the Rust hub coerces client data exactly the same way.
//!
//! Lone UTF-16 surrogates: JavaScript strings may hold them (`"\ud800"` in JSON), Rust strings
//! cannot. The body parser maps each lone surrogate to a sentinel in Supplementary Private Use
//! Area-B (`LONE_SURROGATE_BASE + (unit - 0xD800)`). Sentinels count as one UTF-16 unit, become
//! U+FFFD when written as Postgres text (what node-pg's UTF-8 encoding produced), and become the
//! literal text `\uXXXX` inside JSON documents (server.mjs `escapeLoneSurrogates`).

use serde_json::Value;

pub const LONE_SURROGATE_BASE: u32 = 0x10_F800;
const SURROGATE_FIRST: u32 = 0xD800;
const SURROGATE_COUNT: u32 = 0x800;
const REPLACEMENT: char = '\u{FFFD}';

/// `Boolean(value)`.
pub fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => is_nonzero_number(number),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// `value ?? fallback` treats only `null`/`undefined` as absent; a missing key is `Value::Null` here.
#[inline]
pub fn is_nullish(value: &Value) -> bool {
    value.is_null()
}

/// `String(value ?? "")`, still holding lone-surrogate sentinels.
pub fn js_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => js_number_text(number),
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(array_item_string)
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_owned(),
    }
}

/// `text(value, max)` as stored by Postgres: JS `.slice(0, max)` then node-pg UTF-8 encoding.
pub fn js_text(value: &Value, max_utf16_units: usize) -> String {
    postgres_text(&truncate_utf16(&js_string(value), max_utf16_units))
}

/// `Number(value)` for the coercions server.mjs performed (`Math.max(0, x)`, `Number(baseVersion)`).
pub fn js_number(value: &Value) -> f64 {
    match value {
        Value::Null => 0.0,
        Value::Bool(flag) => f64::from(u8::from(*flag)),
        Value::Number(number) => number.as_f64().unwrap_or(f64::NAN),
        Value::String(text) => string_to_number(text),
        Value::Array(_) | Value::Object(_) => f64::NAN,
    }
}

/// JS `.slice(0, max)` over UTF-16 units. Cutting through a surrogate pair leaves a lone high
/// surrogate in JavaScript, kept here as its sentinel (U+FFFD once written to Postgres).
pub fn truncate_utf16(text: &str, max_utf16_units: usize) -> String {
    let mut kept = String::new();
    let mut used_units = 0;

    for character in text.chars() {
        let units = js_utf16_units(character);
        if used_units + units > max_utf16_units {
            if used_units < max_utf16_units {
                kept.push(lone_surrogate_sentinel(high_surrogate_of(character)));
            }
            break;
        }
        kept.push(character);
        used_units += units;
    }
    kept
}

/// `string.length` in JavaScript.
pub fn js_utf16_length(text: &str) -> usize {
    text.chars().map(js_utf16_units).sum()
}

/// What Postgres stored for a JS string parameter: lone surrogates became U+FFFD.
pub fn postgres_text(text: &str) -> String {
    text.chars()
        .map(|character| {
            if is_lone_surrogate_sentinel(character) {
                REPLACEMENT
            } else {
                character
            }
        })
        .collect()
}

/// server.mjs `escapeLoneSurrogates`: each lone surrogate becomes the six characters `\uXXXX`.
pub fn escape_lone_surrogates(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match lone_surrogate_unit(character) {
            Some(unit) => escaped.push_str(&format!("\\u{unit:04x}")),
            None => escaped.push(character),
        }
    }
    escaped
}

/// How node-pg stringified a JS value before Postgres parsed it (`String(n)`, `'true'`, JSON).
pub fn postgres_parameter_text(value: &Value) -> String {
    match value {
        Value::Array(_) | Value::Object(_) => postgres_text(&value.to_string()),
        other => postgres_text(&js_string(other)),
    }
}

/// Sentinel for the lone surrogate code unit `unit` (0xD800..=0xDFFF).
#[inline]
pub fn lone_surrogate_sentinel(unit: u32) -> char {
    char::from_u32(LONE_SURROGATE_BASE + (unit - SURROGATE_FIRST)).unwrap_or(REPLACEMENT)
}

/// First UTF-16 unit of a supplementary-plane character.
#[inline]
fn high_surrogate_of(character: char) -> u32 {
    SURROGATE_FIRST + ((u32::from(character) - 0x1_0000) >> 10)
}

#[inline]
fn lone_surrogate_unit(character: char) -> Option<u32> {
    is_lone_surrogate_sentinel(character)
        .then(|| u32::from(character) - LONE_SURROGATE_BASE + SURROGATE_FIRST)
}

#[inline]
fn is_lone_surrogate_sentinel(character: char) -> bool {
    (LONE_SURROGATE_BASE..LONE_SURROGATE_BASE + SURROGATE_COUNT).contains(&u32::from(character))
}

#[inline]
fn js_utf16_units(character: char) -> usize {
    if is_lone_surrogate_sentinel(character) {
        1
    } else {
        character.len_utf16()
    }
}

#[inline]
fn is_nonzero_number(number: &serde_json::Number) -> bool {
    number
        .as_f64()
        .is_some_and(|float| float != 0.0 && !float.is_nan())
}

/// JavaScript prints integral doubles without a fraction (`5`, not `5.0`).
fn js_number_text(number: &serde_json::Number) -> String {
    match number.as_f64() {
        Some(float) if is_safe_integer(float) => format!("{}", float as i64),
        _ => number.to_string(),
    }
}

#[inline]
fn is_safe_integer(float: f64) -> bool {
    float.fract() == 0.0 && float.abs() < 9_007_199_254_740_992.0
}

/// `Array.prototype.join` renders `null` items as empty strings.
#[inline]
fn array_item_string(item: &Value) -> String {
    if item.is_null() {
        String::new()
    } else {
        js_string(item)
    }
}

fn string_to_number(text: &str) -> f64 {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return 0.0;
    }
    trimmed.parse::<f64>().unwrap_or(f64::NAN)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn truthiness_matches_javascript() {
        for falsy in [json!(null), json!(false), json!(0), json!(""), json!(0.0)] {
            assert!(!is_truthy(&falsy), "{falsy} should be falsy");
        }
        for truthy in [json!(true), json!(1), json!("0"), json!([]), json!({})] {
            assert!(is_truthy(&truthy), "{truthy} should be truthy");
        }
    }

    #[test]
    fn string_conversion_matches_javascript() {
        assert_eq!(js_string(&json!(null)), "");
        assert_eq!(js_string(&json!(5.0)), "5");
        assert_eq!(js_string(&json!(1.5)), "1.5");
        assert_eq!(js_string(&json!([1, null, "a"])), "1,,a");
        assert_eq!(js_string(&json!({"a": 1})), "[object Object]");
    }

    #[test]
    fn text_truncation_splits_surrogate_pairs_like_javascript() {
        assert_eq!(js_text(&json!("abcdef"), 3), "abc");
        assert_eq!(js_text(&json!("a😀b"), 2), "a\u{FFFD}");
        assert_eq!(js_text(&json!("a😀b"), 3), "a😀");
        assert_eq!(js_text(&json!("a😀b"), 1), "a");
    }

    #[test]
    fn lone_surrogates_become_replacement_or_escape_text() {
        let text = format!("x{}y", lone_surrogate_sentinel(0xD800));
        assert_eq!(js_utf16_length(&text), 3);
        assert_eq!(postgres_text(&text), "x\u{FFFD}y");
        assert_eq!(escape_lone_surrogates(&text), "x\\ud800y");
    }

    #[test]
    fn number_conversion_matches_javascript() {
        assert_eq!(js_number(&json!("  7 ")), 7.0);
        assert_eq!(js_number(&json!("")), 0.0);
        assert!(js_number(&json!("x")).is_nan());
        assert_eq!(js_number(&json!(true)), 1.0);
    }
}
