//! obs.py timestamp text: `datetime.isoformat()` in UTC with `+00:00` replaced by `Z`
//! (microsecond precision, fraction omitted when it is zero). Spool `created_at` ordering
//! depends on obs-rs writing exactly the same shape as concurrent Python writers.

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use serde_json::Value;

const OFFSET_FORMATS: [&str; 4] = [
    "%Y-%m-%dT%H:%M:%S%.f%:z",
    "%Y-%m-%d %H:%M:%S%.f%:z",
    "%Y-%m-%dT%H:%M%:z",
    "%Y-%m-%d %H:%M%:z",
];

const NAIVE_FORMATS: [&str; 4] = [
    "%Y-%m-%dT%H:%M:%S%.f",
    "%Y-%m-%d %H:%M:%S%.f",
    "%Y-%m-%dT%H:%M",
    "%Y-%m-%d %H:%M",
];

/// obs.py:44 `now()`.
pub fn python_utc_now() -> String {
    python_isoformat_utc(&Utc::now())
}

/// obs.py:127 `observed_time(value)`: re-renders a parseable ISO timestamp in UTC; anything
/// else (missing, not a string, unparseable) becomes `now()`.
pub fn python_observed_time(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .and_then(parse_python_isoformat)
        .map(|instant| python_isoformat_utc(&instant))
        .unwrap_or_else(python_utc_now)
}

/// `datetime.fromisoformat(value.replace("Z", "+00:00"))`, naive values taken as UTC.
fn parse_python_isoformat(text: &str) -> Option<DateTime<Utc>> {
    let offset_text = text.replace('Z', "+00:00");

    parse_with_offset(&offset_text)
        .or_else(|| parse_naive(&offset_text))
        .or_else(|| parse_date_only(&offset_text))
}

fn parse_with_offset(text: &str) -> Option<DateTime<Utc>> {
    OFFSET_FORMATS
        .iter()
        .find_map(|format| DateTime::parse_from_str(text, format).ok())
        .map(|instant| instant.with_timezone(&Utc))
}

fn parse_naive(text: &str) -> Option<DateTime<Utc>> {
    NAIVE_FORMATS
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

fn python_isoformat_utc(instant: &DateTime<Utc>) -> String {
    let seconds = instant.format("%Y-%m-%dT%H:%M:%S");
    let micros = instant.timestamp_subsec_micros().min(999_999);

    if micros == 0 {
        format!("{seconds}Z")
    } else {
        format!("{seconds}.{micros:06}Z")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn converts_offsets_to_utc_like_obs_py() {
        let value = json!("2026-09-03T00:00:00+08:00");
        assert_eq!(python_observed_time(Some(&value)), "2026-09-02T16:00:00Z");
    }

    #[test]
    fn keeps_microseconds_and_z_suffix() {
        let value = json!("2026-09-30T01:02:03.123456Z");
        assert_eq!(
            python_observed_time(Some(&value)),
            "2026-09-30T01:02:03.123456Z"
        );
    }

    #[test]
    fn treats_naive_and_date_only_as_utc() {
        assert_eq!(
            python_observed_time(Some(&json!("2026-09-30T01:02:03.5"))),
            "2026-09-30T01:02:03.500000Z"
        );
        assert_eq!(
            python_observed_time(Some(&json!("2026-09-30"))),
            "2026-09-30T00:00:00Z"
        );
    }

    #[test]
    fn falls_back_to_now_for_garbage() {
        let rendered = python_observed_time(Some(&json!("yesterday")));
        assert!(rendered.ends_with('Z'));
        assert!(rendered.len() >= 20);
    }

    #[test]
    fn now_has_python_shape() {
        let now = python_utc_now();
        assert!(now.ends_with('Z'));
        assert_eq!(&now[4..5], "-");
        assert_eq!(&now[10..11], "T");
    }
}
