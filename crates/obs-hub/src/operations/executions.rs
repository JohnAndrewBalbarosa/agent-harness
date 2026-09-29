//! Execution lifecycle: `execution.start`, `execution.finish`, `execution.code-version`
//! (server.mjs:191-203).

use anyhow::Result;
use rusqlite::{params, Connection};

use super::Operation;

const EXECUTION_RUNNING: i64 = 1;
const EXECUTION_COMPLETED: i64 = 2;
const OUTCOME_SUCCESS: i64 = 1;

pub fn start_execution(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    let execution_id = data.uuid("executionId")?;

    connection.execute(
        "INSERT INTO executions(execution_id,project_id,session_id,prompt_id,parent_execution_id,execution_kind,
         attempt_no,trace_id,status,started_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT(execution_id) DO NOTHING",
        params![
            execution_id,
            data.uuid("projectId")?,
            data.uuid_if_truthy("sessionId")?,
            data.uuid_if_truthy("promptId")?,
            data.uuid_if_truthy("parentExecutionId")?,
            data.integer_or("executionKind", 1)?,
            data.integer_or("attemptNo", 1)?,
            data.hex_bytes("traceId", 16)?,
            EXECUTION_RUNNING,
            operation.occurred_at,
        ],
    )?;

    if data.is_truthy("codeVersionId") {
        set_code_version(
            connection,
            execution_id.as_deref(),
            data.uuid("codeVersionId")?.as_deref(),
        )?;
    }
    Ok(())
}

/// Duration is `GREATEST(0, ended_at - started_at)` in whole milliseconds.
pub fn finish_execution(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    connection.execute(
        "UPDATE executions SET status=?2,outcome=?3,ended_at=?4,
         duration_ms=MAX(0,CAST(ROUND((julianday(?4)-julianday(started_at))*86400000.0) AS INTEGER)),
         result_summary=?5 WHERE execution_id=?1",
        params![
            data.uuid("executionId")?,
            data.integer_or("status", EXECUTION_COMPLETED)?,
            data.integer_or("outcome", OUTCOME_SUCCESS)?,
            operation.occurred_at,
            data.text("resultSummary", 1200),
        ],
    )?;
    Ok(())
}

pub fn attach_code_version(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    set_code_version(
        connection,
        data.uuid("executionId")?.as_deref(),
        data.uuid("codeVersionId")?.as_deref(),
    )
}

fn set_code_version(
    connection: &Connection,
    execution_id: Option<&str>,
    code_version_id: Option<&str>,
) -> Result<()> {
    connection.execute(
        "UPDATE executions SET code_version_id=?2 WHERE execution_id=?1",
        params![execution_id, code_version_id],
    )?;
    Ok(())
}
