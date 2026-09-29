//! Row → JSON encoding that reproduces what node-pg produced from Postgres types, so HTTP
//! clients see identical values: int8/count/sum as strings, booleans as bools, jsonb parsed,
//! bytea selected via `*` as Node `Buffer` JSON.

use anyhow::Result;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, Params, Row};
use serde_json::{json, Map, Value};

/// The Postgres type a selected column had, which decides its JSON rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wire {
    /// text, varchar, uuid, date, char(n).
    Text,
    /// timestamptz stored with microseconds -> milliseconds like JavaScript `Date`.
    Timestamp,
    /// smallint, integer, `::integer` casts.
    Integer,
    /// bigint, `count(*)`, `SUM(...)`, `::bigint` casts → JSON string.
    BigIntText,
    /// double precision → JSON number.
    Real,
    /// boolean stored as 0/1.
    Bool,
    /// jsonb stored as text → parsed JSON.
    Json,
    /// bytea selected directly → `{"type":"Buffer","data":[..]}`.
    Buffer,
}

pub type Columns = &'static [(&'static str, Wire)];

/// Runs `sql` and renders every row as an ordered JSON object using `columns` (same order as the SELECT list).
pub fn fetch_all<P: Params>(
    connection: &Connection,
    sql: &str,
    params: P,
    columns: Columns,
) -> Result<Vec<Value>> {
    let mut statement = connection.prepare_cached(sql)?;
    let rows = statement.query_map(params, |row| Ok(row_to_json(row, columns)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// First row or `None`.
pub fn fetch_optional<P: Params>(
    connection: &Connection,
    sql: &str,
    params: P,
    columns: Columns,
) -> Result<Option<Value>> {
    Ok(fetch_all(connection, sql, params, columns)?
        .into_iter()
        .next())
}

fn row_to_json(row: &Row<'_>, columns: Columns) -> Value {
    let object: Map<String, Value> = columns
        .iter()
        .enumerate()
        .map(|(index, (name, wire))| {
            let value = row.get_ref(index).map(|cell| encode_cell(cell, *wire));
            ((*name).to_owned(), value.unwrap_or(Value::Null))
        })
        .collect();
    Value::Object(object)
}

pub fn encode_cell(cell: ValueRef<'_>, wire: Wire) -> Value {
    match cell {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(number) => encode_integer(number, wire),
        ValueRef::Real(number) => encode_real(number, wire),
        ValueRef::Text(bytes) => encode_text(&String::from_utf8_lossy(bytes), wire),
        ValueRef::Blob(bytes) => encode_blob(bytes, wire),
    }
}

fn encode_integer(number: i64, wire: Wire) -> Value {
    match wire {
        Wire::BigIntText | Wire::Text | Wire::Timestamp => Value::String(number.to_string()),
        Wire::Bool => Value::Bool(number != 0),
        Wire::Integer | Wire::Real | Wire::Json | Wire::Buffer => Value::from(number),
    }
}

fn encode_real(number: f64, wire: Wire) -> Value {
    match wire {
        Wire::BigIntText | Wire::Text | Wire::Timestamp => Value::String(js_number_text(number)),
        _ => js_number_value(number),
    }
}

fn encode_text(text: &str, wire: Wire) -> Value {
    match wire {
        Wire::Timestamp => Value::String(obs_common::to_wire_millis(text)),
        Wire::Json => serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_owned())),
        _ => Value::String(text.to_owned()),
    }
}

fn encode_blob(bytes: &[u8], wire: Wire) -> Value {
    match wire {
        Wire::Text | Wire::Timestamp => Value::String(hex::encode(bytes)),
        _ => node_buffer_json(bytes),
    }
}

/// A JSON response rendered like `JSON.stringify`: lone surrogates echoed from client text are
/// written as `\uXXXX` escapes instead of being replaced.
pub fn js_json_response(status: axum::http::StatusCode, body: &Value) -> axum::response::Response {
    use axum::response::IntoResponse;

    let text = crate::operations::js_values::escape_lone_surrogates(&body.to_string());
    (
        status,
        [(
            axum::http::header::CONTENT_TYPE,
            "application/json; charset=utf-8",
        )],
        text,
    )
        .into_response()
}

/// `JSON.stringify(Buffer)`.
#[inline]
pub fn node_buffer_json(bytes: &[u8]) -> Value {
    json!({ "type": "Buffer", "data": bytes })
}

/// JavaScript renders integral doubles without a fraction and non-finite ones as `null`.
pub fn js_number_value(number: f64) -> Value {
    if !number.is_finite() {
        Value::Null
    } else if is_safe_integer(number) {
        Value::from(number as i64)
    } else {
        serde_json::Number::from_f64(number).map_or(Value::Null, Value::Number)
    }
}

#[inline]
fn js_number_text(number: f64) -> String {
    if is_safe_integer(number) {
        (number as i64).to_string()
    } else {
        number.to_string()
    }
}

#[inline]
fn is_safe_integer(number: f64) -> bool {
    number.fract() == 0.0 && number.abs() < 9_007_199_254_740_992.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_like_node_pg() {
        assert_eq!(
            encode_cell(ValueRef::Integer(7), Wire::BigIntText),
            json!("7")
        );
        assert_eq!(encode_cell(ValueRef::Integer(7), Wire::Integer), json!(7));
        assert_eq!(encode_cell(ValueRef::Integer(1), Wire::Bool), json!(true));
        assert_eq!(encode_cell(ValueRef::Real(1.0), Wire::Real), json!(1));
        assert_eq!(encode_cell(ValueRef::Real(1.5), Wire::Real), json!(1.5));
        assert_eq!(
            encode_cell(ValueRef::Text(b"{\"a\":1}"), Wire::Json),
            json!({"a": 1})
        );
        assert_eq!(
            encode_cell(ValueRef::Blob(&[1, 255]), Wire::Buffer),
            json!({"type": "Buffer", "data": [1, 255]})
        );
        assert_eq!(encode_cell(ValueRef::Null, Wire::BigIntText), Value::Null);
    }
}
