//! Streams one `<table>.jsonl` file into its SQLite table, one line in memory at a time.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use rusqlite::types::Value as SqlValue;
use rusqlite::{params_from_iter, Connection, Statement};
use serde_json::{Map, Value as JsonValue};

use super::column_conversion::convert_value;
use super::import_plan::TableImportPlan;

const SKIP_REASON_BLANK_LINE: &str = "blank line";
const READ_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Debug, Default)]
pub struct TableInsertOutcome {
    pub imported: u64,
    pub skip_reasons: BTreeMap<String, u64>,
}

impl TableInsertOutcome {
    #[inline]
    pub fn skipped(&self) -> u64 {
        self.skip_reasons.values().sum()
    }

    fn record_skip(&mut self, reason: &str) {
        *self.skip_reasons.entry(reason.to_owned()).or_insert(0) += 1;
    }
}

// Mental model: open file → prepare one INSERT → for each line: blank? skip : parse → convert → execute.
// Any failure aborts the whole import with "table X line N" so the caller rolls back.
pub fn insert_table_rows(
    connection: &Connection,
    plan: &TableImportPlan,
    source_path: &Path,
) -> Result<TableInsertOutcome> {
    let file = File::open(source_path)
        .with_context(|| format!("table {}: open {}", plan.table, source_path.display()))?;
    let mut reader = BufReader::with_capacity(READ_BUFFER_BYTES, file);
    let mut statement = connection
        .prepare(&plan.insert_sql())
        .with_context(|| format!("table {}: prepare insert", plan.table))?;

    let mut outcome = TableInsertOutcome::default();
    let mut line = String::new();
    let mut line_number: u64 = 0;

    loop {
        line.clear();
        let bytes_read = reader
            .read_line(&mut line)
            .with_context(|| format!("table {} line {}: read", plan.table, line_number + 1))?;
        if bytes_read == 0 {
            break;
        }
        line_number += 1;

        process_line(&mut statement, plan, &line, &mut outcome)
            .with_context(|| format!("table {} line {line_number}", plan.table))?;
    }

    Ok(outcome)
}

fn process_line(
    statement: &mut Statement<'_>,
    plan: &TableImportPlan,
    line: &str,
    outcome: &mut TableInsertOutcome,
) -> Result<()> {
    let content = line.trim_end_matches(['\n', '\r']);
    if is_blank(content) {
        outcome.record_skip(SKIP_REASON_BLANK_LINE);
        return Ok(());
    }

    let row = parse_row_object(content)?;
    let values = convert_row(plan, &row)?;
    statement
        .execute(params_from_iter(values))
        .context("insert row")?;
    outcome.imported += 1;
    Ok(())
}

fn parse_row_object(content: &str) -> Result<Map<String, JsonValue>> {
    match serde_json::from_str::<JsonValue>(content).context("parse JSON line")? {
        JsonValue::Object(row) => Ok(row),
        _ => Err(anyhow!("JSON line is not an object")),
    }
}

fn convert_row(plan: &TableImportPlan, row: &Map<String, JsonValue>) -> Result<Vec<SqlValue>> {
    plan.columns
        .iter()
        .map(|column| {
            let value = row
                .get(&column.name)
                .ok_or_else(|| anyhow!("column {} missing from JSON line", column.name))?;
            convert_value(column.conversion, value)
                .with_context(|| format!("column {}", column.name))
        })
        .collect()
}

#[inline]
fn is_blank(content: &str) -> bool {
    content.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::super::import_plan::plan_table_import;
    use super::super::manifest::ExportManifest;
    use super::*;
    use std::io::Write;

    const PROJECTS_MANIFEST: &str = r#"{"tables":[{"name":"projects","row_count":1,"columns":[
        {"name":"project_id","type":"uuid"},{"name":"display_name","type":"text"},
        {"name":"slug","type":"text"},{"name":"status","type":"int2"},
        {"name":"created_at","type":"timestamptz"},{"name":"last_seen_at","type":"timestamptz"}]}]}"#;

    fn write_temp_jsonl(name: &str, content: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("obs-import-{}-{name}.jsonl", std::process::id()));
        let mut file = File::create(&path).unwrap();
        file.write_all(content.as_bytes()).unwrap();
        path
    }

    fn schema_connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(include_str!("../store/schema.sql"))
            .unwrap();
        connection
    }

    #[test]
    fn inserts_converted_rows_and_counts_blank_lines_as_skipped() {
        let connection = schema_connection();
        let manifest: ExportManifest = serde_json::from_str(PROJECTS_MANIFEST).unwrap();
        let plan = plan_table_import(&connection, "projects", manifest.table("projects").unwrap())
            .unwrap();
        let path = write_temp_jsonl(
            "ok",
            "{\"project_id\":\"01A07F1A-A3B3-727D-80DE-D2DBB1C38326\",\"display_name\":\"demo\",\"slug\":\"demo\",\"status\":1,\"created_at\":\"2026-09-14T07:14:17.643994+00:00\",\"last_seen_at\":\"2026-09-14T07:14:17+00:00\"}\r\n\n",
        );

        let outcome = insert_table_rows(&connection, &plan, &path).unwrap();

        assert_eq!(outcome.imported, 1);
        assert_eq!(outcome.skipped(), 1);
        let (id, created): (String, String) = connection
            .query_row("SELECT project_id, created_at FROM projects", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(id, "01a07f1a-a3b3-727d-80de-d2dbb1c38326");
        assert_eq!(created, "2026-09-14T07:14:17.643994Z");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn failure_names_table_and_line() {
        let connection = schema_connection();
        let manifest: ExportManifest = serde_json::from_str(PROJECTS_MANIFEST).unwrap();
        let plan = plan_table_import(&connection, "projects", manifest.table("projects").unwrap())
            .unwrap();
        let path = write_temp_jsonl("bad", "\n{\"project_id\":\"not-a-uuid\"}\n");

        let error = insert_table_rows(&connection, &plan, &path).unwrap_err();

        assert!(format!("{error:#}").contains("table projects line 2"));
        std::fs::remove_file(path).unwrap();
    }
}
