//! Benchmark programs: `benchmark.init`, `benchmark.observe`, `benchmark.run`
//! (server.mjs:316-360).

use anyhow::{bail, Result};
use rusqlite::{params, Connection, OptionalExtension};

use super::fields::Fields;
use super::Operation;

const KEY_MAX: usize = 160;
const URL_MAX: usize = 1200;

// Mental model:
// upsert cited references → activate the program → supersede older contract snapshots
// → store this snapshot → store its metrics
pub fn initialize_program(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();

    for reference in data.array("references") {
        upsert_reference(connection, Fields::of(reference))?;
    }
    activate_program(connection, operation)?;
    supersede_other_snapshots(connection, operation)?;
    store_contract_snapshot(connection, operation)?;

    let contract_version_id = data.uuid("contractVersionId")?;
    for metric in data.array("metrics") {
        store_metric(
            connection,
            contract_version_id.as_deref(),
            Fields::of(metric),
        )?;
    }
    Ok(())
}

pub fn record_observation(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    let program_id = data.uuid("programId")?;
    let metric_key = data.text("metricKey", KEY_MAX);

    if !is_active_metric(connection, program_id.as_deref(), &metric_key)? {
        bail!("benchmark metric is not active for this program");
    }

    connection.execute(
        "INSERT INTO benchmark_observations(observation_id,program_id,metric_key,execution_id,prompt_id,value,
         observed_at,provenance_kind,provenance_locator,evidence_sha256,environment_fingerprint,comparable,
         validation_status,attributes)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14) ON CONFLICT(observation_id) DO NOTHING",
        params![
            data.uuid("observationId")?,
            program_id,
            metric_key,
            data.uuid_if_truthy("executionId")?,
            data.uuid_if_truthy("promptId")?,
            finite_observation_value(data)?,
            data.timestamp_or("observedAt", &operation.occurred_at)?,
            data.text("provenanceKind", 24),
            data.text("provenanceLocator", URL_MAX),
            data.sha256_hex_if_truthy("evidenceSha256")?,
            data.sha256_hex_if_truthy("environmentFingerprint")?,
            data.is_not_false("comparable"),
            data.text_or("validationStatus", "accepted", 24),
            data.json_object_text("attributes"),
        ],
    )?;
    Ok(())
}

pub fn record_run(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    let run_id = data.uuid("runId")?;

    connection.execute(
        "INSERT INTO benchmark_runs(run_id,program_id,contract_version_id,window_start,window_end,status,
         scipy_version,environment_fingerprint,created_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(run_id) DO NOTHING",
        params![
            run_id,
            data.uuid("programId")?,
            data.uuid("contractVersionId")?,
            data.timestamp_if_truthy("windowStart")?,
            data.timestamp("windowEnd")?,
            data.text("status", 32),
            data.text_or_null("scipyVersion", 80),
            data.raw_text_if_truthy("environmentFingerprint"),
            operation.occurred_at,
        ],
    )?;

    for result in data.array("results") {
        store_result(connection, run_id.as_deref(), Fields::of(result))?;
    }
    Ok(())
}

fn upsert_reference(connection: &Connection, reference: Fields<'_>) -> Result<()> {
    let reference_type = reference.text("type", 32);
    let source_url = reference.text("sourceUrl", URL_MAX);
    ensure_citable_source(&reference_type, &source_url)?;

    connection.execute(
        "INSERT INTO benchmark_references(reference_id,reference_key,reference_type,issuer,title,version,source_url,
         published_at,retrieved_at,content_sha256,review_due_at,status)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'active')
         ON CONFLICT(reference_key,version) DO UPDATE SET issuer=excluded.issuer,title=excluded.title,
         source_url=excluded.source_url,review_due_at=excluded.review_due_at",
        params![
            reference.uuid("referenceId")?,
            reference.text("key", KEY_MAX),
            reference_type,
            reference.text("issuer", 240),
            reference.text("title", 500),
            reference.text("version", 120),
            source_url,
            reference.timestamp_if_truthy("publishedAt")?,
            reference.timestamp("retrievedAt")?,
            reference.sha256_hex("contentSha256")?,
            reference.timestamp_if_truthy("reviewDueAt")?,
        ],
    )?;
    Ok(())
}

fn activate_program(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    connection.execute(
        "INSERT INTO benchmark_programs(program_id,project_id,name,measured_subject,status,approval_prompt_id,activated_at)
         VALUES(?1,?2,?3,?4,'active',?5,?6)
         ON CONFLICT(project_id,name) DO UPDATE SET measured_subject=excluded.measured_subject,status='active'",
        params![
            data.uuid("programId")?,
            data.uuid("projectId")?,
            data.text("name", 240),
            data.text_or("measuredSubject", "project", 32),
            data.uuid("approvalPromptId")?,
            data.timestamp_or("approvedAt", &operation.occurred_at)?,
        ],
    )?;
    Ok(())
}

fn supersede_other_snapshots(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    connection.execute(
        "UPDATE benchmark_contract_versions SET superseded_at=?2
         WHERE program_id=?1 AND superseded_at IS NULL AND snapshot_sha256<>?3",
        params![
            data.uuid("programId")?,
            operation.occurred_at,
            data.raw_text("snapshotSha256"),
        ],
    )?;
    Ok(())
}

fn store_contract_snapshot(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    connection.execute(
        "INSERT INTO benchmark_contract_versions(contract_version_id,program_id,version_no,source_path,snapshot_toml,
         snapshot_sha256,approved_by,approved_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(program_id,snapshot_sha256) DO NOTHING",
        params![
            data.uuid("contractVersionId")?,
            data.uuid("programId")?,
            data.integer("versionNo")?,
            data.text("sourcePath", URL_MAX),
            data.full_string("snapshotToml"),
            data.sha256_hex("snapshotSha256")?,
            data.text_or("approvedBy", "user", 120),
            data.timestamp_or("approvedAt", &operation.occurred_at)?,
        ],
    )?;
    Ok(())
}

fn store_metric(
    connection: &Connection,
    contract_version_id: Option<&str>,
    metric: Fields<'_>,
) -> Result<()> {
    connection.execute(
        "INSERT INTO benchmark_metrics(metric_id,contract_version_id,reference_id,metric_key,title,unit,aggregation,
         direction,evaluation_method,hypothesis_mode,null_hypothesis,alternative_hypothesis,target_value,margin,
         lower_bound,upper_bound,alpha,confidence_level,minimum_samples,window_days,required,core_extractor,
         evidence_requirements,comparability_rules)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24)
         ON CONFLICT(contract_version_id,metric_key) DO NOTHING",
        params![
            metric.uuid("metricId")?,
            contract_version_id,
            metric.uuid_if_truthy("referenceId")?,
            metric.text("key", KEY_MAX),
            metric.text("title", 240),
            metric.text("unit", 80),
            metric.text_or("aggregation", "mean", 16),
            metric.text("direction", 16),
            metric.text("evaluationMethod", 32),
            metric.text_if_truthy("hypothesisMode", 24),
            metric.text("nullHypothesis", URL_MAX),
            metric.text("alternativeHypothesis", URL_MAX),
            metric.real("targetValue")?,
            metric.real_or("margin", 0.0)?,
            metric.real("lowerBound")?,
            metric.real("upperBound")?,
            metric.real_or("alpha", 0.05)?,
            metric.real_or("confidenceLevel", 0.95)?,
            metric.integer_or("minimumSamples", 1)?,
            metric.integer_if_truthy("windowDays")?,
            metric.is_not_false("required"),
            metric.text_if_truthy("coreExtractor", 64),
            metric.json_array_text("evidenceRequirements"),
            metric.json_object_text("comparabilityRules"),
        ],
    )?;
    Ok(())
}

fn store_result(connection: &Connection, run_id: Option<&str>, result: Fields<'_>) -> Result<()> {
    connection.execute(
        "INSERT INTO benchmark_results(result_id,run_id,metric_id,sample_count,observed_value,statistic,p_value,
         confidence_low,confidence_high,verdict,readiness_gaps,details)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12) ON CONFLICT(run_id,metric_id) DO NOTHING",
        params![
            result.uuid("resultId")?,
            run_id,
            result.uuid("metricId")?,
            result.integer_or("sampleCount", 0)?,
            result.real("observedValue")?,
            result.real("statistic")?,
            result.real("pValue")?,
            result.real("confidenceLow")?,
            result.real("confidenceHigh")?,
            result.text("verdict", 32),
            result.json_array_text("readinessGaps"),
            result.json_object_text("details"),
        ],
    )?;
    Ok(())
}

fn is_active_metric(
    connection: &Connection,
    program_id: Option<&str>,
    metric_key: &str,
) -> Result<bool> {
    let found: Option<i64> = connection
        .query_row(
            "SELECT 1 FROM benchmark_metrics m JOIN benchmark_contract_versions v USING(contract_version_id)
             WHERE v.program_id=?1 AND v.superseded_at IS NULL AND m.metric_key=?2",
            params![program_id, metric_key],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

/// Postgres CHECK `value NOT IN ('NaN','Infinity','-Infinity')` plus NOT NULL.
fn finite_observation_value(data: Fields<'_>) -> Result<f64> {
    match data.real("value")? {
        Some(value) if value.is_finite() => Ok(value),
        Some(_) => bail!("benchmark observation value must be finite"),
        None => bail!("null value in column \"value\" violates not-null constraint"),
    }
}

/// Postgres CHECK `source_url ~ '^https?://' OR (reference_type='internal_slo' AND source_url ~ '^project:')`.
fn ensure_citable_source(reference_type: &str, source_url: &str) -> Result<()> {
    if !is_citable_source(reference_type, source_url) {
        bail!("benchmark reference source_url violates check constraint");
    }
    Ok(())
}

#[inline]
fn is_citable_source(reference_type: &str, source_url: &str) -> bool {
    source_url.starts_with("http://")
        || source_url.starts_with("https://")
        || (reference_type == "internal_slo" && source_url.starts_with("project:"))
}
