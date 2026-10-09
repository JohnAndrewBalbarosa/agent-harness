//! Project-scoped diagnostics: open errors, session bootstrap context, usage analytics
//! (server.mjs:404-429, 507-530).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;
use rusqlite::params;
use serde_json::{json, Value};

use super::path_params::{
    days_ago, joined_query_value, js_parse_int, query_text_or, uuid_parameter,
};
use super::wire::{fetch_all, fetch_optional, js_json_response, Columns, Wire};
use super::SharedDatabase;
use crate::http_errors::HubError;

const ANALYTICS_DEFAULT_DAYS: i64 = 7;
const ANALYTICS_MAX_DAYS: i64 = 30;

const ERROR_LIST_COLUMNS: Columns = &[
    ("fingerprint", Wire::Text),
    ("error_group_id", Wire::Text),
    ("exception_class", Wire::Text),
    ("normalized_summary", Wire::Text),
    ("first_seen_at", Wire::Timestamp),
    ("last_seen_at", Wire::Timestamp),
    ("occurrence_count", Wire::BigIntText),
    ("status", Wire::Integer),
];

pub async fn list_project_errors(
    State(database): State<SharedDatabase>,
    Path(project_id): Path<String>,
) -> Result<Json<Value>, HubError> {
    let project_id = uuid_parameter(&project_id)?;
    let items = database
        .read(move |connection| {
            fetch_all(
                connection,
                "SELECT lower(hex(g.fingerprint)),g.error_group_id,g.exception_class,g.normalized_summary,
                 g.first_seen_at,g.last_seen_at,g.occurrence_count,g.status
                 FROM error_groups g WHERE g.project_id=?1 ORDER BY g.last_seen_at DESC LIMIT 200",
                params![project_id],
                ERROR_LIST_COLUMNS,
            )
        })
        .await?;
    Ok(Json(json!({ "items": items })))
}

const CONTEXT_COLUMNS: Columns = &[
    ("version_no", Wire::Integer),
    ("language_code", Wire::Text),
    ("vision_summary", Wire::Text),
    ("architecture_summary", Wire::Text),
    ("current_goal_summary", Wire::Text),
    ("constraints", Wire::Json),
    ("created_at", Wire::Timestamp),
];

const HANDOFF_COLUMNS: Columns = &[
    ("version_no", Wire::BigIntText),
    ("handoff_kind", Wire::Text),
    ("profile_name", Wire::Text),
    ("content", Wire::Text),
    ("created_at", Wire::Timestamp),
];

const OPEN_ERROR_COLUMNS: Columns = &[
    ("error_group_id", Wire::Text),
    ("normalized_summary", Wire::Text),
    ("last_seen_at", Wire::Timestamp),
    ("occurrence_count", Wire::BigIntText),
];

const CODE_VERSION_COLUMNS: Columns = &[
    ("commit_sha", Wire::Text),
    ("branch_name", Wire::Text),
    ("build_version", Wire::Text),
    ("created_at", Wire::Timestamp),
];

// Mental model:
// project must exist → latest context + 5 active handoffs + 3 open errors + latest code version
pub async fn bootstrap_project_context(
    State(database): State<SharedDatabase>,
    Path(raw_project_id): Path<String>,
    Query(query): Query<Vec<(String, String)>>,
) -> Result<Response, HubError> {
    let project_id = uuid_parameter(&raw_project_id)?;
    let snapshot = database
        .read(move |connection| {
            let exists = fetch_optional(
                connection,
                "SELECT project_id FROM projects WHERE project_id=?1",
                params![project_id],
                &[("project_id", Wire::Text)],
            )?
            .is_some();
            if !exists {
                return Ok(None);
            }
            Ok(Some((
                fetch_optional(
                    connection,
                    "SELECT version_no,language_code,vision_summary,architecture_summary,current_goal_summary,
                     constraints,created_at FROM project_context_versions WHERE project_id=?1
                     ORDER BY version_no DESC LIMIT 1",
                    params![project_id],
                    CONTEXT_COLUMNS,
                )?,
                fetch_all(
                    connection,
                    "SELECT version_no,handoff_kind,profile_name,content,created_at FROM project_handoffs
                     WHERE project_id=?1 AND is_active=1 AND conflict=0 ORDER BY version_no DESC LIMIT 5",
                    params![project_id],
                    HANDOFF_COLUMNS,
                )?,
                fetch_all(
                    connection,
                    "SELECT error_group_id,normalized_summary,last_seen_at,occurrence_count FROM error_groups
                     WHERE project_id=?1 AND status=1 ORDER BY last_seen_at DESC LIMIT 3",
                    params![project_id],
                    OPEN_ERROR_COLUMNS,
                )?,
                fetch_optional(
                    connection,
                    "SELECT commit_sha,branch_name,build_version,created_at FROM code_versions
                     WHERE project_id=?1 ORDER BY created_at DESC LIMIT 1",
                    params![project_id],
                    CODE_VERSION_COLUMNS,
                )?,
            )))
        })
        .await?;

    let Some((context, handoffs, errors, code_version)) = snapshot else {
        return Err(HubError::not_found("project not found"));
    };
    Ok(js_json_response(
        StatusCode::OK,
        &json!({
            "project_id": raw_project_id,
            "profile": query_text_or(joined_query_value(&query, "profile").as_ref(), 32, "personal"),
            "source": query_text_or(joined_query_value(&query, "source").as_ref(), 32, "startup"),
            "context": context,
            "handoffs": handoffs,
            "errors": errors,
            "code_version": code_version,
        }),
    ))
}

const PROMPT_USAGE_COLUMNS: Columns = &[
    ("prompt_id", Wire::Text),
    ("source", Wire::Text),
    ("input_tokens", Wire::BigIntText),
    ("output_tokens", Wire::BigIntText),
    ("cached_input_tokens", Wire::BigIntText),
    ("reasoning_tokens", Wire::BigIntText),
    ("records", Wire::BigIntText),
];

const TOOL_USAGE_COLUMNS: Columns = &[
    ("tool_name", Wire::Text),
    ("origin", Wire::Text),
    ("calls", Wire::BigIntText),
    ("successes", Wire::BigIntText),
    ("failures", Wire::BigIntText),
    ("outcome_unknown", Wire::BigIntText),
    ("dynamic_discovery_calls", Wire::BigIntText),
    ("cache_hits", Wire::BigIntText),
    ("cache_misses", Wire::BigIntText),
    ("cache_unknown", Wire::BigIntText),
    ("avg_duration_ms", Wire::BigIntText),
];

/// Hook-sourced usage wins over other sources for the same prompt (`DISTINCT ON` → `ROW_NUMBER`).
const PROMPT_USAGE_SQL: &str = "
    WITH by_source AS (
      SELECT prompt_id,source,SUM(input_tokens) input_tokens,SUM(output_tokens) output_tokens,
        SUM(cached_input_tokens) cached_input_tokens,SUM(reasoning_tokens) reasoning_tokens,COUNT(*) records
      FROM turn_usage WHERE project_id=?1 AND prompt_id IS NOT NULL AND availability='exact' AND recorded_at>=?2
      GROUP BY prompt_id,source
    ), ranked AS (
      SELECT *, ROW_NUMBER() OVER (PARTITION BY prompt_id
        ORDER BY CASE WHEN source IN ('codex_hook','claude_hook') THEN 0 ELSE 1 END, source) preference
      FROM by_source
    )
    SELECT prompt_id,source,input_tokens,output_tokens,cached_input_tokens,reasoning_tokens,records
    FROM ranked WHERE preference=1 ORDER BY prompt_id DESC LIMIT 100";

const TOOL_USAGE_SQL: &str = "
    SELECT tool_name,origin,COUNT(*) calls,
      COUNT(*) FILTER (WHERE failed=0) successes,
      COUNT(*) FILTER (WHERE failed=1) failures,
      COUNT(*) FILTER (WHERE failed IS NULL) outcome_unknown,
      COUNT(*) FILTER (WHERE lower(tool_name) IN ('toolsearch','tool_search','functions.tool_search','functions__tool_search')) dynamic_discovery_calls,
      COUNT(*) FILTER (WHERE cache_status='hit') cache_hits,COUNT(*) FILTER (WHERE cache_status='miss') cache_misses,
      COUNT(*) FILTER (WHERE cache_status='unknown') cache_unknown,
      CAST(ROUND(AVG(duration_ms)) AS INTEGER) avg_duration_ms
    FROM tool_uses WHERE project_id=?1 AND completed_at>=?2
    GROUP BY tool_name,origin ORDER BY calls DESC,tool_name LIMIT 100";

pub async fn usage_analytics(
    State(database): State<SharedDatabase>,
    Path(project_id): Path<String>,
    Query(query): Query<Vec<(String, String)>>,
) -> Result<Json<Value>, HubError> {
    if !is_canonical_uuid(&project_id) {
        return Err(HubError::bad_request("invalid project id"));
    }
    let project_id = project_id.to_ascii_lowercase();
    let days = analytics_window_days(joined_query_value(&query, "days").as_ref());
    let since = days_ago(days);

    let (prompt_usage, tools) = database
        .read(move |connection| {
            Ok((
                fetch_all(
                    connection,
                    PROMPT_USAGE_SQL,
                    params![project_id, since],
                    PROMPT_USAGE_COLUMNS,
                )?,
                fetch_all(
                    connection,
                    TOOL_USAGE_SQL,
                    params![project_id, since],
                    TOOL_USAGE_COLUMNS,
                )?,
            ))
        })
        .await?;

    Ok(Json(
        json!({ "days": days, "promptUsage": prompt_usage, "tools": tools }),
    ))
}

/// `Math.min(30, Math.max(1, Number.parseInt(days, 10) || 7))`.
#[inline]
fn analytics_window_days(raw: Option<&String>) -> i64 {
    let requested = js_parse_int(raw)
        .filter(|days| *days != 0)
        .unwrap_or(ANALYTICS_DEFAULT_DAYS);
    requested.clamp(1, ANALYTICS_MAX_DAYS)
}

/// `/^[a-f0-9]{8}-[a-f0-9]{4}-[1-8][a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/i`.
fn is_canonical_uuid(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    bytes.len() == 36
        && [8, 13, 18, 23].iter().all(|&index| bytes[index] == b'-')
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 8 | 13 | 18 | 23) || byte.is_ascii_hexdigit())
        && (b'1'..=b'8').contains(&bytes[14])
        && matches!(bytes[19], b'8' | b'9' | b'a' | b'b')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analytics_window_matches_server_mjs() {
        let days = |text: &str| analytics_window_days(Some(&text.to_owned()));
        assert_eq!(analytics_window_days(None), 7);
        assert_eq!(days("0"), 7);
        assert_eq!(days("-5"), 1);
        assert_eq!(days("90"), 30);
        assert_eq!(days("3"), 3);
    }

    #[test]
    fn canonical_uuid_check_matches_regex() {
        assert!(is_canonical_uuid("01A0D736-8ED8-76DC-9C20-C342229F9A82"));
        assert!(!is_canonical_uuid("01a0d736-8ed8-06dc-9c20-c342229f9a82"));
        assert!(!is_canonical_uuid("nope"));
    }
}
