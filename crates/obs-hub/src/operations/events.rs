//! `event.emit` (server.mjs:204-217): upsert the event definition and component, then store
//! the event once per `(event_id, occurred_at)`.

use anyhow::{bail, Result};
use rusqlite::{params, Connection, OptionalExtension};

use super::fields::Fields;
use super::Operation;

const EVENT_CODE_MAX: usize = 160;

// Mental model:
// definition id for the code → component id when named → insert the event (replay-safe)
pub fn emit_event(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    let project_id = data.uuid("projectId")?;

    let definition_id = upsert_event_definition(connection, data)?;
    let component_id = if data.is_truthy("component") {
        Some(upsert_component(connection, data, project_id.as_deref())?)
    } else {
        None
    };

    insert_event(connection, operation, definition_id, component_id)
}

fn upsert_event_definition(connection: &Connection, data: Fields<'_>) -> Result<i64> {
    let code = data.text("code", EVENT_CODE_MAX);
    ensure_valid_event_code(&code)?;

    let id = connection
        .query_row(
            "INSERT INTO event_definitions(code,category,default_severity,description) VALUES(?1,?2,?3,?4)
             ON CONFLICT(code) DO UPDATE SET code=excluded.code RETURNING event_definition_id",
            params![
                code,
                data.integer_or("category", 1)?,
                data.integer_or("severity", 1)?,
                data.text("description", 500),
            ],
            |row| row.get(0),
        )
        .optional()?;
    id.ok_or_else(|| anyhow::anyhow!("event definition upsert returned no id"))
}

fn upsert_component(
    connection: &Connection,
    data: Fields<'_>,
    project_id: Option<&str>,
) -> Result<i64> {
    let id = connection.query_row(
        "INSERT INTO components(project_id,name,component_kind) VALUES(?1,?2,?3)
         ON CONFLICT(project_id,name) DO UPDATE SET name=excluded.name RETURNING component_id",
        params![
            project_id,
            data.text("component", 160),
            data.integer_or("componentKind", 1)?,
        ],
        |row| row.get(0),
    )?;
    Ok(id)
}

fn insert_event(
    connection: &Connection,
    operation: &Operation,
    definition_id: i64,
    component_id: Option<i64>,
) -> Result<()> {
    let data = operation.fields();
    connection.execute(
        "INSERT INTO events(event_id,execution_id,component_id,event_definition_id,sequence_no,severity,outcome,
         trace_id,span_id,parent_span_id,message_summary,attributes,raw_payload,occurred_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14) ON CONFLICT(event_id,occurred_at) DO NOTHING",
        params![
            data.uuid("eventId")?,
            data.uuid("executionId")?,
            component_id,
            definition_id,
            data.integer_or("sequenceNo", 0)?,
            data.integer_or("severity", 1)?,
            data.integer_if_truthy("outcome")?,
            data.hex_bytes("traceId", 16)?,
            data.hex_bytes_if_truthy("spanId", 8)?,
            data.hex_bytes_if_truthy("parentSpanId", 8)?,
            data.text("messageSummary", 500),
            data.json_object_text("attributes"),
            data.json_text_or_empty_object("rawPayload"),
            operation.occurred_at,
        ],
    )?;
    Ok(())
}

/// Postgres CHECK `code ~ '^[a-z0-9][a-z0-9._-]{1,159}$'`.
fn ensure_valid_event_code(code: &str) -> Result<()> {
    if !is_valid_event_code(code) {
        bail!(
            "new row for relation \"event_definitions\" violates check constraint \"event_definitions_code_check\""
        );
    }
    Ok(())
}

#[inline]
fn is_valid_event_code(code: &str) -> bool {
    let mut characters = code.chars();
    let length = code.chars().count();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
        && (2..=EVENT_CODE_MAX).contains(&length)
        && characters.all(is_event_code_character)
}

#[inline]
fn is_event_code_character(character: char) -> bool {
    character.is_ascii_lowercase()
        || character.is_ascii_digit()
        || matches!(character, '.' | '_' | '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_codes_follow_the_postgres_check() {
        assert!(is_valid_event_code("agent.instructions.acknowledged"));
        assert!(is_valid_event_code("ab"));
        assert!(!is_valid_event_code("a"));
        assert!(!is_valid_event_code("Bad.code"));
        assert!(!is_valid_event_code(".lead"));
    }
}
