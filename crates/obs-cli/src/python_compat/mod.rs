//! Byte-level parity helpers: each function reproduces one Python/obs.py behaviour
//! (int()/float() parsing, str slicing, repr, json.dumps, isoformat, uuid, redaction)
//! so obs-rs output and spool rows are indistinguishable from obs.py's.

pub mod json_text;
pub mod redaction;
pub mod time_text;
pub mod uuid_text;

pub use json_text::{python_float_repr, to_python_json, write_json_string, JsonLayout};
pub use redaction::redact;
pub use time_text::{python_observed_time, python_utc_now};
pub use uuid_text::{python_uuid_text, uuid5_url_text, uuid7_text};

/// Python `int(text)` for the ASCII subset: surrounding whitespace, sign, `_` between digits.
pub fn parse_python_int(text: &str) -> Option<i64> {
    let trimmed = text.trim();
    let (negative, body) = split_sign(trimmed);

    if !is_python_digit_run(body) {
        return None;
    }

    let digits: String = body.chars().filter(|character| *character != '_').collect();
    let magnitude: i64 = digits.parse().ok()?;
    Some(if negative { -magnitude } else { magnitude })
}

/// Python `float(text)` for the forms env vars realistically carry.
pub fn parse_python_float(text: &str) -> Option<f64> {
    let cleaned: String = text
        .trim()
        .chars()
        .filter(|character| *character != '_')
        .collect();
    cleaned.parse::<f64>().ok()
}

/// Python text-mode stdout/stderr on Windows writes every `\n` as `\r\n`.
pub fn with_platform_newlines(text: &str) -> String {
    if cfg!(windows) {
        text.replace('\n', "\r\n")
    } else {
        text.to_owned()
    }
}

/// Python `value[:limit]` on a `str` (code points, not bytes).
pub fn truncate_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

/// Python `repr(str)`: single quotes unless the text holds `'` and no `"`.
pub fn python_repr_str(value: &str) -> String {
    let quote = if value.contains('\'') && !value.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut rendered = String::with_capacity(value.len() + 2);
    rendered.push(quote);

    for character in value.chars() {
        push_repr_character(&mut rendered, character, quote);
    }

    rendered.push(quote);
    rendered
}

fn push_repr_character(rendered: &mut String, character: char, quote: char) {
    match character {
        '\\' => rendered.push_str("\\\\"),
        '\n' => rendered.push_str("\\n"),
        '\r' => rendered.push_str("\\r"),
        '\t' => rendered.push_str("\\t"),
        other if other == quote => {
            rendered.push('\\');
            rendered.push(other);
        }
        other if (other as u32) < 0x20 || other as u32 == 0x7f => {
            rendered.push_str(&format!("\\x{:02x}", other as u32));
        }
        other => rendered.push(other),
    }
}

#[inline]
fn split_sign(text: &str) -> (bool, &str) {
    match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    }
}

/// Digits with single underscores strictly between digits (`1_000`, not `_1` or `1__0`).
fn is_python_digit_run(body: &str) -> bool {
    let bytes = body.as_bytes();
    let has_digit_ends = matches!((bytes.first(), bytes.last()), (Some(first), Some(last)) if first.is_ascii_digit() && last.is_ascii_digit());

    has_digit_ends
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_digit() || *byte == b'_')
        && !body.contains("__")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_python_int_forms() {
        assert_eq!(parse_python_int(" 42 "), Some(42));
        assert_eq!(parse_python_int("-1_000"), Some(-1000));
        assert_eq!(parse_python_int("+7"), Some(7));
        assert_eq!(parse_python_int("1.5"), None);
        assert_eq!(parse_python_int("_1"), None);
        assert_eq!(parse_python_int(""), None);
    }

    #[test]
    fn repr_matches_python_quoting() {
        assert_eq!(python_repr_str("x"), "'x'");
        assert_eq!(python_repr_str("it's"), "\"it's\"");
        assert_eq!(python_repr_str("a\\b\n"), "'a\\\\b\\n'");
    }

    #[test]
    fn truncates_by_code_point() {
        assert_eq!(truncate_chars("héllo", 2), "hé");
    }
}
