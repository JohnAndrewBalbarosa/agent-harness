//! Hook-written session context (obs.py:731 `context_for`, project branch): the newest row for
//! the project, preferring rows whose prompt still has a live execution.

use rusqlite::{params, Connection, OptionalExtension};

use crate::failure::CliResult;

/// The `prompt_id` of the project's current context row (`None` when no row or NULL).
pub fn active_prompt_id(conn: &Connection, project_id: &str) -> CliResult<Option<String>> {
    let prompt_id = conn
        .query_row(
            "SELECT prompt_id FROM context WHERE project_id=? ORDER BY (execution_id IS NOT NULL) DESC,updated_at DESC LIMIT 1",
            params![project_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?;
    Ok(prompt_id.flatten())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spool::open::prepare_spool_connection;

    #[test]
    fn prefers_live_execution_within_the_project() {
        let conn = Connection::open_in_memory().unwrap();
        prepare_spool_connection(&conn).unwrap();
        let insert = "INSERT INTO context(native_session,project_id,session_id,prompt_id,execution_id,cwd,updated_at) VALUES(?,?,?,?,?,?,?)";
        conn.execute(
            insert,
            params![
                "one",
                "p1",
                "s1",
                "prompt-live",
                "e1",
                "/first",
                "2026-01-01T00:00:00Z"
            ],
        )
        .unwrap();
        conn.execute(
            insert,
            params![
                "one-ended",
                "p1",
                "s2",
                None::<String>,
                None::<String>,
                "/first",
                "2026-01-03T00:00:00Z"
            ],
        )
        .unwrap();
        conn.execute(
            insert,
            params![
                "two",
                "p2",
                "s3",
                "prompt-other",
                "e2",
                "/second",
                "2026-01-04T00:00:00Z"
            ],
        )
        .unwrap();

        assert_eq!(
            active_prompt_id(&conn, "p1").unwrap(),
            Some("prompt-live".to_owned())
        );
        assert_eq!(active_prompt_id(&conn, "missing").unwrap(), None);
    }
}
