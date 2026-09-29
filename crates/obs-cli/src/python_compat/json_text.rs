//! Python `json.dumps` text for `serde_json::Value` (keys keep insertion order via the
//! `preserve_order` feature). Three layouts are used by obs.py:
//! - `Compact`  = `separators=(",", ":")`   (spool payloads, POST bodies)
//! - `Spaced`   = default separators         (`json.dumps(list)`, `{"flushed": n}`)
//! - `Indented` = `indent=2`                 (command results)
//!
//! `ensure_ascii=True` escapes everything outside printable ASCII as `\uXXXX` (lowercase hex,
//! surrogate pairs above U+FFFF). Floats use Python `repr` (`1e-05`, `1e+16`, `2.0`).

use std::fmt::Write as _;

use serde_json::{Number, Value};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum JsonLayout {
    Compact,
    Spaced,
    Indented,
}

const INDENT_WIDTH: usize = 2;

pub fn to_python_json(value: &Value, layout: JsonLayout, ensure_ascii: bool) -> String {
    let mut rendered = String::new();
    write_value(&mut rendered, value, layout, ensure_ascii, 0);
    rendered
}

fn write_value(
    out: &mut String,
    value: &Value,
    layout: JsonLayout,
    ensure_ascii: bool,
    depth: usize,
) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Value::Number(number) => write_number(out, number),
        Value::String(text) => write_json_string(out, text, ensure_ascii),
        Value::Array(items) => write_array(out, items, layout, ensure_ascii, depth),
        Value::Object(entries) => {
            let pairs: Vec<(&String, &Value)> = entries.iter().collect();
            write_object(out, &pairs, layout, ensure_ascii, depth);
        }
    }
}

fn write_array(
    out: &mut String,
    items: &[Value],
    layout: JsonLayout,
    ensure_ascii: bool,
    depth: usize,
) {
    if items.is_empty() {
        out.push_str("[]");
        return;
    }

    out.push('[');
    for (index, item) in items.iter().enumerate() {
        write_item_separator(out, layout, depth + 1, index == 0);
        write_value(out, item, layout, ensure_ascii, depth + 1);
    }
    write_closing_break(out, layout, depth);
    out.push(']');
}

fn write_object(
    out: &mut String,
    pairs: &[(&String, &Value)],
    layout: JsonLayout,
    ensure_ascii: bool,
    depth: usize,
) {
    if pairs.is_empty() {
        out.push_str("{}");
        return;
    }

    out.push('{');
    for (index, (key, item)) in pairs.iter().enumerate() {
        write_item_separator(out, layout, depth + 1, index == 0);
        write_json_string(out, key, ensure_ascii);
        out.push_str(key_separator(layout));
        write_value(out, item, layout, ensure_ascii, depth + 1);
    }
    write_closing_break(out, layout, depth);
    out.push('}');
}

fn write_item_separator(out: &mut String, layout: JsonLayout, depth: usize, is_first: bool) {
    match layout {
        JsonLayout::Compact if !is_first => out.push(','),
        JsonLayout::Spaced if !is_first => out.push_str(", "),
        JsonLayout::Indented => {
            if !is_first {
                out.push(',');
            }
            push_newline_indent(out, depth);
        }
        _ => {}
    }
}

#[inline]
fn write_closing_break(out: &mut String, layout: JsonLayout, depth: usize) {
    if layout == JsonLayout::Indented {
        push_newline_indent(out, depth);
    }
}

#[inline]
fn key_separator(layout: JsonLayout) -> &'static str {
    match layout {
        JsonLayout::Compact => ":",
        JsonLayout::Spaced | JsonLayout::Indented => ": ",
    }
}

#[inline]
fn push_newline_indent(out: &mut String, depth: usize) {
    out.push('\n');
    out.push_str(&" ".repeat(depth * INDENT_WIDTH));
}

fn write_number(out: &mut String, number: &Number) {
    if number.is_i64() || number.is_u64() {
        out.push_str(&number.to_string());
        return;
    }
    match number.as_f64() {
        Some(float) => out.push_str(&python_float_repr(float)),
        None => out.push_str(&number.to_string()),
    }
}

/// Python json string escaping; with `ensure_ascii` everything outside `' '..='~'` is escaped.
pub fn write_json_string(out: &mut String, text: &str, ensure_ascii: bool) {
    out.push('"');
    for character in text.chars() {
        write_string_character(out, character, ensure_ascii);
    }
    out.push('"');
}

fn write_string_character(out: &mut String, character: char, ensure_ascii: bool) {
    match character {
        '"' => out.push_str("\\\""),
        '\\' => out.push_str("\\\\"),
        '\n' => out.push_str("\\n"),
        '\r' => out.push_str("\\r"),
        '\t' => out.push_str("\\t"),
        '\u{08}' => out.push_str("\\b"),
        '\u{0c}' => out.push_str("\\f"),
        control if (control as u32) < 0x20 => push_unicode_escape(out, control as u32),
        other if ensure_ascii && !is_printable_ascii(other) => push_utf16_escapes(out, other),
        other => out.push(other),
    }
}

#[inline]
fn is_printable_ascii(character: char) -> bool {
    (' '..='~').contains(&character)
}

fn push_utf16_escapes(out: &mut String, character: char) {
    let mut units = [0u16; 2];
    for unit in character.encode_utf16(&mut units) {
        push_unicode_escape(out, u32::from(*unit));
    }
}

#[inline]
fn push_unicode_escape(out: &mut String, code_unit: u32) {
    let _ = write!(out, "\\u{code_unit:04x}");
}

/// Python `repr(float)`: shortest round-trip digits; scientific when exponent < -4 or >= 16.
pub fn python_float_repr(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.to_owned();
    }

    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent_text) = scientific
        .split_once('e')
        .unwrap_or((scientific.as_str(), "0"));
    let exponent: i32 = exponent_text.parse().unwrap_or(0);
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let sign = if value.is_sign_negative() { "-" } else { "" };

    let body = if (-4..16).contains(&exponent) {
        positional_float_text(&digits, exponent)
    } else {
        scientific_float_text(&digits, exponent)
    };
    format!("{sign}{body}")
}

fn positional_float_text(digits: &str, exponent: i32) -> String {
    let point = exponent + 1;

    if point <= 0 {
        let zeros = "0".repeat(point.unsigned_abs() as usize);
        return format!("0.{zeros}{digits}");
    }

    let point = point as usize;
    if point >= digits.len() {
        let zeros = "0".repeat(point - digits.len());
        return format!("{digits}{zeros}.0");
    }
    format!("{}.{}", &digits[..point], &digits[point..])
}

fn scientific_float_text(digits: &str, exponent: i32) -> String {
    let (lead, rest) = digits.split_at(1);
    let fraction = if rest.is_empty() {
        String::new()
    } else {
        format!(".{rest}")
    };
    let exponent_sign = if exponent < 0 { '-' } else { '+' };
    format!(
        "{lead}{fraction}e{exponent_sign}{:02}",
        exponent.unsigned_abs()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn indented_layout_matches_python_indent_2() {
        let value = json!({"a": [1, 2], "b": {}, "c": []});
        assert_eq!(
            to_python_json(&value, JsonLayout::Indented, true),
            "{\n  \"a\": [\n    1,\n    2\n  ],\n  \"b\": {},\n  \"c\": []\n}"
        );
    }

    #[test]
    fn compact_and_spaced_separators() {
        let value = json!({"x": [1, "y"]});
        assert_eq!(
            to_python_json(&value, JsonLayout::Compact, true),
            "{\"x\":[1,\"y\"]}"
        );
        assert_eq!(
            to_python_json(&value, JsonLayout::Spaced, true),
            "{\"x\": [1, \"y\"]}"
        );
    }

    #[test]
    fn ensure_ascii_escapes_like_python() {
        let value = json!("é😀\u{7f}\n");
        assert_eq!(
            to_python_json(&value, JsonLayout::Compact, true),
            "\"\\u00e9\\ud83d\\ude00\\u007f\\n\""
        );
        assert_eq!(
            to_python_json(&value, JsonLayout::Compact, false),
            "\"é😀\u{7f}\\n\""
        );
    }

    #[test]
    fn float_repr_matches_python() {
        assert_eq!(python_float_repr(2.0), "2.0");
        assert_eq!(python_float_repr(0.1), "0.1");
        assert_eq!(python_float_repr(1e-5), "1e-05");
        assert_eq!(python_float_repr(0.0001), "0.0001");
        assert_eq!(python_float_repr(1e16), "1e+16");
        assert_eq!(python_float_repr(1.5e16), "1.5e+16");
        assert_eq!(python_float_repr(123456789012345.6), "123456789012345.6");
        assert_eq!(python_float_repr(-0.0), "-0.0");
        assert_eq!(python_float_repr(-12.5), "-12.5");
    }

    #[test]
    fn preserves_key_insertion_order() {
        let value: Value = serde_json::from_str("{\"z\":1,\"a\":2}").unwrap();
        assert_eq!(
            to_python_json(&value, JsonLayout::Compact, true),
            "{\"z\":1,\"a\":2}"
        );
    }
}
