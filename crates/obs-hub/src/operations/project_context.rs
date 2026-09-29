//! Shared project memory: `context.record`, `handoff.record`, `handoff.promote`
//! (server.mjs:218-223, 256-268).

use anyhow::Result;
use rusqlite::{params, Connection};
use serde_json::Value;

use super::js_values::{is_nullish, js_number};
use super::Operation;

const DEFAULT_LANGUAGE: &str = "tl-en";
const SUMMARY_MAX: usize = 2000;
const HANDOFF_CONTENT_MAX: usize = 12000;
const PROMOTION_REQUIRED_KINDS: [&str; 2] = ["architecture", "constraint"];

pub fn record_context(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    let project_id = data.uuid("projectId")?;
    let next_version = latest_context_version(connection, project_id.as_deref())? + 1;

    connection.execute(
        "INSERT INTO project_context_versions(context_id,project_id,version_no,language_code,vision_summary,
         architecture_summary,current_goal_summary,constraints,source_prompt_id)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(context_id) DO NOTHING",
        params![
            data.uuid("contextId")?,
            project_id,
            next_version,
            data.text_or("languageCode", DEFAULT_LANGUAGE, 16),
            data.text("visionSummary", SUMMARY_MAX),
            data.text("architectureSummary", SUMMARY_MAX),
            data.text("currentGoalSummary", SUMMARY_MAX),
            data.json_array_text("constraints"),
            data.uuid_if_truthy("sourcePromptId")?,
        ],
    )?;
    Ok(())
}

// Mental model:
// read the current version → a stale base version is a conflict → architecture/constraint
// handoffs wait for promotion → store as the next version (active only when neither applies)
pub fn record_handoff(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    let project_id = data.uuid("projectId")?;
    let current_version = latest_handoff_version(connection, project_id.as_deref())?;

    let is_conflict = is_stale_base_version(data.raw("baseVersion"), current_version);
    let needs_promotion = is_promotion_required(&data.full_string("handoffKind"));

    connection.execute(
        "INSERT INTO project_handoffs(handoff_id,project_id,version_no,base_version,handoff_kind,profile_name,
         native_session,content,is_active,requires_promotion,conflict,created_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12) ON CONFLICT(handoff_id) DO NOTHING",
        params![
            data.uuid("handoffId")?,
            project_id,
            current_version + 1,
            data.integer("baseVersion")?,
            data.text("handoffKind", 32),
            data.text("profileName", 32),
            data.text_or_null("nativeSession", 240),
            data.text("content", HANDOFF_CONTENT_MAX),
            !is_conflict && !needs_promotion,
            needs_promotion,
            is_conflict,
            operation.occurred_at,
        ],
    )?;
    Ok(())
}

pub fn promote_handoff(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    connection.execute(
        "UPDATE project_handoffs SET is_active=1,promoted_at=?3
         WHERE handoff_id=?1 AND project_id=?2 AND conflict=0",
        params![
            data.uuid("handoffId")?,
            data.uuid("projectId")?,
            operation.occurred_at
        ],
    )?;
    Ok(())
}

fn latest_context_version(connection: &Connection, project_id: Option<&str>) -> Result<i64> {
    Ok(connection.query_row(
        "SELECT COALESCE(max(version_no),0) FROM project_context_versions WHERE project_id=?1",
        params![project_id],
        |row| row.get(0),
    )?)
}

fn latest_handoff_version(connection: &Connection, project_id: Option<&str>) -> Result<i64> {
    Ok(connection.query_row(
        "SELECT COALESCE(max(version_no),0) FROM project_handoffs WHERE project_id=?1",
        params![project_id],
        |row| row.get(0),
    )?)
}

/// `d.baseVersion !== null && d.baseVersion !== undefined && Number(d.baseVersion) !== currentVersion`.
#[inline]
fn is_stale_base_version(base_version: &Value, current_version: i64) -> bool {
    !is_nullish(base_version) && js_number(base_version) != current_version as f64
}

#[inline]
fn is_promotion_required(handoff_kind: &str) -> bool {
    PROMOTION_REQUIRED_KINDS.contains(&handoff_kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn base_version_conflict_follows_javascript_number_rules() {
        assert!(!is_stale_base_version(&json!(null), 3));
        assert!(!is_stale_base_version(&json!(3), 3));
        assert!(!is_stale_base_version(&json!("3"), 3));
        assert!(is_stale_base_version(&json!(2), 3));
        assert!(is_stale_base_version(&json!("x"), 3));
    }
}
