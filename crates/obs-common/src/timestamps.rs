//! One canonical stored timestamp text: UTC, microsecond precision (like Postgres `timestamptz`),
//! `Z` suffix (`2026-09-30T01:02:03.456789Z`). Fixed width, so lexicographic order equals
//! chronological order. HTTP output truncates to milliseconds (`to_wire_millis`), as node-pg did.

use chrono::{DateTime, NaiveDate, NaiveDateTime, SecondsFormat, Utc};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
#[error("invalid timestamp: {0}")]
pub struct TimestampError(pub String);

/// Parses the timestamp shapes Postgres accepted from clients and exports, then renders the
/// canonical UTC microsecond text (sub-microsecond digits truncated).
pub fn normalize_timestamp(raw: &str) -> Result<String, TimestampError> {
    let trimmed = raw.trim();

    parse_with_offset(trimmed)
        .or_else(|| parse_without_offset(trimmed))
        .or_else(|| parse_date_only(trimmed))
        .map(|instant| format_utc_micros(&instant))
        .ok_or_else(|| TimestampError(truncate_for_message(trimmed)))
}

pub fn now_utc_micros() -> String {
    format_utc_micros(&Utc::now())
}

pub fn format_utc_micros(instant: &DateTime<Utc>) -> String {
    instant.to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// Canonical stored text → what node-pg sent over HTTP (`Date#toISOString`, milliseconds).
pub fn to_wire_millis(stored: &str) -> String {
    match stored.as_bytes() {
        bytes if bytes.len() == 27 && bytes[19] == b'.' && bytes[26] == b'Z' => {
            format!("{}Z", &stored[..23])
        }
        _ => stored.to_owned(),
    }
}

fn parse_with_offset(text: &str) -> Option<DateTime<Utc>> {
    let postgres_style = with_colon_offset(&text.replacen(' ', "T", 1));

    DateTime::parse_from_rfc3339(text)
        .or_else(|_| DateTime::parse_from_rfc3339(&postgres_style))
        .ok()
        .map(|instant| instant.with_timezone(&Utc))
}

fn parse_without_offset(text: &str) -> Option<DateTime<Utc>> {
    const LOCAL_FORMATS: [&str; 4] = [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ];

    LOCAL_FORMATS
        .iter()
        .find_map(|format| NaiveDateTime::parse_from_str(text, format).ok())
        .map(|naive| naive.and_utc())
}

fn parse_date_only(text: &str) -> Option<DateTime<Utc>> {
    NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .ok()
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .map(|naive| naive.and_utc())
}

/// Postgres renders offsets as `+00` or `+0530`; RFC 3339 needs `+00:00`.
fn with_colon_offset(text: &str) -> String {
    let Some(sign_index) = offset_sign_index(text) else {
        return text.to_owned();
    };
    let (instant, offset) = text.split_at(sign_index);
    let digits: String = offset[1..].chars().filter(char::is_ascii_digit).collect();

    match digits.len() {
        2 => format!("{instant}{}{digits}:00", &offset[..1]),
        4 => format!("{instant}{}{}:{}", &offset[..1], &digits[..2], &digits[2..]),
        _ => text.to_owned(),
    }
}

/// The offset sign is the last `+`/`-` after the time part (`T` separator).
#[inline]
fn offset_sign_index(text: &str) -> Option<usize> {
    let time_start = text.find('T')?;
    text[time_start..]
        .rfind(['+', '-'])
        .map(|relative| time_start + relative)
}

#[inline]
fn truncate_for_message(text: &str) -> String {
    text.chars().take(64).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pads_millis_to_micros() {
        assert_eq!(
            normalize_timestamp("2026-09-30T01:02:03.456Z").unwrap(),
            "2026-09-30T01:02:03.456000Z"
        );
    }

    #[test]
    fn wire_text_truncates_to_millis() {
        assert_eq!(
            to_wire_millis("2026-09-30T01:02:03.456789Z"),
            "2026-09-30T01:02:03.456Z"
        );
        assert_eq!(to_wire_millis("2026-09-30"), "2026-09-30");
    }

    #[test]
    fn keeps_postgres_microseconds_and_short_offset() {
        assert_eq!(
            normalize_timestamp("2026-09-30 09:02:03.4567891+08").unwrap(),
            "2026-09-30T01:02:03.456789Z"
        );
    }

    #[test]
    fn converts_row_to_json_offsets() {
        assert_eq!(
            normalize_timestamp("2026-09-30T01:02:03.4+00:00").unwrap(),
            "2026-09-30T01:02:03.400000Z"
        );
    }

    #[test]
    fn treats_offsetless_and_date_only_as_utc() {
        assert_eq!(
            normalize_timestamp("2026-09-30T01:02:03").unwrap(),
            "2026-09-30T01:02:03.000000Z"
        );
        assert_eq!(
            normalize_timestamp("2026-09-30").unwrap(),
            "2026-09-30T00:00:00.000000Z"
        );
    }

    #[test]
    fn rejects_garbage() {
        assert!(normalize_timestamp("yesterday").is_err());
    }
}
