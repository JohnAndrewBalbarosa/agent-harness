//! Request-shape validation equivalent to the zod schema in server.mjs:18-24:
//! `{ operations: [{ operationId: uuid, kind: string(3..80), occurredAt: ISO datetime Z, data: record }] }`
//! with 1..500 operations. Issues use zod's `{ path, code }` vocabulary.

use chrono::NaiveDate;
use serde::Serialize;
use serde_json::{Map, Value};

use super::js_values::{js_utf16_length, truncate_utf16};
use super::Operation;

pub const MAX_OPERATIONS_PER_BATCH: usize = 500;
const MIN_KIND_LENGTH: usize = 3;
const MAX_KIND_LENGTH: usize = 80;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ValidationIssue {
    pub path: Vec<Value>,
    pub code: &'static str,
}

// Mental model:
// body is an object → `operations` is an array → each item has the four typed fields
// → array size is within 1..500 → normalized operations
pub fn parse_operation_batch(body: &Value) -> Result<Vec<Operation>, Vec<ValidationIssue>> {
    let Some(root) = body.as_object() else {
        return Err(vec![issue(Vec::new(), "invalid_type")]);
    };
    let Some(items) = root.get("operations").and_then(Value::as_array) else {
        return Err(vec![issue(vec!["operations".into()], "invalid_type")]);
    };

    let mut issues = Vec::new();
    let operations: Vec<Operation> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| parse_operation(index, item, &mut issues))
        .collect();

    issues.extend(batch_size_issue(items.len()));

    if issues.is_empty() {
        Ok(operations)
    } else {
        Err(issues)
    }
}

fn parse_operation(
    index: usize,
    item: &Value,
    issues: &mut Vec<ValidationIssue>,
) -> Option<Operation> {
    let path_to = |field: &str| vec![Value::from("operations"), Value::from(index), field.into()];

    let Some(object) = item.as_object() else {
        issues.push(issue(
            vec![Value::from("operations"), Value::from(index)],
            "invalid_type",
        ));
        return None;
    };

    let before = issues.len();
    let operation_id = check_uuid_field(object, "operationId", &path_to, issues);
    let kind = check_kind_field(object, &path_to, issues);
    let occurred_at = check_datetime_field(object, "occurredAt", &path_to, issues);
    let data = check_record_field(object, "data", &path_to, issues);

    if issues.len() != before {
        return None;
    }

    Some(Operation {
        operation_id: operation_id?,
        kind: kind?,
        occurred_at: occurred_at?,
        data: data?,
    })
}

fn check_uuid_field(
    object: &Map<String, Value>,
    field: &str,
    path_to: &dyn Fn(&str) -> Vec<Value>,
    issues: &mut Vec<ValidationIssue>,
) -> Option<String> {
    let Some(text) = object.get(field).and_then(Value::as_str) else {
        issues.push(issue(path_to(field), "invalid_type"));
        return None;
    };
    if !is_zod_uuid(text) {
        issues.push(issue(path_to(field), "invalid_format"));
        return None;
    }
    Some(text.to_ascii_lowercase())
}

fn check_kind_field(
    object: &Map<String, Value>,
    path_to: &dyn Fn(&str) -> Vec<Value>,
    issues: &mut Vec<ValidationIssue>,
) -> Option<String> {
    let Some(kind) = object.get("kind").and_then(Value::as_str) else {
        issues.push(issue(path_to("kind"), "invalid_type"));
        return None;
    };
    let length = js_utf16_length(kind);
    if length < MIN_KIND_LENGTH {
        issues.push(issue(path_to("kind"), "too_small"));
        return None;
    }
    if length > MAX_KIND_LENGTH {
        issues.push(issue(path_to("kind"), "too_big"));
        return None;
    }
    Some(kind.to_owned())
}

fn check_datetime_field(
    object: &Map<String, Value>,
    field: &str,
    path_to: &dyn Fn(&str) -> Vec<Value>,
    issues: &mut Vec<ValidationIssue>,
) -> Option<String> {
    let Some(text) = object.get(field).and_then(Value::as_str) else {
        issues.push(issue(path_to(field), "invalid_type"));
        return None;
    };
    let normalized = is_zod_datetime(text)
        .then(|| obs_common::normalize_timestamp(text).ok())
        .flatten();
    if normalized.is_none() {
        issues.push(issue(path_to(field), "invalid_format"));
    }
    normalized
}

fn check_record_field(
    object: &Map<String, Value>,
    field: &str,
    path_to: &dyn Fn(&str) -> Vec<Value>,
    issues: &mut Vec<ValidationIssue>,
) -> Option<Map<String, Value>> {
    match object.get(field) {
        Some(Value::Object(record)) => Some(record.clone()),
        _ => {
            issues.push(issue(path_to(field), "invalid_type"));
            None
        }
    }
}

fn batch_size_issue(count: usize) -> Option<ValidationIssue> {
    if count == 0 {
        Some(issue(vec!["operations".into()], "too_small"))
    } else if count > MAX_OPERATIONS_PER_BATCH {
        Some(issue(vec!["operations".into()], "too_big"))
    } else {
        None
    }
}

/// zod v4 `.uuid()`: RFC 9562 layout with version 1-8 and variant 8-b, or the nil/max UUIDs.
fn is_zod_uuid(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if lower == "00000000-0000-0000-0000-000000000000"
        || lower == "ffffffff-ffff-ffff-ffff-ffffffffffff"
    {
        return true;
    }
    let bytes = lower.as_bytes();
    bytes.len() == 36
        && has_uuid_hyphens(bytes)
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| is_hyphen_position(index) || byte.is_ascii_hexdigit())
        && (b'1'..=b'8').contains(&bytes[14])
        && matches!(bytes[19], b'8' | b'9' | b'a' | b'b')
}

#[inline]
fn has_uuid_hyphens(bytes: &[u8]) -> bool {
    [8, 13, 18, 23].iter().all(|&index| bytes[index] == b'-')
}

#[inline]
fn is_hyphen_position(index: usize) -> bool {
    matches!(index, 8 | 13 | 18 | 23)
}

/// zod v4 `.datetime()` defaults: calendar-valid date, `T`, `HH:MM[:SS[.fraction]]`, literal `Z`.
fn is_zod_datetime(text: &str) -> bool {
    let Some((date, time)) = text.split_once('T') else {
        return false;
    };
    let Some(clock) = time.strip_suffix('Z') else {
        return false;
    };
    is_calendar_date(date) && is_clock_time(clock)
}

#[inline]
fn is_calendar_date(date: &str) -> bool {
    date.len() == 10 && NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok()
}

fn is_clock_time(clock: &str) -> bool {
    let (whole, fraction) = match clock.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (clock, None),
    };
    let parts: Vec<&str> = whole.split(':').collect();

    let fraction_is_valid = match fraction {
        None => true,
        Some(digits) => parts.len() == 3 && is_digits(digits) && !digits.is_empty(),
    };

    fraction_is_valid
        && matches!(parts.len(), 2 | 3)
        && is_two_digit_in_range(parts[0], 23)
        && is_two_digit_in_range(parts[1], 59)
        && parts
            .get(2)
            .is_none_or(|seconds| is_two_digit_in_range(seconds, 59))
}

#[inline]
fn is_two_digit_in_range(text: &str, max: u32) -> bool {
    text.len() == 2 && is_digits(text) && text.parse::<u32>().is_ok_and(|value| value <= max)
}

#[inline]
fn is_digits(text: &str) -> bool {
    text.bytes().all(|byte| byte.is_ascii_digit())
}

#[inline]
fn issue(path: Vec<Value>, code: &'static str) -> ValidationIssue {
    ValidationIssue { path, code }
}

/// Error messages returned to clients are capped at 300 UTF-16 units like `text(message, 300)`.
pub fn client_error_message(error: &anyhow::Error) -> String {
    truncate_utf16(&format!("{error:#}"), 300)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_operation() -> Value {
        json!({
            "operationId": "01928c5e-7d3a-7b2c-9d4e-5f6a7b8c9d0e",
            "kind": "session.end",
            "occurredAt": "2026-09-30T01:02:03.456Z",
            "data": {}
        })
    }

    #[test]
    fn accepts_a_valid_batch_and_normalizes_time() {
        let parsed = parse_operation_batch(&json!({"operations": [valid_operation()]})).unwrap();
        assert_eq!(parsed[0].occurred_at, "2026-09-30T01:02:03.456000Z");
    }

    #[test]
    fn reports_zod_style_issues() {
        let mut bad = valid_operation();
        bad["operationId"] = json!("nope");
        bad["kind"] = json!("ab");
        bad["occurredAt"] = json!("2026-09-30T01:02:03+08:00");
        bad["data"] = json!([]);

        let issues = parse_operation_batch(&json!({"operations": [bad]})).unwrap_err();
        let codes: Vec<_> = issues.iter().map(|issue| issue.code).collect();
        assert_eq!(
            codes,
            [
                "invalid_format",
                "too_small",
                "invalid_format",
                "invalid_type"
            ]
        );
        assert_eq!(
            issues[0].path,
            vec![json!("operations"), json!(0), json!("operationId")]
        );
    }

    #[test]
    fn enforces_batch_size_bounds() {
        let empty = parse_operation_batch(&json!({"operations": []})).unwrap_err();
        assert_eq!(empty, vec![issue(vec!["operations".into()], "too_small")]);

        let missing = parse_operation_batch(&json!({})).unwrap_err();
        assert_eq!(missing[0].code, "invalid_type");
    }

    #[test]
    fn datetime_rules_match_zod_defaults() {
        assert!(is_zod_datetime("2024-02-29T23:59Z"));
        assert!(!is_zod_datetime("2023-02-29T00:00:00Z"));
        assert!(!is_zod_datetime("2026-09-30T24:00:00Z"));
        assert!(!is_zod_datetime("2026-09-30 01:02:03Z"));
    }
}
