//! Output shapes for `obs query`:
//! - `json`  (default): Python-style `indent=2` array of row objects in column order; BLOBs as
//!   lowercase hex; duplicate column names are kept as written (not merged).
//! - `table`: aligned text with a `(N rows)` footer; cells are single-line and capped.

use crate::python_compat::{python_float_repr, write_json_string};
use crate::query::statement::{CellValue, QueryRows};

const MAX_TABLE_CELL_CHARS: usize = 120;
const TRUNCATION_MARKER: &str = "...";
const NULL_TEXT: &str = "NULL";
const COLUMN_SEPARATOR: &str = " | ";

pub fn render_json(result: &QueryRows) -> String {
    if result.rows.is_empty() {
        return "[]".to_owned();
    }

    let rendered_rows: Vec<String> = result
        .rows
        .iter()
        .map(|row| render_json_row(&result.columns, row))
        .collect();
    format!("[\n{}\n]", rendered_rows.join(",\n"))
}

fn render_json_row(columns: &[String], row: &[CellValue]) -> String {
    if columns.is_empty() {
        return "  {}".to_owned();
    }

    let fields: Vec<String> = columns
        .iter()
        .zip(row)
        .map(|(column, cell)| {
            let mut field = String::from("    ");
            write_json_string(&mut field, column, false);
            field.push_str(": ");
            write_json_cell(&mut field, cell);
            field
        })
        .collect();
    format!("  {{\n{}\n  }}", fields.join(",\n"))
}

fn write_json_cell(out: &mut String, cell: &CellValue) {
    match cell {
        CellValue::Null => out.push_str("null"),
        CellValue::Integer(number) => out.push_str(&number.to_string()),
        CellValue::Real(number) if number.is_finite() => out.push_str(&python_float_repr(*number)),
        CellValue::Real(_) => out.push_str("null"),
        CellValue::Text(text) => write_json_string(out, text, false),
        CellValue::Blob(bytes) => write_json_string(out, &hex::encode(bytes), false),
    }
}

/// Mental model: stringify every cell → measure column widths → pad header, rule, rows.
pub fn render_table(result: &QueryRows) -> String {
    let body: Vec<Vec<String>> = result
        .rows
        .iter()
        .map(|row| row.iter().map(table_cell_text).collect())
        .collect();
    let widths = column_widths(&result.columns, &body);

    let mut lines = Vec::with_capacity(body.len() + 3);
    lines.push(padded_line(
        result.columns.iter().map(String::as_str),
        &widths,
    ));
    lines.push(rule_line(&widths));
    lines.extend(
        body.iter()
            .map(|cells| padded_line(cells.iter().map(String::as_str), &widths)),
    );
    lines.push(row_count_footer(result.rows.len()));
    lines.join("\n")
}

fn table_cell_text(cell: &CellValue) -> String {
    let text = match cell {
        CellValue::Null => NULL_TEXT.to_owned(),
        CellValue::Integer(number) => number.to_string(),
        CellValue::Real(number) => python_float_repr(*number),
        CellValue::Text(text) => single_line(text),
        CellValue::Blob(bytes) => hex::encode(bytes),
    };
    capped(&text)
}

/// Control characters would break alignment; show them escaped.
fn single_line(text: &str) -> String {
    text.chars()
        .flat_map(|character| match character {
            '\n' => vec!['\\', 'n'],
            '\r' => vec!['\\', 'r'],
            '\t' => vec!['\\', 't'],
            other if other.is_control() => vec!['?'],
            other => vec![other],
        })
        .collect()
}

fn capped(text: &str) -> String {
    if text.chars().count() <= MAX_TABLE_CELL_CHARS {
        return text.to_owned();
    }
    let kept: String = text
        .chars()
        .take(MAX_TABLE_CELL_CHARS - TRUNCATION_MARKER.len())
        .collect();
    format!("{kept}{TRUNCATION_MARKER}")
}

fn column_widths(columns: &[String], body: &[Vec<String>]) -> Vec<usize> {
    columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let widest_cell = body
                .iter()
                .filter_map(|cells| cells.get(index))
                .map(|cell| cell.chars().count())
                .max()
                .unwrap_or(0);
            widest_cell.max(column.chars().count())
        })
        .collect()
}

fn padded_line<'a>(cells: impl Iterator<Item = &'a str>, widths: &[usize]) -> String {
    let padded: Vec<String> = cells
        .zip(widths.iter().copied())
        .map(|(cell, width)| format!("{cell:<width$}"))
        .collect();
    padded.join(COLUMN_SEPARATOR).trim_end().to_owned()
}

#[inline]
fn rule_line(widths: &[usize]) -> String {
    let rules: Vec<String> = widths.iter().map(|width| "-".repeat(*width)).collect();
    rules.join("-+-")
}

#[inline]
fn row_count_footer(count: usize) -> String {
    if count == 1 {
        "(1 row)".to_owned()
    } else {
        format!("({count} rows)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> QueryRows {
        QueryRows {
            columns: vec!["id".to_owned(), "name".to_owned(), "id".to_owned()],
            rows: vec![vec![
                CellValue::Integer(1),
                CellValue::Text("é\nx".to_owned()),
                CellValue::Blob(vec![0xab]),
            ]],
            is_truncated: false,
        }
    }

    #[test]
    fn json_keeps_duplicate_columns_and_hex_blobs() {
        assert_eq!(
            render_json(&sample()),
            "[\n  {\n    \"id\": 1,\n    \"name\": \"é\\nx\",\n    \"id\": \"ab\"\n  }\n]"
        );
    }

    #[test]
    fn empty_result_is_an_empty_array() {
        let empty = QueryRows {
            columns: vec!["id".to_owned()],
            rows: Vec::new(),
            is_truncated: false,
        };
        assert_eq!(render_json(&empty), "[]");
        assert_eq!(render_table(&empty), "id\n--\n(0 rows)");
    }

    #[test]
    fn table_aligns_columns() {
        assert_eq!(
            render_table(&sample()),
            "id | name | id\n---+------+---\n1  | é\\nx | ab\n(1 row)"
        );
    }
}
