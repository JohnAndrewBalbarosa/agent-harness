//! Prompt lifecycle: `prompt.begin`, `prompt.enrich`, `prompt.finish`, `prompt.native-link`
//! (server.mjs:169-190).

use anyhow::Result;
use rusqlite::{params, Connection};

use super::Operation;

const DEFAULT_LANGUAGE: &str = "tl-en";
const PROMPT_COMPLETED: i64 = 2;
const NATIVE_ID_MAX: usize = 240;

pub fn begin_prompt(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    connection.execute(
        "INSERT INTO prompt_nodes(prompt_id,session_id,parent_prompt_id,ordinal,prompt_hmac,raw_prompt,language_code,created_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
         ON CONFLICT(prompt_id) DO UPDATE SET raw_prompt=excluded.raw_prompt",
        params![
            data.uuid("promptId")?,
            data.uuid("sessionId")?,
            data.uuid_if_truthy("parentPromptId")?,
            data.integer("ordinal")?,
            data.hex_bytes("promptHmac", 32)?,
            data.full_string("rawPrompt"),
            data.text_or("languageCode", DEFAULT_LANGUAGE, 16),
            operation.occurred_at,
        ],
    )?;
    Ok(())
}

pub fn enrich_prompt(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    connection.execute(
        "UPDATE prompt_nodes SET summary=?2,intent=?3,target_system=?4,constraints=?5,success_criteria=?6,
         result_summary=?7,language_code=?8,enriched_at=?9 WHERE prompt_id=?1",
        params![
            data.uuid("promptId")?,
            data.text("summary", 1200),
            data.text("intent", 1200),
            data.text("targetSystem", 400),
            data.json_array_text("constraints"),
            data.json_array_text("successCriteria"),
            data.text("resultSummary", 1200),
            data.text_or("languageCode", DEFAULT_LANGUAGE, 16),
            operation.occurred_at,
        ],
    )?;
    Ok(())
}

/// An empty result summary keeps the previous one (`CASE WHEN $3='' THEN result_summary ELSE $3 END`).
pub fn finish_prompt(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    connection.execute(
        "UPDATE prompt_nodes SET status=?2,result_summary=CASE WHEN ?3='' THEN result_summary ELSE ?3 END,
         raw_result=?4,completed_at=?5 WHERE prompt_id=?1",
        params![
            data.uuid("promptId")?,
            data.integer_or("status", PROMPT_COMPLETED)?,
            data.text("resultSummary", 1200),
            data.full_string("rawResult"),
            operation.occurred_at,
        ],
    )?;
    Ok(())
}

// Mental model:
// remember which prompt owns (native session, native turn) → backfill usage rows recorded earlier
pub fn link_native_turn(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    if !(data.is_truthy("nativeSession") && data.is_truthy("nativeTurn")) {
        return Ok(());
    }

    let project_id = data.uuid("projectId")?;
    let native_session = data.text("nativeSession", NATIVE_ID_MAX);
    let native_turn = data.text("nativeTurn", NATIVE_ID_MAX);
    let prompt_id = data.uuid("promptId")?;

    connection.execute(
        "INSERT INTO prompt_native_turns(project_id,native_session,native_turn,prompt_id) VALUES(?1,?2,?3,?4)
         ON CONFLICT(project_id,native_session,native_turn) DO NOTHING",
        params![project_id, native_session, native_turn, prompt_id],
    )?;
    connection.execute(
        "UPDATE turn_usage SET prompt_id=?4
         WHERE project_id=?1 AND native_session=?2 AND native_turn=?3 AND prompt_id IS NULL",
        params![project_id, native_session, native_turn, prompt_id],
    )?;
    Ok(())
}
