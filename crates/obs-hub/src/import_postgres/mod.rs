//! One-time migration of the legacy observability Postgres into the SQLite hub.
//!
//! Input: a dump folder written by `scripts/obs-export-postgres.{ps1,sh}` — one
//! `<table>.jsonl` per table (`row_to_json` lines) plus `manifest.json` (row counts and
//! Postgres column types captured in the same REPEATABLE READ snapshot as the rows).
//!
//! Folder map:
//! - `manifest.rs`          reads `manifest.json`
//! - `table_order.rs`       which tables are imported, in FK-safe order
//! - `column_conversion.rs` per-column Postgres JSON value → SQLite value rules
//! - `import_plan.rs`       joins manifest columns with the SQLite target columns
//! - `row_insertion.rs`     streams one `.jsonl` file into its table
//! - `verification.rs`      empty-target guard, FK check, row-count comparison

mod column_conversion;
mod import_plan;
mod manifest;
mod row_insertion;
mod table_order;
mod verification;

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use rusqlite::Transaction;
use serde::Serialize;

use crate::store::open_database;
use import_plan::plan_table_import;
use manifest::{load_manifest, ExportManifest};
use row_insertion::insert_table_rows;
use table_order::IMPORT_ORDER;
use verification::{count_rows, ensure_foreign_keys_hold, ensure_target_is_empty};

/// Outcome of one import run; printed as JSON by `obs-hub import-postgres`.
#[derive(Debug, Serialize)]
pub struct ImportReport {
    /// True only when every table's SQLite count equals the manifest count and the data was committed.
    pub ok: bool,
    pub committed: bool,
    pub dump_dir: String,
    pub database_path: String,
    pub manifest_format: Option<String>,
    pub manifest_exported_at: Option<String>,
    pub manifest_snapshot: Option<String>,
    pub tables: Vec<TableReport>,
    pub mismatches: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct TableReport {
    pub table: String,
    /// Row count recorded in `manifest.json`.
    pub expected: u64,
    /// Rows inserted from the `.jsonl` file.
    pub imported: u64,
    /// `SELECT count(*)` in SQLite after insertion (inside the import transaction).
    pub target_count: u64,
    pub skipped: u64,
    pub skip_reasons: BTreeMap<String, u64>,
    /// Postgres columns with no SQLite counterpart (values not imported).
    pub dropped_columns: Vec<String>,
    /// SQLite columns absent from the dump (filled by the SQLite default).
    pub defaulted_columns: Vec<String>,
    pub count_matches: bool,
}

// Mental model:
// manifest → open target → refuse non-empty target → ONE transaction with deferred FK checks
// → per table (FK-safe order): plan columns → stream rows → count → FK check over everything
// → commit only when every count matches the manifest, otherwise roll back and report.
pub fn run(dump_dir: &Path, database_path: &Path) -> Result<ImportReport> {
    let is_new_target = !database_path.exists();
    let outcome = import_into(dump_dir, database_path);

    if is_new_target && !matches!(&outcome, Ok(report) if report.committed) {
        remove_created_database(database_path);
    }
    outcome
}

/// Deletes a database file (and its WAL/SHM siblings) that this failed run created.
fn remove_created_database(database_path: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let mut path = database_path.as_os_str().to_owned();
        path.push(suffix);
        let _ = std::fs::remove_file(path);
    }
}

fn import_into(dump_dir: &Path, database_path: &Path) -> Result<ImportReport> {
    let manifest = load_manifest(dump_dir)?;
    let mut connection = open_database(database_path)
        .with_context(|| format!("open SQLite target {}", database_path.display()))?;
    ensure_target_is_empty(&connection, &IMPORT_ORDER)?;

    let transaction = connection
        .transaction()
        .context("begin import transaction")?;
    defer_foreign_key_checks(&transaction)?;

    let tables = import_all_tables(&transaction, &manifest, dump_dir)?;
    ensure_foreign_keys_hold(&transaction)?;

    let mismatches = describe_count_mismatches(&tables);
    let committed = mismatches.is_empty();
    finish_transaction(transaction, committed)?;

    Ok(ImportReport {
        ok: committed,
        committed,
        dump_dir: dump_dir.display().to_string(),
        database_path: database_path.display().to_string(),
        manifest_format: manifest.format.clone(),
        manifest_exported_at: manifest.exported_at.clone(),
        manifest_snapshot: manifest.snapshot.clone(),
        tables,
        mismatches,
    })
}

fn import_all_tables(
    transaction: &Transaction<'_>,
    manifest: &ExportManifest,
    dump_dir: &Path,
) -> Result<Vec<TableReport>> {
    IMPORT_ORDER
        .iter()
        .map(|&table| import_one_table(transaction, manifest, dump_dir, table))
        .collect()
}

fn import_one_table(
    transaction: &Transaction<'_>,
    manifest: &ExportManifest,
    dump_dir: &Path,
    table: &'static str,
) -> Result<TableReport> {
    let manifest_table = manifest.table(table).ok_or_else(|| {
        anyhow!("manifest.json has no entry for table {table}; dump is incomplete")
    })?;
    let plan = plan_table_import(transaction, table, manifest_table)?;

    let source_path = dump_dir.join(format!("{table}.jsonl"));
    let outcome = insert_table_rows(transaction, &plan, &source_path)?;
    let target_count = count_rows(transaction, table)?;

    Ok(TableReport {
        table: table.to_owned(),
        expected: manifest_table.row_count,
        imported: outcome.imported,
        target_count,
        skipped: outcome.skipped(),
        skip_reasons: outcome.skip_reasons,
        dropped_columns: plan.dropped_columns,
        defaulted_columns: plan.defaulted_columns,
        count_matches: counts_agree(manifest_table.row_count, outcome.imported, target_count),
    })
}

/// Self-references (prompt_nodes.parent_prompt_id, executions.parent_execution_id) and any
/// residual ordering gaps are then checked once, at `ensure_foreign_keys_hold`.
fn defer_foreign_key_checks(transaction: &Transaction<'_>) -> Result<()> {
    transaction
        .execute_batch("PRAGMA defer_foreign_keys = ON;")
        .context("enable deferred foreign key checks")
}

fn finish_transaction(transaction: Transaction<'_>, should_commit: bool) -> Result<()> {
    if should_commit {
        transaction.commit().context("commit import transaction")
    } else {
        transaction
            .rollback()
            .context("roll back import transaction")
    }
}

fn describe_count_mismatches(tables: &[TableReport]) -> Vec<String> {
    tables
        .iter()
        .filter(|table| !table.count_matches)
        .map(|table| {
            format!(
                "{}: manifest={} imported={} sqlite_count={} skipped={}",
                table.table, table.expected, table.imported, table.target_count, table.skipped
            )
        })
        .collect()
}

#[inline]
fn counts_agree(expected: u64, imported: u64, target_count: u64) -> bool {
    expected == imported && imported == target_count
}
