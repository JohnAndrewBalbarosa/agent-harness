//! Safety checks around the import: refuse a non-empty target, prove FK integrity before
//! commit, and count rows for comparison with the manifest.

use anyhow::{bail, Context, Result};
use rusqlite::Connection;

const MAX_REPORTED_FK_VIOLATIONS: usize = 5;

#[derive(Debug)]
struct ForeignKeyViolation {
    table: String,
    rowid: Option<i64>,
    parent: String,
}

/// The import only ever creates data; it never merges into an existing hub database.
pub fn ensure_target_is_empty(connection: &Connection, tables: &[&str]) -> Result<()> {
    let populated: Vec<String> = tables
        .iter()
        .map(|table| count_rows(connection, table).map(|count| (*table, count)))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .filter(|(_, count)| *count > 0)
        .map(|(table, count)| format!("{table}={count}"))
        .collect();

    if !populated.is_empty() {
        bail!(
            "refusing to import: target database already has rows ({}); use a new database file",
            populated.join(", ")
        );
    }
    Ok(())
}

pub fn count_rows(connection: &Connection, table: &str) -> Result<u64> {
    let sql = format!("SELECT count(*) FROM \"{}\"", table.replace('"', "\"\""));
    let count: i64 = connection
        .query_row(&sql, [], |row| row.get(0))
        .with_context(|| format!("count rows of {table}"))?;
    u64::try_from(count).with_context(|| format!("negative row count for {table}"))
}

/// Runs `PRAGMA foreign_key_check` over the whole database (deferred checks would otherwise
/// only fail at COMMIT without naming the offending row).
pub fn ensure_foreign_keys_hold(connection: &Connection) -> Result<()> {
    let violations = read_foreign_key_violations(connection)?;
    if violations.is_empty() {
        return Ok(());
    }

    let described = violations
        .iter()
        .take(MAX_REPORTED_FK_VIOLATIONS)
        .map(describe_violation)
        .collect::<Vec<_>>()
        .join("; ");
    bail!(
        "foreign key check failed ({} violation(s)): {described}",
        violations.len()
    )
}

fn read_foreign_key_violations(connection: &Connection) -> Result<Vec<ForeignKeyViolation>> {
    let mut statement = connection
        .prepare("PRAGMA foreign_key_check")
        .context("prepare foreign_key_check")?;
    let violations = statement
        .query_map([], |row| {
            Ok(ForeignKeyViolation {
                table: row.get(0)?,
                rowid: row.get(1)?,
                parent: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("read foreign_key_check results")?;
    Ok(violations)
}

#[inline]
fn describe_violation(violation: &ForeignKeyViolation) -> String {
    match violation.rowid {
        Some(rowid) => format!(
            "{} rowid {} -> missing {}",
            violation.table, rowid, violation.parent
        ),
        None => format!(
            "{} (WITHOUT ROWID) -> missing {}",
            violation.table, violation.parent
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema_connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(include_str!("../store/schema.sql"))
            .unwrap();
        connection
    }

    #[test]
    fn empty_schema_passes_and_populated_table_is_refused() {
        let connection = schema_connection();
        assert!(ensure_target_is_empty(&connection, &["projects"]).is_ok());

        connection
            .execute(
                "INSERT INTO projects(project_id, display_name, slug) VALUES ('p', 'demo', 'demo')",
                [],
            )
            .unwrap();

        let error = ensure_target_is_empty(&connection, &["projects"]).unwrap_err();
        assert!(error.to_string().contains("projects=1"));
    }

    #[test]
    fn dangling_reference_is_reported_by_table() {
        let connection = schema_connection();
        // Bundled SQLite enforces FKs by default; switch them off to plant the dangling row.
        connection
            .pragma_update(None, "foreign_keys", false)
            .unwrap();
        connection
            .execute(
                "INSERT INTO project_aliases(project_id, alias_kind, alias_hash) VALUES ('missing', 1, zeroblob(32))",
                [],
            )
            .unwrap();

        let error = ensure_foreign_keys_hold(&connection).unwrap_err();
        assert!(error.to_string().contains("project_aliases"));
    }
}
