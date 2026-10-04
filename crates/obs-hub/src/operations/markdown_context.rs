//! Idempotent context batches reference one document and content version per project.
use anyhow::{bail, Result};
use rusqlite::{params, Connection};

use super::{fields::Fields, Operation};

// Mental model: validate bounded metadata → establish batch → upsert document/version → observation.
// The caller's transaction rolls every row back together on any malformed record.
pub fn record(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    let project_id = data.uuid("projectId")?;
    let batch_id = data.uuid("batchId")?;
    if project_id.is_none() || batch_id.is_none() || data.array("documents").len() > 32 {
        bail!("markdown context requires projectId, batchId and at most 32 documents");
    }
    connection.execute(
        "INSERT INTO markdown_context_batches(batch_id,project_id,native_session,native_turn,agent,instance,
         context_epoch,reason,duration_ms,recorded_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT DO NOTHING",
        params![batch_id, project_id, data.text("nativeSession",240), data.text_or_null("nativeTurn",240),
            data.text("agent",80),data.text("instance",80),data.text_or("contextEpoch","0",80),
            data.text("reason",80),data.raw("durationMs").as_f64().unwrap_or(0.0),operation.occurred_at])?;
    // A replay must not graft documents from a different project onto an existing batch identity.
    let owner: String = connection.query_row(
        "SELECT project_id FROM markdown_context_batches WHERE batch_id=?1",
        params![batch_id],
        |row| row.get(0),
    )?;
    if project_id.as_deref() != Some(owner.as_str()) {
        bail!("markdown batch belongs to another project");
    }
    for document in data.array("documents") {
        record_document(
            connection,
            batch_id.as_deref().unwrap(),
            owner.as_str(),
            Fields::of(document),
        )?;
    }
    Ok(())
}

fn record_document(
    connection: &Connection,
    batch_id: &str,
    project_id: &str,
    data: Fields<'_>,
) -> Result<()> {
    let path = data.full_string("path");
    if path.is_empty()
        || path.len() > 2048
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains(':')
        || path.split('/').any(|part| matches!(part, "" | "." | ".."))
    {
        bail!("markdown document path must be project-relative");
    }
    let hash = obs_common::decode_hex_bytes(&data.full_string("hash"), 32)?;
    connection.execute("INSERT INTO markdown_documents(project_id,relative_path) VALUES(?1,?2) ON CONFLICT DO NOTHING",
        params![project_id,path])?;
    let document_id: i64 = connection.query_row(
        "SELECT document_id FROM markdown_documents WHERE project_id=?1 AND relative_path=?2",
        params![project_id, path],
        |row| row.get(0),
    )?;
    connection.execute("INSERT INTO markdown_document_versions(document_id,content_hash,byte_size) VALUES(?1,?2,?3) ON CONFLICT DO NOTHING",
        params![document_id,hash,data.integer("byteSize")?])?;
    let version_id: i64 = connection.query_row("SELECT version_id FROM markdown_document_versions WHERE document_id=?1 AND content_hash=?2",
        params![document_id,hash],|row|row.get(0))?;
    connection.execute("INSERT INTO markdown_context_observations(batch_id,version_id,source,coverage,outcome,
        start_character,end_character,supplied_characters) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT DO NOTHING",
        params![batch_id,version_id,data.text("source",32),data.text("coverage",16),data.text("outcome",32),
            data.raw("startCharacter").as_i64(),data.raw("endCharacter").as_i64(),data.integer("suppliedCharacters")?])?;
    Ok(())
}
