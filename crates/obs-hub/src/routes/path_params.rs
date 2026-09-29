//! Path/query parameter handling with Postgres/JavaScript parity.

use chrono::{Duration, Utc};

use crate::http_errors::HubError;

/// Postgres SQLSTATE `invalid_text_representation`.
const INVALID_TEXT_REPRESENTATION: &str = "22P02";

/// Fastify parses `?a=1&a=2` into an array and server.mjs `String()`-ed it: values join with `,`.
pub fn joined_query_value(pairs: &[(String, String)], key: &str) -> Option<String> {
    let values: Vec<&str> = pairs
        .iter()
        .filter(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
        .collect();
    (!values.is_empty()).then(|| values.join(","))
}

/// Postgres compared `uuid` parameters semantically and raised on malformed input (→ HTTP 500).
pub fn uuid_parameter(raw: &str) -> Result<String, HubError> {
    obs_common::normalize_uuid(raw)
        .map_err(|error| HubError::Internal(error.to_string(), Some(INVALID_TEXT_REPRESENTATION)))
}

/// `new Date(value).toISOString()` when `Date.parse` succeeds, else `None`.
pub fn optional_instant(raw: Option<&String>) -> Option<String> {
    raw.and_then(|text| obs_common::normalize_timestamp(text).ok())
}

/// Timestamp text for `now() - days`.
pub fn days_ago(days: i64) -> String {
    obs_common::format_utc_micros(&(Utc::now() - Duration::days(days)))
}

/// `Number.parseInt(text, 10)`: leading whitespace, optional sign, digits up to the first non-digit.
pub fn js_parse_int(raw: Option<&String>) -> Option<i64> {
    let text = raw?.trim_start();
    let (sign, digits) = match text.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, text.strip_prefix('+').unwrap_or(text)),
    };
    let leading_digits: String = digits.chars().take_while(char::is_ascii_digit).collect();
    leading_digits.parse::<i64>().ok().map(|value| sign * value)
}

/// `text(value, max) || fallback` for query strings.
pub fn query_text_or(raw: Option<&String>, max: usize, fallback: &str) -> String {
    let truncated =
        crate::operations::js_values::truncate_utf16(raw.map_or("", String::as_str), max);
    if truncated.is_empty() {
        fallback.to_owned()
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_int_follows_javascript() {
        let parse = |text: &str| js_parse_int(Some(&text.to_owned()));
        assert_eq!(parse("7"), Some(7));
        assert_eq!(parse(" 12abc"), Some(12));
        assert_eq!(parse("-3"), Some(-3));
        assert_eq!(parse("abc"), None);
        assert_eq!(js_parse_int(None), None);
    }
}
