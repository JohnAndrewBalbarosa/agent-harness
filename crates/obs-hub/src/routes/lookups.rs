//! Single-entity lookups used by `obs diagnose|prompt show|trace` (server.mjs:487-505, 532-537).

use axum::extract::{Path, State};
use axum::Json;
use rusqlite::params;
use serde_json::{json, Value};

use super::path_params::uuid_parameter;
use super::wire::{fetch_all, fetch_optional, Columns, Wire};
use super::SharedDatabase;
use crate::http_errors::HubError;

/// `g.*` with the hex `fingerprint` alias replacing the bytea column in place, then `project_name`.
const ERROR_GROUP_COLUMNS: Columns = &[
    ("error_group_id", Wire::Text),
    ("project_id", Wire::Text),
    ("fingerprint", Wire::Text),
    ("exception_class", Wire::Text),
    ("normalized_summary", Wire::Text),
    ("first_seen_at", Wire::Timestamp),
    ("last_seen_at", Wire::Timestamp),
    ("occurrence_count", Wire::BigIntText),
    ("status", Wire::Integer),
    ("project_name", Wire::Text),
];

const OCCURRENCE_COLUMNS: Columns = &[
    ("event_id", Wire::Text),
    ("event_occurred_at", Wire::Timestamp),
    ("handled", Wire::Bool),
    ("retryable", Wire::Bool),
    ("top_frame", Wire::Text),
    ("message_summary", Wire::Text),
    ("attributes", Wire::Json),
    ("raw_payload", Wire::Json),
    ("execution_id", Wire::Text),
    ("trace_id", Wire::Text),
    ("code_version_id", Wire::Text),
    ("commit_sha", Wire::Text),
    ("branch_name", Wire::Text),
    ("code_version_dirty", Wire::Bool),
    ("prompt_id", Wire::Text),
    ("prompt_summary", Wire::Text),
    ("prompt_intent", Wire::Text),
    ("target_system", Wire::Text),
];

pub async fn diagnose_error_group(
    State(database): State<SharedDatabase>,
    Path(error_group_id): Path<String>,
) -> Result<Json<Value>, HubError> {
    let error_group_id = uuid_parameter(&error_group_id)?;
    let found = database
        .read(move |connection| {
            let Some(group) = fetch_optional(
                connection,
                "SELECT g.error_group_id,g.project_id,lower(hex(g.fingerprint)),g.exception_class,g.normalized_summary,
                 g.first_seen_at,g.last_seen_at,g.occurrence_count,g.status,p.display_name
                 FROM error_groups g JOIN projects p USING(project_id) WHERE g.error_group_id=?1",
                params![error_group_id],
                ERROR_GROUP_COLUMNS,
            )?
            else {
                return Ok(None);
            };
            let occurrences = fetch_all(
                connection,
                "SELECT o.event_id,o.event_occurred_at,o.handled,o.retryable,o.top_frame,e.message_summary,e.attributes,
                 e.raw_payload,x.execution_id,lower(hex(x.trace_id)),x.code_version_id,v.commit_sha,v.branch_name,
                 (v.dirty_fingerprint IS NOT NULL),p.prompt_id,p.summary,p.intent,p.target_system
                 FROM error_occurrences o
                 JOIN events e ON e.event_id=o.event_id AND e.occurred_at=o.event_occurred_at
                 JOIN executions x ON x.execution_id=e.execution_id
                 LEFT JOIN code_versions v ON v.code_version_id=x.code_version_id
                 LEFT JOIN prompt_nodes p ON p.prompt_id=x.prompt_id
                 WHERE o.error_group_id=?1 ORDER BY o.event_occurred_at DESC LIMIT 200",
                params![error_group_id],
                OCCURRENCE_COLUMNS,
            )?;
            Ok(Some((group, occurrences)))
        })
        .await?;

    let (group, occurrences) = found.ok_or_else(|| HubError::not_found("error group not found"))?;
    Ok(Json(
        json!({ "errorGroup": group, "occurrences": occurrences }),
    ))
}

/// `p.*` of prompt_nodes (bytea `prompt_hmac` as Buffer JSON) plus `project_id` and `project_name`.
const PROMPT_COLUMNS: Columns = &[
    ("prompt_id", Wire::Text),
    ("session_id", Wire::Text),
    ("parent_prompt_id", Wire::Text),
    ("ordinal", Wire::Integer),
    ("prompt_hmac", Wire::Buffer),
    ("language_code", Wire::Text),
    ("summary", Wire::Text),
    ("intent", Wire::Text),
    ("target_system", Wire::Text),
    ("constraints", Wire::Json),
    ("success_criteria", Wire::Json),
    ("result_summary", Wire::Text),
    ("status", Wire::Integer),
    ("created_at", Wire::Timestamp),
    ("enriched_at", Wire::Timestamp),
    ("completed_at", Wire::Timestamp),
    ("raw_prompt", Wire::Text),
    ("raw_result", Wire::Text),
    ("project_id", Wire::Text),
    ("project_name", Wire::Text),
];

const PROMPT_EXECUTION_COLUMNS: Columns = &[
    ("execution_id", Wire::Text),
    ("status", Wire::Integer),
    ("outcome", Wire::Integer),
    ("started_at", Wire::Timestamp),
    ("ended_at", Wire::Timestamp),
    ("result_summary", Wire::Text),
    ("trace_id", Wire::Text),
];

pub async fn show_prompt(
    State(database): State<SharedDatabase>,
    Path(prompt_id): Path<String>,
) -> Result<Json<Value>, HubError> {
    let prompt_id = uuid_parameter(&prompt_id)?;
    let found = database
        .read(move |connection| {
            let Some(prompt) = fetch_optional(
                connection,
                "SELECT p.prompt_id,p.session_id,p.parent_prompt_id,p.ordinal,p.prompt_hmac,p.language_code,p.summary,
                 p.intent,p.target_system,p.constraints,p.success_criteria,p.result_summary,p.status,p.created_at,
                 p.enriched_at,p.completed_at,p.raw_prompt,p.raw_result,s.project_id,pr.display_name
                 FROM prompt_nodes p JOIN sessions s ON s.session_id=p.session_id
                 JOIN projects pr ON pr.project_id=s.project_id WHERE p.prompt_id=?1",
                params![prompt_id],
                PROMPT_COLUMNS,
            )?
            else {
                return Ok(None);
            };
            let executions = fetch_all(
                connection,
                "SELECT execution_id,status,outcome,started_at,ended_at,result_summary,lower(hex(trace_id))
                 FROM executions WHERE prompt_id=?1 ORDER BY started_at",
                params![prompt_id],
                PROMPT_EXECUTION_COLUMNS,
            )?;
            Ok(Some((prompt, executions)))
        })
        .await?;

    let (prompt, executions) = found.ok_or_else(|| HubError::not_found("prompt not found"))?;
    Ok(Json(json!({ "prompt": prompt, "executions": executions })))
}

const TRACE_EVENT_COLUMNS: Columns = &[
    ("event_id", Wire::Text),
    ("occurred_at", Wire::Timestamp),
    ("sequence_no", Wire::BigIntText),
    ("severity", Wire::Integer),
    ("outcome", Wire::Integer),
    ("code", Wire::Text),
    ("message_summary", Wire::Text),
    ("attributes", Wire::Json),
    ("raw_payload", Wire::Json),
    ("execution_id", Wire::Text),
    ("prompt_id", Wire::Text),
];

pub async fn trace_events(
    State(database): State<SharedDatabase>,
    Path(trace_id): Path<String>,
) -> Result<Json<Value>, HubError> {
    let trace_bytes = obs_common::decode_hex_bytes(&trace_id, 16)
        .map_err(|error| HubError::Internal(error.to_string(), None))?;
    let items = database
        .read(move |connection| {
            fetch_all(
                connection,
                "SELECT e.event_id,e.occurred_at,e.sequence_no,e.severity,e.outcome,d.code,e.message_summary,
                 e.attributes,e.raw_payload,x.execution_id,x.prompt_id
                 FROM events e JOIN event_definitions d ON d.event_definition_id=e.event_definition_id
                 JOIN executions x ON x.execution_id=e.execution_id
                 WHERE e.trace_id=?1 ORDER BY e.occurred_at,e.sequence_no",
                params![trace_bytes],
                TRACE_EVENT_COLUMNS,
            )
        })
        .await?;
    Ok(Json(json!({ "items": items })))
}
