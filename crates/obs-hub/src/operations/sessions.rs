//! `session.start` and `session.end` (server.mjs:161-168).

use anyhow::Result;
use rusqlite::{params, Connection};

use super::Operation;

const SESSION_ACTIVE: i64 = 1;
const SESSION_ENDED: i64 = 2;

pub fn start_session(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    connection.execute(
        "INSERT INTO sessions(session_id,project_id,agent_instance_id,session_kind,session_fingerprint,status,started_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7)
         ON CONFLICT(project_id,agent_instance_id,session_fingerprint) DO UPDATE SET status=?6,ended_at=NULL",
        params![
            data.uuid("sessionId")?,
            data.uuid("projectId")?,
            data.uuid("agentInstanceId")?,
            data.integer_or("sessionKind", 1)?,
            data.hex_bytes("sessionFingerprint", 32)?,
            SESSION_ACTIVE,
            operation.occurred_at,
        ],
    )?;
    Ok(())
}

pub fn end_session(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    connection.execute(
        "UPDATE sessions SET status=?3,ended_at=?2 WHERE session_id=?1",
        params![
            data.uuid("sessionId")?,
            operation.occurred_at,
            SESSION_ENDED
        ],
    )?;
    Ok(())
}
