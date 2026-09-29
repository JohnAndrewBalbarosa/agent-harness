//! `error.record` (server.mjs:361-374): group occurrences by `(project, fingerprint)`,
//! count repeats, and link each occurrence to its event.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

use super::Operation;

// Mental model:
// find the group for this fingerprint → bump it or create it → attach the occurrence (replay-safe)
pub fn record_error(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    let project_id = data.uuid("projectId")?;
    let fingerprint = data.hex_bytes("fingerprint", 32)?;

    let existing_group = find_error_group(connection, project_id.as_deref(), &fingerprint)?;
    let group_id = match existing_group {
        Some(group_id) => {
            count_repeat_occurrence(connection, &group_id, &operation.occurred_at)?;
            Some(group_id)
        }
        None => {
            let group_id = data.uuid("errorGroupId")?;
            create_error_group(
                connection,
                operation,
                group_id.as_deref(),
                project_id.as_deref(),
                &fingerprint,
            )?;
            group_id
        }
    };

    attach_occurrence(connection, operation, group_id.as_deref())
}

fn find_error_group(
    connection: &Connection,
    project_id: Option<&str>,
    fingerprint: &[u8],
) -> Result<Option<String>> {
    Ok(connection
        .query_row(
            "SELECT error_group_id FROM error_groups WHERE project_id=?1 AND fingerprint=?2",
            params![project_id, fingerprint],
            |row| row.get(0),
        )
        .optional()?)
}

fn count_repeat_occurrence(connection: &Connection, group_id: &str, seen_at: &str) -> Result<()> {
    connection.execute(
        "UPDATE error_groups SET last_seen_at=?2,occurrence_count=occurrence_count+1 WHERE error_group_id=?1",
        params![group_id, seen_at],
    )?;
    Ok(())
}

fn create_error_group(
    connection: &Connection,
    operation: &Operation,
    group_id: Option<&str>,
    project_id: Option<&str>,
    fingerprint: &[u8],
) -> Result<()> {
    let data = operation.fields();
    connection.execute(
        "INSERT INTO error_groups(error_group_id,project_id,fingerprint,exception_class,normalized_summary,
         first_seen_at,last_seen_at) VALUES(?1,?2,?3,?4,?5,?6,?6)",
        params![
            group_id,
            project_id,
            fingerprint,
            data.text("exceptionClass", 240),
            data.text("normalizedSummary", 500),
            operation.occurred_at,
        ],
    )?;
    Ok(())
}

fn attach_occurrence(
    connection: &Connection,
    operation: &Operation,
    group_id: Option<&str>,
) -> Result<()> {
    let data = operation.fields();
    connection.execute(
        "INSERT INTO error_occurrences(event_id,event_occurred_at,error_group_id,handled,retryable,stack_hash,top_frame)
         VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(event_id,event_occurred_at) DO NOTHING",
        params![
            data.uuid("eventId")?,
            data.timestamp("eventOccurredAt")?,
            group_id,
            data.is_truthy("handled"),
            data.is_truthy("retryable"),
            data.hex_bytes_if_truthy("stackHash", 32)?,
            data.text("topFrame", 500),
        ],
    )?;
    Ok(())
}
