//! Per-column rules turning a `row_to_json` value into the SQLite value the target schema
//! expects (see the type mapping header of `store/schema.sql`).
//!
//! | Postgres `udt_name`            | JSON shape from row_to_json     | SQLite value                  |
//! |--------------------------------|---------------------------------|-------------------------------|
//! | uuid                           | string                          | TEXT lowercase hyphenated     |
//! | timestamptz, timestamp         | string `...+00:00`              | TEXT UTC ms `...456Z`         |
//! | date                           | string `YYYY-MM-DD`             | TEXT unchanged (validated)    |
//! | json, jsonb                    | nested JSON value               | TEXT compact JSON             |
//! | bytea                          | string `\x0a1b...`              | BLOB                          |
//! | bool                           | true / false                    | INTEGER 0 / 1                 |
//! | int2, int4, int8               | number                          | INTEGER (must fit i64)        |
//! | float4, float8, numeric        | number or "NaN"/"Infinity"      | REAL                          |
//! | text, varchar, name            | string                          | TEXT unchanged                |
//! | bpchar                         | string (blank padded)           | TEXT, trailing blanks removed |
//!
//! JSON `null` becomes SQL NULL for every rule; NOT NULL constraints stay with SQLite.

use anyhow::{anyhow, bail, Result};
use chrono::NaiveDate;
use obs_common::{normalize_timestamp, normalize_uuid};
use rusqlite::types::Value as SqlValue;
use serde_json::Value as JsonValue;

const BYTEA_HEX_PREFIX: &str = "\\x";
const DATE_FORMAT: &str = "%Y-%m-%d";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnConversion {
    Uuid,
    Timestamp,
    Date,
    Json,
    Bytea,
    Boolean,
    Integer,
    Float,
    Text,
    BlankPaddedText,
}

impl ColumnConversion {
    pub fn for_source_type(source_type: &str) -> Result<Self> {
        let conversion = match source_type {
            "uuid" => Self::Uuid,
            "timestamptz" | "timestamp" => Self::Timestamp,
            "date" => Self::Date,
            "json" | "jsonb" => Self::Json,
            "bytea" => Self::Bytea,
            "bool" => Self::Boolean,
            "int2" | "int4" | "int8" => Self::Integer,
            "float4" | "float8" | "numeric" => Self::Float,
            "text" | "varchar" | "name" => Self::Text,
            "bpchar" => Self::BlankPaddedText,
            other => bail!("no conversion rule for Postgres type {other:?}"),
        };
        Ok(conversion)
    }

    /// Declared SQLite type the target column must have for this rule to be valid.
    pub fn expected_target_type(self) -> &'static str {
        match self {
            Self::Bytea => "BLOB",
            Self::Boolean | Self::Integer => "INTEGER",
            Self::Float => "REAL",
            Self::Uuid
            | Self::Timestamp
            | Self::Date
            | Self::Json
            | Self::Text
            | Self::BlankPaddedText => "TEXT",
        }
    }
}

pub fn convert_value(conversion: ColumnConversion, value: &JsonValue) -> Result<SqlValue> {
    if value.is_null() {
        return Ok(SqlValue::Null);
    }

    match conversion {
        ColumnConversion::Uuid => convert_uuid(value),
        ColumnConversion::Timestamp => convert_timestamp(value),
        ColumnConversion::Date => convert_date(value),
        ColumnConversion::Json => convert_json(value),
        ColumnConversion::Bytea => convert_bytea(value),
        ColumnConversion::Boolean => convert_boolean(value),
        ColumnConversion::Integer => convert_integer(value),
        ColumnConversion::Float => convert_float(value),
        ColumnConversion::Text => convert_text(value),
        ColumnConversion::BlankPaddedText => convert_blank_padded_text(value),
    }
}

fn convert_uuid(value: &JsonValue) -> Result<SqlValue> {
    let text = expect_string(value, "uuid")?;
    let normalized = normalize_uuid(text)?;
    Ok(SqlValue::Text(normalized))
}

fn convert_timestamp(value: &JsonValue) -> Result<SqlValue> {
    let text = expect_string(value, "timestamp")?;
    let normalized = normalize_timestamp(text)?;
    Ok(SqlValue::Text(normalized))
}

fn convert_date(value: &JsonValue) -> Result<SqlValue> {
    let text = expect_string(value, "date")?;
    NaiveDate::parse_from_str(text, DATE_FORMAT)
        .map_err(|_| anyhow!("invalid date {:?}; expected YYYY-MM-DD", preview(text)))?;
    Ok(SqlValue::Text(text.to_owned()))
}

fn convert_json(value: &JsonValue) -> Result<SqlValue> {
    Ok(SqlValue::Text(serde_json::to_string(value)?))
}

fn convert_bytea(value: &JsonValue) -> Result<SqlValue> {
    let text = expect_string(value, "bytea")?;
    let hex_digits = text
        .strip_prefix(BYTEA_HEX_PREFIX)
        .ok_or_else(|| anyhow!("bytea {:?} lacks the \\x hex prefix", preview(text)))?;
    let bytes = hex::decode(hex_digits)
        .map_err(|error| anyhow!("bytea {:?} is not valid hex: {error}", preview(text)))?;
    Ok(SqlValue::Blob(bytes))
}

fn convert_boolean(value: &JsonValue) -> Result<SqlValue> {
    let flag = value
        .as_bool()
        .ok_or_else(|| anyhow!("expected boolean, found {}", json_kind(value)))?;
    Ok(SqlValue::Integer(boolean_as_integer(flag)))
}

fn convert_integer(value: &JsonValue) -> Result<SqlValue> {
    let number = value
        .as_i64()
        .ok_or_else(|| anyhow!("expected integer representable as i64, found {value}"))?;
    Ok(SqlValue::Integer(number))
}

fn convert_float(value: &JsonValue) -> Result<SqlValue> {
    let number = match value {
        JsonValue::Number(number) => number.as_f64(),
        JsonValue::String(text) => parse_special_float(text),
        _ => None,
    };
    number
        .map(SqlValue::Real)
        .ok_or_else(|| anyhow!("expected float, found {}", json_kind(value)))
}

fn convert_text(value: &JsonValue) -> Result<SqlValue> {
    Ok(SqlValue::Text(expect_string(value, "text")?.to_owned()))
}

fn convert_blank_padded_text(value: &JsonValue) -> Result<SqlValue> {
    let text = expect_string(value, "character")?;
    Ok(SqlValue::Text(text.trim_end_matches(' ').to_owned()))
}

/// row_to_json renders non-finite float8 values as JSON strings.
fn parse_special_float(text: &str) -> Option<f64> {
    match text {
        "NaN" => Some(f64::NAN),
        "Infinity" => Some(f64::INFINITY),
        "-Infinity" => Some(f64::NEG_INFINITY),
        _ => None,
    }
}

fn expect_string<'a>(value: &'a JsonValue, expected: &str) -> Result<&'a str> {
    value
        .as_str()
        .ok_or_else(|| anyhow!("expected {expected} string, found {}", json_kind(value)))
}

#[inline]
fn boolean_as_integer(flag: bool) -> i64 {
    i64::from(flag)
}

#[inline]
fn json_kind(value: &JsonValue) -> &'static str {
    match value {
        JsonValue::Null => "null",
        JsonValue::Bool(_) => "boolean",
        JsonValue::Number(_) => "number",
        JsonValue::String(_) => "string",
        JsonValue::Array(_) => "array",
        JsonValue::Object(_) => "object",
    }
}

/// Error messages never echo whole values (they may hold user prompt text).
#[inline]
fn preview(text: &str) -> String {
    text.chars().take(48).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn convert(conversion: ColumnConversion, value: JsonValue) -> SqlValue {
        convert_value(conversion, &value).unwrap()
    }

    #[test]
    fn maps_every_exported_postgres_type() {
        let cases = [
            ("uuid", ColumnConversion::Uuid),
            ("timestamptz", ColumnConversion::Timestamp),
            ("date", ColumnConversion::Date),
            ("jsonb", ColumnConversion::Json),
            ("bytea", ColumnConversion::Bytea),
            ("bool", ColumnConversion::Boolean),
            ("int2", ColumnConversion::Integer),
            ("int4", ColumnConversion::Integer),
            ("int8", ColumnConversion::Integer),
            ("float8", ColumnConversion::Float),
            ("text", ColumnConversion::Text),
            ("varchar", ColumnConversion::Text),
            ("bpchar", ColumnConversion::BlankPaddedText),
        ];
        for (source_type, expected) in cases {
            assert_eq!(
                ColumnConversion::for_source_type(source_type).unwrap(),
                expected
            );
        }
        assert!(ColumnConversion::for_source_type("tsvector").is_err());
    }

    #[test]
    fn expected_target_types_follow_schema_mapping() {
        assert_eq!(ColumnConversion::Bytea.expected_target_type(), "BLOB");
        assert_eq!(ColumnConversion::Boolean.expected_target_type(), "INTEGER");
        assert_eq!(ColumnConversion::Float.expected_target_type(), "REAL");
        assert_eq!(ColumnConversion::Uuid.expected_target_type(), "TEXT");
        assert_eq!(ColumnConversion::Json.expected_target_type(), "TEXT");
    }

    #[test]
    fn null_is_null_for_every_rule() {
        assert_eq!(
            convert(ColumnConversion::Bytea, JsonValue::Null),
            SqlValue::Null
        );
        assert_eq!(
            convert(ColumnConversion::Integer, JsonValue::Null),
            SqlValue::Null
        );
        assert_eq!(
            convert(ColumnConversion::Json, JsonValue::Null),
            SqlValue::Null
        );
    }

    #[test]
    fn uuid_is_lowercased() {
        assert_eq!(
            convert(
                ColumnConversion::Uuid,
                json!("01A07F1A-A3B3-727D-80DE-D2DBB1C38326")
            ),
            SqlValue::Text("01a07f1a-a3b3-727d-80de-d2dbb1c38326".into())
        );
        assert!(convert_value(ColumnConversion::Uuid, &json!("nope")).is_err());
    }

    #[test]
    fn row_to_json_timestamp_becomes_utc_micros() {
        assert_eq!(
            convert(
                ColumnConversion::Timestamp,
                json!("2026-09-14T07:14:17.643994+00:00")
            ),
            SqlValue::Text("2026-09-14T07:14:17.643994Z".into())
        );
        assert_eq!(
            convert(
                ColumnConversion::Timestamp,
                json!("2026-09-14T15:14:17.5+08:00")
            ),
            SqlValue::Text("2026-09-14T07:14:17.500000Z".into())
        );
    }

    #[test]
    fn date_is_kept_and_validated() {
        assert_eq!(
            convert(ColumnConversion::Date, json!("2026-09-03")),
            SqlValue::Text("2026-09-03".into())
        );
        assert!(convert_value(ColumnConversion::Date, &json!("infinity")).is_err());
    }

    #[test]
    fn jsonb_becomes_compact_text() {
        assert_eq!(
            convert(ColumnConversion::Json, json!({"tool": "integration-test"})),
            SqlValue::Text(r#"{"tool":"integration-test"}"#.into())
        );
        assert_eq!(
            convert(ColumnConversion::Json, json!([])),
            SqlValue::Text("[]".into())
        );
    }

    #[test]
    fn bytea_hex_becomes_blob() {
        assert_eq!(
            convert(ColumnConversion::Bytea, json!("\\x0a1bFF")),
            SqlValue::Blob(vec![0x0a, 0x1b, 0xff])
        );
        assert_eq!(
            convert(ColumnConversion::Bytea, json!("\\x")),
            SqlValue::Blob(vec![])
        );
        assert!(convert_value(ColumnConversion::Bytea, &json!("0a1b")).is_err());
        assert!(convert_value(ColumnConversion::Bytea, &json!("\\x0g")).is_err());
    }

    #[test]
    fn boolean_becomes_zero_or_one() {
        assert_eq!(
            convert(ColumnConversion::Boolean, json!(true)),
            SqlValue::Integer(1)
        );
        assert_eq!(
            convert(ColumnConversion::Boolean, json!(false)),
            SqlValue::Integer(0)
        );
        assert!(convert_value(ColumnConversion::Boolean, &json!("t")).is_err());
    }

    #[test]
    fn integer_must_fit_i64() {
        assert_eq!(
            convert(ColumnConversion::Integer, json!(9_007_199_254_740_993_i64)),
            SqlValue::Integer(9_007_199_254_740_993)
        );
        assert!(convert_value(ColumnConversion::Integer, &json!(u64::MAX)).is_err());
        assert!(convert_value(ColumnConversion::Integer, &json!(1.5)).is_err());
    }

    #[test]
    fn float_accepts_numbers_and_non_finite_strings() {
        assert_eq!(
            convert(ColumnConversion::Float, json!(0.05)),
            SqlValue::Real(0.05)
        );
        assert_eq!(
            convert(ColumnConversion::Float, json!(3)),
            SqlValue::Real(3.0)
        );
        assert_eq!(
            convert(ColumnConversion::Float, json!("Infinity")),
            SqlValue::Real(f64::INFINITY)
        );
        assert!(convert_value(ColumnConversion::Float, &json!("1.5")).is_err());
    }

    #[test]
    fn text_is_kept_and_bpchar_padding_trimmed() {
        assert_eq!(
            convert(ColumnConversion::Text, json!("  keep  ")),
            SqlValue::Text("  keep  ".into())
        );
        assert_eq!(
            convert(ColumnConversion::BlankPaddedText, json!("abc   ")),
            SqlValue::Text("abc".into())
        );
        assert!(convert_value(ColumnConversion::Text, &json!(5)).is_err());
    }
}
