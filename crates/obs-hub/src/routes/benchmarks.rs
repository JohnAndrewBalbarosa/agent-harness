//! Benchmark reads: program list, evaluation data (with hook-derived observations), latest report
//! (server.mjs:431-485).

use anyhow::Result;
use axum::extract::{Path, Query, State};
use axum::Json;
use rusqlite::{params, Connection};
use serde_json::{json, Value};

use super::path_params::{joined_query_value, optional_instant, uuid_parameter};
use super::wire::{fetch_all, fetch_optional, Columns, Wire};
use super::SharedDatabase;
use crate::http_errors::HubError;

const PROGRAM_SUMMARY_COLUMNS: Columns = &[
    ("program_id", Wire::Text),
    ("name", Wire::Text),
    ("measured_subject", Wire::Text),
    ("status", Wire::Text),
    ("activated_at", Wire::Timestamp),
    ("contract_version_id", Wire::Text),
    ("version_no", Wire::Integer),
    ("snapshot_sha256", Wire::Text),
    ("approved_at", Wire::Timestamp),
    ("metric_count", Wire::Integer),
    ("required_metric_count", Wire::Integer),
];

/// Latest contract per program (`LEFT JOIN LATERAL ... LIMIT 1` → correlated subquery).
pub async fn list_programs(
    State(database): State<SharedDatabase>,
    Path(project_id): Path<String>,
) -> Result<Json<Value>, HubError> {
    let project_id = uuid_parameter(&project_id)?;
    let items = database
        .read(move |connection| {
            fetch_all(
                connection,
                "SELECT p.program_id,p.name,p.measured_subject,p.status,p.activated_at,
                   v.contract_version_id,v.version_no,v.snapshot_sha256,v.approved_at,
                   count(m.metric_id),count(m.metric_id) FILTER (WHERE m.required)
                 FROM benchmark_programs p
                 LEFT JOIN benchmark_contract_versions v ON v.contract_version_id=(
                   SELECT c.contract_version_id FROM benchmark_contract_versions c
                   WHERE c.program_id=p.program_id ORDER BY c.version_no DESC LIMIT 1)
                 LEFT JOIN benchmark_metrics m ON m.contract_version_id=v.contract_version_id
                 WHERE p.project_id=?1
                 GROUP BY p.program_id,v.contract_version_id ORDER BY p.name",
                params![project_id],
                PROGRAM_SUMMARY_COLUMNS,
            )
        })
        .await?;
    Ok(Json(json!({ "items": items })))
}

const PROGRAM_COLUMNS: Columns = &[
    ("program_id", Wire::Text),
    ("project_id", Wire::Text),
    ("name", Wire::Text),
    ("measured_subject", Wire::Text),
    ("status", Wire::Text),
    ("approval_prompt_id", Wire::Text),
    ("activated_at", Wire::Timestamp),
    ("created_at", Wire::Timestamp),
    ("contract_version_id", Wire::Text),
    ("version_no", Wire::Integer),
    ("snapshot_sha256", Wire::Text),
];

const METRIC_COLUMNS: Columns = &[
    ("metric_id", Wire::Text),
    ("contract_version_id", Wire::Text),
    ("reference_id", Wire::Text),
    ("metric_key", Wire::Text),
    ("title", Wire::Text),
    ("unit", Wire::Text),
    ("aggregation", Wire::Text),
    ("direction", Wire::Text),
    ("evaluation_method", Wire::Text),
    ("hypothesis_mode", Wire::Text),
    ("null_hypothesis", Wire::Text),
    ("alternative_hypothesis", Wire::Text),
    ("target_value", Wire::Real),
    ("margin", Wire::Real),
    ("lower_bound", Wire::Real),
    ("upper_bound", Wire::Real),
    ("alpha", Wire::Real),
    ("confidence_level", Wire::Real),
    ("minimum_samples", Wire::Integer),
    ("window_days", Wire::Integer),
    ("required", Wire::Bool),
    ("core_extractor", Wire::Text),
    ("evidence_requirements", Wire::Json),
    ("comparability_rules", Wire::Json),
    ("created_at", Wire::Timestamp),
    ("reference_key", Wire::Text),
    ("reference_type", Wire::Text),
    ("issuer", Wire::Text),
    ("reference_title", Wire::Text),
    ("reference_version", Wire::Text),
    ("source_url", Wire::Text),
    ("review_due_at", Wire::Timestamp),
];

const OBSERVATION_COLUMNS: Columns = &[
    ("observation_id", Wire::Text),
    ("metric_key", Wire::Text),
    ("value", Wire::Real),
    ("observed_at", Wire::Timestamp),
    ("provenance_kind", Wire::Text),
    ("provenance_locator", Wire::Text),
    ("evidence_sha256", Wire::Text),
    ("environment_fingerprint", Wire::Text),
    ("comparable", Wire::Bool),
    ("validation_status", Wire::Text),
    ("attributes", Wire::Json),
];

/// The time window every evaluation query shares: `to` defaults to now, `from` is optional.
struct EvaluationWindow {
    project_id: String,
    to: String,
    from: Option<String>,
}

// Mental model:
// active program + contract → its metrics → stored observations in the window
// → observations derived from hook data for metrics with a core extractor
pub async fn evaluation_data(
    State(database): State<SharedDatabase>,
    Path((project_id, program_id)): Path<(String, String)>,
    Query(query): Query<Vec<(String, String)>>,
) -> Result<Json<Value>, HubError> {
    let window = EvaluationWindow {
        project_id: uuid_parameter(&project_id)?,
        to: optional_instant(joined_query_value(&query, "to").as_ref())
            .unwrap_or_else(obs_common::now_utc_micros),
        from: optional_instant(joined_query_value(&query, "from").as_ref()),
    };
    let program_id = uuid_parameter(&program_id)?;

    let found = database
        .read(move |connection| load_evaluation_data(connection, &window, &program_id))
        .await?;
    found
        .map(Json)
        .ok_or_else(|| HubError::not_found("benchmark program not found"))
}

fn load_evaluation_data(
    connection: &Connection,
    window: &EvaluationWindow,
    program_id: &str,
) -> Result<Option<Value>> {
    let Some(program) = active_program(connection, &window.project_id, program_id)? else {
        return Ok(None);
    };
    let contract_version_id = program["contract_version_id"]
        .as_str()
        .unwrap_or_default()
        .to_owned();

    let metrics = fetch_all(
        connection,
        "SELECT m.metric_id,m.contract_version_id,m.reference_id,m.metric_key,m.title,m.unit,m.aggregation,
           m.direction,m.evaluation_method,m.hypothesis_mode,m.null_hypothesis,m.alternative_hypothesis,
           m.target_value,m.margin,m.lower_bound,m.upper_bound,m.alpha,m.confidence_level,m.minimum_samples,
           m.window_days,m.required,m.core_extractor,m.evidence_requirements,m.comparability_rules,m.created_at,
           r.reference_key,r.reference_type,r.issuer,r.title,r.version,r.source_url,r.review_due_at
         FROM benchmark_metrics m LEFT JOIN benchmark_references r ON r.reference_id=m.reference_id
         WHERE m.contract_version_id=?1 ORDER BY m.metric_key",
        params![contract_version_id],
        METRIC_COLUMNS,
    )?;

    let mut observations = fetch_all(
        connection,
        "SELECT observation_id,metric_key,value,observed_at,provenance_kind,provenance_locator,evidence_sha256,
           environment_fingerprint,comparable,validation_status,attributes
         FROM benchmark_observations WHERE program_id=?1 AND observed_at<=?2 AND (?3 IS NULL OR observed_at>=?3)
         ORDER BY observed_at,observation_id",
        params![program_id, window.to, window.from],
        OBSERVATION_COLUMNS,
    )?;
    for metric in &metrics {
        observations.extend(derived_observations(connection, window, metric)?);
    }

    Ok(Some(
        json!({ "program": program, "metrics": metrics, "observations": observations }),
    ))
}

fn active_program(
    connection: &Connection,
    project_id: &str,
    program_id: &str,
) -> Result<Option<Value>> {
    fetch_optional(
        connection,
        "SELECT p.program_id,p.project_id,p.name,p.measured_subject,p.status,p.approval_prompt_id,p.activated_at,
           p.created_at,v.contract_version_id,v.version_no,v.snapshot_sha256
         FROM benchmark_programs p
         JOIN benchmark_contract_versions v ON v.contract_version_id=(
           SELECT c.contract_version_id FROM benchmark_contract_versions c
           WHERE c.program_id=p.program_id AND c.superseded_at IS NULL ORDER BY c.version_no DESC LIMIT 1)
         WHERE p.project_id=?1 AND p.program_id=?2",
        params![project_id, program_id],
        PROGRAM_COLUMNS,
    )
}

/// One hook-derived observation source per `core_extractor`.
struct CoreExtractor {
    sql: &'static str,
    value_wire: Wire,
    provenance_locator: &'static str,
}

const SUCCESSFUL_CYCLE_TIME: CoreExtractor = CoreExtractor {
    sql: "SELECT execution_id,CAST(duration_ms AS REAL),ended_at FROM executions
          WHERE project_id=?1 AND outcome=1 AND duration_ms IS NOT NULL AND ended_at<=?2
          AND (?3 IS NULL OR ended_at>=?3)",
    value_wire: Wire::Real,
    provenance_locator: "executions.duration_ms",
};

const CHANGE_FAILURE_RATE: CoreExtractor = CoreExtractor {
    sql: "SELECT assessment_id,CAST(status='failed' AS REAL),completed_at FROM prompt_change_assessments
          WHERE project_id=?1 AND change_kind='code' AND status IN ('success','failed') AND completed_at<=?2
          AND (?3 IS NULL OR completed_at>=?3)",
    value_wire: Wire::Real,
    provenance_locator: "prompt_change_assessments.status",
};

/// Postgres typed `THEN 1.0 ELSE 0.0` as numeric, which node-pg returned as the strings "1.0"/"0.0".
const OBSERVABILITY_COMPLETENESS: CoreExtractor = CoreExtractor {
    sql: "SELECT x.execution_id,CASE WHEN x.ended_at IS NOT NULL AND x.duration_ms IS NOT NULL
            AND x.trace_id IS NOT NULL AND x.prompt_id IS NOT NULL
            AND EXISTS(SELECT 1 FROM events e WHERE e.execution_id=x.execution_id) THEN '1.0' ELSE '0.0' END,
          x.ended_at
          FROM executions x WHERE x.project_id=?1 AND x.started_at<=?2 AND (?3 IS NULL OR x.started_at>=?3)",
    value_wire: Wire::Text,
    provenance_locator: "executions/events completeness",
};

fn core_extractor(name: &str) -> Option<&'static CoreExtractor> {
    match name {
        "successful_cycle_time_ms" => Some(&SUCCESSFUL_CYCLE_TIME),
        "change_failure_rate" => Some(&CHANGE_FAILURE_RATE),
        "observability_completeness_rate" => Some(&OBSERVABILITY_COMPLETENESS),
        _ => None,
    }
}

fn derived_observations(
    connection: &Connection,
    window: &EvaluationWindow,
    metric: &Value,
) -> Result<Vec<Value>> {
    let Some(extractor) = metric["core_extractor"].as_str().and_then(core_extractor) else {
        return Ok(Vec::new());
    };
    let metric_key = metric["metric_key"].as_str().unwrap_or_default();

    let mut statement = connection.prepare_cached(extractor.sql)?;
    let rows = statement.query_map(params![window.project_id, window.to, window.from], |row| {
        Ok(derived_observation(row, extractor, metric_key))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn derived_observation(
    row: &rusqlite::Row<'_>,
    extractor: &CoreExtractor,
    metric_key: &str,
) -> Value {
    let cell = |index: usize, wire: Wire| {
        row.get_ref(index)
            .map(|value| super::wire::encode_cell(value, wire))
            .unwrap_or(Value::Null)
    };
    let source_id = cell(0, Wire::Text);

    json!({
        "observation_id": format!("core:{metric_key}:{}", source_id.as_str().unwrap_or_default()),
        "metric_key": metric_key,
        "value": cell(1, extractor.value_wire),
        "observed_at": cell(2, Wire::Timestamp),
        "provenance_kind": "hook",
        "provenance_locator": extractor.provenance_locator,
        "comparable": true,
        "validation_status": "accepted",
        "attributes": {},
    })
}

const RUN_COLUMNS: Columns = &[
    ("run_id", Wire::Text),
    ("program_id", Wire::Text),
    ("contract_version_id", Wire::Text),
    ("window_start", Wire::Timestamp),
    ("window_end", Wire::Timestamp),
    ("status", Wire::Text),
    ("scipy_version", Wire::Text),
    ("environment_fingerprint", Wire::Text),
    ("created_at", Wire::Timestamp),
];

const RESULT_COLUMNS: Columns = &[
    ("result_id", Wire::Text),
    ("run_id", Wire::Text),
    ("metric_id", Wire::Text),
    ("sample_count", Wire::Integer),
    ("observed_value", Wire::Real),
    ("statistic", Wire::Real),
    ("p_value", Wire::Real),
    ("confidence_low", Wire::Real),
    ("confidence_high", Wire::Real),
    ("verdict", Wire::Text),
    ("readiness_gaps", Wire::Json),
    ("details", Wire::Json),
    ("created_at", Wire::Timestamp),
    ("metric_key", Wire::Text),
    ("title", Wire::Text),
    ("unit", Wire::Text),
    ("direction", Wire::Text),
    ("evaluation_method", Wire::Text),
    ("hypothesis_mode", Wire::Text),
];

pub async fn latest_report(
    State(database): State<SharedDatabase>,
    Path((project_id, program_id)): Path<(String, String)>,
) -> Result<Json<Value>, HubError> {
    let project_id = uuid_parameter(&project_id)?;
    let program_id = uuid_parameter(&program_id)?;

    let found = database
        .read(move |connection| {
            let Some(run) = fetch_optional(
                connection,
                "SELECT r.run_id,r.program_id,r.contract_version_id,r.window_start,r.window_end,r.status,
                   r.scipy_version,r.environment_fingerprint,r.created_at
                 FROM benchmark_runs r JOIN benchmark_programs p ON p.program_id=r.program_id
                 WHERE p.project_id=?1 AND r.program_id=?2 ORDER BY r.created_at DESC LIMIT 1",
                params![project_id, program_id],
                RUN_COLUMNS,
            )?
            else {
                return Ok(None);
            };
            let results = fetch_all(
                connection,
                "SELECT x.result_id,x.run_id,x.metric_id,x.sample_count,x.observed_value,x.statistic,x.p_value,
                   x.confidence_low,x.confidence_high,x.verdict,x.readiness_gaps,x.details,x.created_at,
                   m.metric_key,m.title,m.unit,m.direction,m.evaluation_method,m.hypothesis_mode
                 FROM benchmark_results x JOIN benchmark_metrics m ON m.metric_id=x.metric_id
                 WHERE x.run_id=?1 ORDER BY m.metric_key",
                params![run["run_id"].as_str().unwrap_or_default()],
                RESULT_COLUMNS,
            )?;
            Ok(Some(json!({ "run": run, "results": results })))
        })
        .await?;
    found
        .map(Json)
        .ok_or_else(|| HubError::not_found("no benchmark run found"))
}
