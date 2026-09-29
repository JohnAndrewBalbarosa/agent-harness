//! `core.jsonl.parse_ts`: `datetime.fromisoformat(text.replace("Z", "+00:00")).astimezone(utc)`.
//! Naive timestamps are local time (what `astimezone` assumes), offsets are honoured.

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde_json::Value;

const OFFSET_FORMATS: [&str; 6] = [
    "%Y-%m-%dT%H:%M:%S%.f%:z",
    "%Y-%m-%d %H:%M:%S%.f%:z",
    "%Y-%m-%dT%H:%M%:z",
    "%Y-%m-%d %H:%M%:z",
    "%Y-%m-%dT%H:%M:%S%.f%z",
    "%Y-%m-%d %H:%M:%S%.f%z",
];

const NAIVE_FORMATS: [&str; 4] = [
    "%Y-%m-%dT%H:%M:%S%.f",
    "%Y-%m-%d %H:%M:%S%.f",
    "%Y-%m-%dT%H:%M",
    "%Y-%m-%d %H:%M",
];

/// Parses a JSON timestamp value; non-strings and unparseable text are `None`.
#[inline]
pub fn parse_timestamp_value(value: Option<&Value>) -> Option<DateTime<Utc>> {
    value.and_then(Value::as_str).and_then(parse_iso_instant)
}

pub fn parse_iso_instant(text: &str) -> Option<DateTime<Utc>> {
    let normalized = text.trim().replace('Z', "+00:00");
    parse_with_offset(&normalized).or_else(|| parse_as_local(&normalized))
}

#[inline]
fn parse_with_offset(text: &str) -> Option<DateTime<Utc>> {
    OFFSET_FORMATS.iter().find_map(|format| {
        DateTime::parse_from_str(text, format)
            .ok()
            .map(|instant| instant.with_timezone(&Utc))
    })
}

#[inline]
fn parse_as_local(text: &str) -> Option<DateTime<Utc>> {
    let naive = NAIVE_FORMATS
        .iter()
        .find_map(|format| NaiveDateTime::parse_from_str(text, format).ok())
        .or_else(|| {
            NaiveDate::parse_from_str(text, "%Y-%m-%d")
                .ok()
                .and_then(|date| date.and_hms_opt(0, 0, 0))
        })?;
    Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|instant| instant.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_zulu_and_offset_forms() {
        let zulu = parse_iso_instant("2026-09-28T11:50:00Z").unwrap();
        let offset = parse_iso_instant("2026-09-28T19:50:00.000+08:00").unwrap();
        assert_eq!(zulu, offset);
        let python_isoformat = parse_iso_instant("2026-09-28T11:50:00.123456+00:00").unwrap();
        assert_eq!(python_isoformat.timestamp(), zulu.timestamp());
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_iso_instant("None").is_none());
        assert!(parse_timestamp_value(Some(&Value::from(5))).is_none());
    }
}
