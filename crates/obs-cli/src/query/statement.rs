//! Runs exactly one read-only statement and collects at most `limit` rows.
//! Safety comes from the connection (read-only file + `query_only`) plus a SQLite authorizer
//! that denies what "read-only" still allows: ATTACH/DETACH (reading other SQLite files such
//! as the spool with raw prompts), PRAGMA (e.g. `query_only=OFF`), and `load_extension`.
//! Note: `--project` TEMP views are a convenience, not an isolation boundary — `main.<table>`
//! still reads every project.

use fallible_iterator::FallibleIterator;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::types::ValueRef;
use rusqlite::{Batch, Connection, Row, Statement};

use crate::failure::{CliFailure, CliResult};

#[derive(Debug, Clone, PartialEq)]
pub enum CellValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

pub struct QueryRows {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<CellValue>>,
    pub is_truncated: bool,
}

/// Mental model: refuse configuration changes → compile exactly one statement → require it to
/// be read-only → step until `limit` rows (one extra step detects truncation).
pub fn run_read_only_statement(conn: &Connection, sql: &str, limit: usize) -> CliResult<QueryRows> {
    conn.authorizer(Some(authorize_read_only_action));
    let outcome = compile_and_collect(conn, sql, limit);
    conn.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    outcome
}

fn compile_and_collect(conn: &Connection, sql: &str, limit: usize) -> CliResult<QueryRows> {
    let statement = single_statement(conn, sql)?;
    if !statement.readonly() {
        return Err(CliFailure::refusal(
            "Only read-only statements are allowed.",
        ));
    }

    collect_rows(statement, limit)
}

/// Denies actions that `Statement::readonly()` still accepts but that escape the hub database.
fn authorize_read_only_action(context: AuthContext<'_>) -> Authorization {
    match context.action {
        AuthAction::Attach { .. } | AuthAction::Detach { .. } | AuthAction::Pragma { .. } => {
            Authorization::Deny
        }
        AuthAction::Function { function_name } if is_extension_loader(function_name) => {
            Authorization::Deny
        }
        _ => Authorization::Allow,
    }
}

#[inline]
fn is_extension_loader(function_name: &str) -> bool {
    function_name.eq_ignore_ascii_case("load_extension")
}

fn single_statement<'conn>(conn: &'conn Connection, sql: &str) -> CliResult<Statement<'conn>> {
    let mut batch = Batch::new(conn, sql);
    let first = batch
        .next()
        .map_err(sql_refusal)?
        .ok_or_else(|| CliFailure::refusal("SQL statement is empty."))?;

    // Anything compilable (or not) after the first statement means more than one statement.
    if !matches!(batch.next(), Ok(None)) {
        return Err(CliFailure::refusal(
            "Only a single SQL statement is allowed.",
        ));
    }
    Ok(first)
}

fn collect_rows(mut statement: Statement<'_>, limit: usize) -> CliResult<QueryRows> {
    let columns: Vec<String> = statement
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect();
    let mut collected = Vec::new();
    let mut is_truncated = false;

    let mut rows = statement.query([]).map_err(sql_refusal)?;
    while let Some(row) = rows.next().map_err(sql_refusal)? {
        if collected.len() == limit {
            is_truncated = true;
            break;
        }
        collected.push(read_cells(row, columns.len())?);
    }

    Ok(QueryRows {
        columns,
        rows: collected,
        is_truncated,
    })
}

fn read_cells(row: &Row<'_>, column_count: usize) -> CliResult<Vec<CellValue>> {
    (0..column_count)
        .map(|index| row.get_ref(index).map(cell_value).map_err(sql_refusal))
        .collect()
}

fn cell_value(value: ValueRef<'_>) -> CellValue {
    match value {
        ValueRef::Null => CellValue::Null,
        ValueRef::Integer(number) => CellValue::Integer(number),
        ValueRef::Real(number) => CellValue::Real(number),
        ValueRef::Text(bytes) => CellValue::Text(String::from_utf8_lossy(bytes).into_owned()),
        ValueRef::Blob(bytes) => CellValue::Blob(bytes.to_vec()),
    }
}

#[inline]
fn sql_refusal(error: rusqlite::Error) -> CliFailure {
    CliFailure::refusal(format!("SQL error: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE t(id INTEGER, name TEXT, payload BLOB, score REAL);
             INSERT INTO t VALUES(1,'a',x'00ff',1.5),(2,NULL,NULL,NULL),(3,'c',NULL,2.0);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn collects_typed_cells_and_marks_truncation() {
        let conn = database();
        let result = run_read_only_statement(&conn, "SELECT * FROM t ORDER BY id", 2).unwrap();

        assert_eq!(result.columns, vec!["id", "name", "payload", "score"]);
        assert_eq!(result.rows.len(), 2);
        assert!(result.is_truncated);
        assert_eq!(result.rows[0][2], CellValue::Blob(vec![0x00, 0xff]));
        assert_eq!(result.rows[1][1], CellValue::Null);
    }

    #[test]
    fn rejects_writes_multiple_statements_and_empty_sql() {
        let conn = database();

        for sql in [
            "DELETE FROM t",
            "SELECT 1; SELECT 2",
            "   ",
            "PRAGMA query_only=OFF",
            "ATTACH 'other.sqlite3' AS other",
            "SELECT load_extension('x')",
        ] {
            assert!(
                matches!(
                    run_read_only_statement(&conn, sql, 10),
                    Err(CliFailure::Refusal(_))
                ),
                "{sql}"
            );
        }
    }

    #[test]
    fn column_named_like_a_pragma_is_allowed() {
        let conn = database();
        let result = run_read_only_statement(&conn, "SELECT 1 AS query_only", 10).unwrap();
        assert_eq!(result.rows[0][0], CellValue::Integer(1));
    }

    #[test]
    fn trailing_semicolon_and_comment_are_one_statement() {
        let conn = database();
        let result = run_read_only_statement(&conn, "SELECT count(*) FROM t; -- done", 10).unwrap();
        assert_eq!(result.rows[0][0], CellValue::Integer(3));
    }
}
