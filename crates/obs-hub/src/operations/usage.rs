//! Cost and tool activity: `turn-usage.record`, `tool-use.record` (server.mjs:240-255).

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

use super::fields::Fields;
use super::Operation;

const NATIVE_ID_MAX: usize = 240;

// Mental model:
// resolve the prompt that owns this native turn (if already linked) → store usage once per source event
pub fn record_turn_usage(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    let project_id = data.uuid("projectId")?;
    let prompt_id = if data.is_truthy("nativeTurn") {
        linked_prompt(connection, data, project_id.as_deref())?
    } else {
        None
    };

    connection.execute(
        "INSERT INTO turn_usage(usage_id,project_id,native_session,native_turn,prompt_id,input_tokens,output_tokens,
         cached_input_tokens,reasoning_tokens,availability,source,source_event_id,recorded_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13) ON CONFLICT DO NOTHING",
        params![
            data.uuid("usageId")?,
            project_id,
            data.text("nativeSession", NATIVE_ID_MAX),
            data.text_or_null("nativeTurn", NATIVE_ID_MAX),
            prompt_id,
            data.integer("inputTokens")?,
            data.integer("outputTokens")?,
            data.integer("cachedInputTokens")?,
            data.integer("reasoningTokens")?,
            data.text("availability", 32),
            data.text("source", 120),
            data.text_or_null("sourceEventId", NATIVE_ID_MAX),
            operation.occurred_at,
        ],
    )?;
    Ok(())
}

pub fn record_tool_use(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    connection.execute(
        "INSERT INTO tool_uses(tool_record_id,project_id,prompt_id,execution_id,native_session,native_turn,tool_use_id,
         tool_name,origin,parent_tool_use_id,started_at,completed_at,duration_ms,failed,cache_hit,cache_status)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16) ON CONFLICT DO NOTHING",
        params![
            data.uuid("toolRecordId")?,
            data.uuid("projectId")?,
            data.uuid_if_truthy("promptId")?,
            data.uuid_if_truthy("executionId")?,
            data.text("nativeSession", NATIVE_ID_MAX),
            data.text_if_truthy("nativeTurn", NATIVE_ID_MAX),
            data.text_if_truthy("toolUseId", NATIVE_ID_MAX),
            data.text("toolName", 160),
            data.text_or("origin", "unknown", 24),
            data.text_if_truthy("parentToolUseId", NATIVE_ID_MAX),
            data.timestamp_if_truthy("startedAt")?,
            data.timestamp_or("completedAt", &operation.occurred_at)?,
            data.integer("durationMs")?,
            data.nullable_boolean("failed")?,
            data.nullable_boolean("cacheHit")?,
            data.text_or("cacheStatus", "unknown", 16),
        ],
    )?;
    Ok(())
}

fn linked_prompt(
    connection: &Connection,
    data: Fields<'_>,
    project_id: Option<&str>,
) -> Result<Option<String>> {
    Ok(connection
        .query_row(
            "SELECT prompt_id FROM prompt_native_turns WHERE project_id=?1 AND native_session=?2 AND native_turn=?3",
            params![
                project_id,
                data.text("nativeSession", NATIVE_ID_MAX),
                data.text("nativeTurn", NATIVE_ID_MAX),
            ],
            |row| row.get(0),
        )
        .optional()?)
}
