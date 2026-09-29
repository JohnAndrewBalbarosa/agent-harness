//! Terminal-runtime evidence from the Herdr bridge: pane bindings, agent state transitions,
//! raw runtime events, and pane snapshots (server.mjs:224-239, 269-273).

use anyhow::Result;
use rusqlite::{params, Connection};

use super::Operation;

const NATIVE_ID_MAX: usize = 240;
const PANE_ID_MAX: usize = 120;
const SNAPSHOT_MAX_LINES: i64 = 80;
const SNAPSHOT_MAX_BYTES: i64 = 32768;

pub fn bind_pane(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    connection.execute(
        "INSERT INTO herdr_pane_bindings(binding_id,project_id,native_session,profile_name,machine_label,herdr_session,
         workspace_id,tab_id,pane_id,runtime_scope,cwd,active,updated_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,1,?12)
         ON CONFLICT(native_session,profile_name) DO UPDATE SET workspace_id=excluded.workspace_id,
         tab_id=excluded.tab_id,pane_id=excluded.pane_id,runtime_scope=excluded.runtime_scope,cwd=excluded.cwd,
         active=1,updated_at=excluded.updated_at",
        params![
            data.uuid("bindingId")?,
            data.uuid("projectId")?,
            data.text("nativeSession", NATIVE_ID_MAX),
            data.text("profileName", 32),
            data.text_or("machineLabel", "local", 240),
            data.text_or("herdrSession", "default", 240),
            data.text_or_null("workspaceId", PANE_ID_MAX),
            data.text_or_null("tabId", PANE_ID_MAX),
            data.text_or_null("paneId", PANE_ID_MAX),
            data.text("runtimeScope", 32),
            data.text("cwd", 4000),
            operation.occurred_at,
        ],
    )?;
    Ok(())
}

pub fn record_agent_state(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    connection.execute(
        "INSERT INTO agent_state_events(state_event_id,project_id,native_session,native_turn,profile_name,
         previous_state,canonical_state,projected_state,reason,authority,signal_source,sequence_no,conflict,
         attributes,occurred_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15) ON CONFLICT(state_event_id) DO NOTHING",
        params![
            data.uuid("stateEventId")?,
            data.uuid("projectId")?,
            data.text("nativeSession", NATIVE_ID_MAX),
            data.text_or_null("nativeTurn", NATIVE_ID_MAX),
            data.text("profileName", 32),
            data.text_or_null("previousState", 32),
            data.text("canonicalState", 32),
            data.text("projectedState", 32),
            data.text("reason", 160),
            data.text("authority", 64),
            data.text("signalSource", 120),
            data.integer_or("sequenceNo", 0)?,
            data.is_truthy("conflict"),
            data.json_object_text("attributes"),
            operation.occurred_at,
        ],
    )?;
    Ok(())
}

pub fn record_runtime_event(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    connection.execute(
        "INSERT INTO herdr_runtime_events(herdr_event_id,project_id,native_session,pane_id,event_name,sequence_no,
         payload,occurred_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(herdr_event_id) DO NOTHING",
        params![
            data.uuid("herdrEventId")?,
            data.uuid("projectId")?,
            data.text_or_null("nativeSession", NATIVE_ID_MAX),
            data.text_or_null("paneId", PANE_ID_MAX),
            data.text("eventName", 160),
            data.integer_or("sequenceNo", 0)?,
            data.json_object_text("payload"),
            operation.occurred_at,
        ],
    )?;
    Ok(())
}

/// One snapshot per `(project, native session, content hash)`; sizes are clamped like the Node hub.
pub fn record_pane_evidence(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    connection.execute(
        "INSERT INTO pane_evidence_snapshots(snapshot_id,project_id,native_session,pane_id,trigger_code,
         content_sha256,line_count,byte_count,redacted_content,explain,captured_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
         ON CONFLICT(project_id,native_session,content_sha256) DO NOTHING",
        params![
            data.uuid("snapshotId")?,
            data.uuid("projectId")?,
            data.text("nativeSession", NATIVE_ID_MAX),
            data.text_or_null("paneId", PANE_ID_MAX),
            data.text("triggerCode", 160),
            data.text("contentSha256", 64),
            data.clamped_integer("lineCount", SNAPSHOT_MAX_LINES)?,
            data.clamped_integer("byteCount", SNAPSHOT_MAX_BYTES)?,
            data.text("redactedContent", SNAPSHOT_MAX_BYTES as usize),
            data.json_object_text("explain"),
            operation.occurred_at,
        ],
    )?;
    Ok(())
}
