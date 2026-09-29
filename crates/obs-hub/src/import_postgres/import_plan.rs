//! Joins the dump's column list (name + Postgres type) with the SQLite target table
//! (`PRAGMA table_info`) into one insert plan per table.

use anyhow::{bail, Context, Result};
use rusqlite::Connection;

use super::column_conversion::ColumnConversion;
use super::manifest::ManifestTable;

#[derive(Debug)]
pub struct PlannedColumn {
    pub name: String,
    pub conversion: ColumnConversion,
}

#[derive(Debug)]
pub struct TableImportPlan {
    pub table: &'static str,
    /// Columns present in both the dump and the target, in dump order.
    pub columns: Vec<PlannedColumn>,
    pub dropped_columns: Vec<String>,
    pub defaulted_columns: Vec<String>,
}

#[derive(Debug)]
struct TargetColumn {
    name: String,
    declared_type: String,
}

impl TableImportPlan {
    pub fn insert_sql(&self) -> String {
        let column_list = self
            .columns
            .iter()
            .map(|column| quote_identifier(&column.name))
            .collect::<Vec<_>>()
            .join(", ");
        let placeholders = (1..=self.columns.len())
            .map(|index| format!("?{index}"))
            .collect::<Vec<_>>()
            .join(", ");

        format!(
            "INSERT INTO {} ({column_list}) VALUES ({placeholders})",
            quote_identifier(self.table)
        )
    }
}

pub fn plan_table_import(
    connection: &Connection,
    table: &'static str,
    manifest_table: &ManifestTable,
) -> Result<TableImportPlan> {
    let target_columns = read_target_columns(connection, table)?;
    if manifest_table.columns().is_empty() {
        bail!("manifest.json lists no columns for table {table}; cannot map types");
    }

    let mut columns = Vec::new();
    let mut dropped_columns = Vec::new();
    for source in manifest_table.columns() {
        match find_target_column(&target_columns, &source.name) {
            Some(target) => columns.push(plan_column(
                table,
                &source.name,
                &source.source_type,
                target,
            )?),
            None => dropped_columns.push(source.name.clone()),
        }
    }

    let defaulted_columns = target_columns
        .iter()
        .filter(|target| !columns.iter().any(|planned| planned.name == target.name))
        .map(|target| target.name.clone())
        .collect();

    if columns.is_empty() {
        bail!("table {table}: no dump column matches a SQLite column");
    }

    Ok(TableImportPlan {
        table,
        columns,
        dropped_columns,
        defaulted_columns,
    })
}

fn plan_column(
    table: &str,
    column: &str,
    source_type: &str,
    target: &TargetColumn,
) -> Result<PlannedColumn> {
    let conversion = ColumnConversion::for_source_type(source_type)
        .with_context(|| format!("table {table} column {column}"))?;

    if !declared_type_matches(&target.declared_type, conversion.expected_target_type()) {
        bail!(
            "table {table} column {column}: Postgres {source_type} needs SQLite {}, target declares {:?}",
            conversion.expected_target_type(),
            target.declared_type
        );
    }

    Ok(PlannedColumn {
        name: column.to_owned(),
        conversion,
    })
}

fn read_target_columns(connection: &Connection, table: &str) -> Result<Vec<TargetColumn>> {
    let mut statement = connection
        .prepare("SELECT name, type FROM pragma_table_info(?1) ORDER BY cid")
        .context("prepare pragma_table_info")?;
    let columns = statement
        .query_map([table], |row| {
            Ok(TargetColumn {
                name: row.get(0)?,
                declared_type: row.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .with_context(|| format!("read SQLite columns of {table}"))?;

    if columns.is_empty() {
        bail!("SQLite target has no table {table}; schema.sql is out of date");
    }
    Ok(columns)
}

#[inline]
fn find_target_column<'a>(targets: &'a [TargetColumn], name: &str) -> Option<&'a TargetColumn> {
    targets.iter().find(|target| target.name == name)
}

#[inline]
fn declared_type_matches(declared: &str, expected: &str) -> bool {
    declared.trim().eq_ignore_ascii_case(expected)
}

#[inline]
fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::super::manifest::ExportManifest;
    use super::*;

    fn schema_connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(include_str!("../store/schema.sql"))
            .unwrap();
        connection
    }

    fn manifest_with(table_json: &str) -> ExportManifest {
        serde_json::from_str(&format!(r#"{{"tables":[{table_json}]}}"#)).unwrap()
    }

    #[test]
    fn plans_matching_columns_and_reports_dropped_and_defaulted() {
        let connection = schema_connection();
        let manifest = manifest_with(
            r#"{"name":"projects","row_count":1,"columns":[
                {"name":"project_id","type":"uuid"},
                {"name":"display_name","type":"text"},
                {"name":"slug","type":"text"},
                {"name":"legacy_only","type":"text"}]}"#,
        );

        let plan = plan_table_import(&connection, "projects", manifest.table("projects").unwrap())
            .unwrap();

        assert_eq!(plan.columns.len(), 3);
        assert_eq!(plan.columns[0].conversion, ColumnConversion::Uuid);
        assert_eq!(plan.dropped_columns, vec!["legacy_only".to_string()]);
        assert_eq!(
            plan.defaulted_columns,
            vec!["status", "created_at", "last_seen_at"]
        );
        assert_eq!(
            plan.insert_sql(),
            r#"INSERT INTO "projects" ("project_id", "display_name", "slug") VALUES (?1, ?2, ?3)"#
        );
    }

    #[test]
    fn rejects_type_that_disagrees_with_target_affinity() {
        let connection = schema_connection();
        let manifest = manifest_with(
            r#"{"name":"project_aliases","row_count":0,"columns":[
                {"name":"alias_hash","type":"text"}]}"#,
        );

        let error = plan_table_import(
            &connection,
            "project_aliases",
            manifest.table("project_aliases").unwrap(),
        )
        .unwrap_err();

        assert!(error.to_string().contains("alias_hash"));
    }
}
