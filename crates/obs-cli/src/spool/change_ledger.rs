//! SQL for the deterministic change counter (obs.py:437-552): `prompt_change_assessments`
//! (one row per project + prompt + delivery stream) and `change_packages` (closed batches).
//! Callers own the transactions so each command keeps obs.py's `with conn:` boundaries.

use rusqlite::{params, Connection, OptionalExtension};

pub struct PlannedAssessment {
    pub assessment_id: String,
    pub project_id: String,
    pub prompt_id: String,
    pub delivery_stream: String,
    pub change_kind: String,
    pub declared_class: String,
    pub effective_class: String,
    pub classification_reason: String,
    pub estimated_lines: i64,
    pub estimated_files: i64,
    pub risk_flags_json: String,
    pub source_urls_json: String,
    pub created_at: String,
}

pub struct StoredAssessment {
    pub assessment_id: String,
    pub change_kind: String,
    pub effective_class: String,
    pub status: String,
    pub risk_flags_json: String,
    pub source_urls_json: String,
    pub increments_counter: i64,
}

pub struct FinalizedAssessment {
    pub assessment_id: String,
    pub effective_class: String,
    pub lines_added: i64,
    pub lines_deleted: i64,
    pub files_changed: i64,
    pub source_urls_json: String,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
    pub reasoning_tokens: Option<i64>,
    pub status: String,
    pub increments_counter: i64,
    pub completed_at: String,
}

pub struct ChangePackage {
    pub package_id: String,
    pub project_id: String,
    pub delivery_stream: String,
    pub delivery_mode: String,
    pub commit_sha: Option<String>,
    pub pr_url: Option<String>,
    pub prompt_count: i64,
    pub created_at: String,
}

pub fn assessment_status(
    conn: &Connection,
    assessment_id: &str,
) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT status FROM prompt_change_assessments WHERE assessment_id=?",
        params![assessment_id],
        |row| row.get(0),
    )
    .optional()
}

/// obs.py:450 insert, or refresh the plan fields of a still-planned assessment.
pub fn upsert_planned_assessment(
    conn: &Connection,
    plan: &PlannedAssessment,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO prompt_change_assessments(
                assessment_id,project_id,prompt_id,delivery_stream,change_kind,declared_class,effective_class,
                classification_reason,estimated_lines,estimated_files,risk_flags,source_urls,created_at)
                VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(assessment_id) DO UPDATE SET
                change_kind=excluded.change_kind,declared_class=excluded.declared_class,
                effective_class=excluded.effective_class,classification_reason=excluded.classification_reason,
                estimated_lines=excluded.estimated_lines,estimated_files=excluded.estimated_files,
                risk_flags=excluded.risk_flags,source_urls=excluded.source_urls",
        params![
            plan.assessment_id,
            plan.project_id,
            plan.prompt_id,
            plan.delivery_stream,
            plan.change_kind,
            plan.declared_class,
            plan.effective_class,
            plan.classification_reason,
            plan.estimated_lines,
            plan.estimated_files,
            plan.risk_flags_json,
            plan.source_urls_json,
            plan.created_at,
        ],
    )?;
    Ok(())
}

pub fn find_assessment(
    conn: &Connection,
    project_id: &str,
    prompt_id: &str,
    delivery_stream: &str,
) -> rusqlite::Result<Option<StoredAssessment>> {
    conn.query_row(
        "SELECT assessment_id,change_kind,effective_class,status,risk_flags,source_urls,increments_counter FROM prompt_change_assessments WHERE project_id=? AND prompt_id=? AND delivery_stream=?",
        params![project_id, prompt_id, delivery_stream],
        |row| {
            Ok(StoredAssessment {
                assessment_id: row.get(0)?,
                change_kind: row.get(1)?,
                effective_class: row.get(2)?,
                status: row.get(3)?,
                risk_flags_json: row.get(4)?,
                source_urls_json: row.get(5)?,
                increments_counter: row.get(6)?,
            })
        },
    )
    .optional()
}

pub fn finalize_assessment(
    conn: &Connection,
    finished: &FinalizedAssessment,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE prompt_change_assessments SET effective_class=?,actual_lines_added=?,
                actual_lines_deleted=?,actual_files=?,source_urls=?,input_tokens=?,output_tokens=?,
                cached_input_tokens=?,reasoning_tokens=?,status=?,increments_counter=?,completed_at=?
                WHERE assessment_id=?",
        params![
            finished.effective_class,
            finished.lines_added,
            finished.lines_deleted,
            finished.files_changed,
            finished.source_urls_json,
            finished.input_tokens,
            finished.output_tokens,
            finished.cached_input_tokens,
            finished.reasoning_tokens,
            finished.status,
            finished.increments_counter,
            finished.completed_at,
            finished.assessment_id,
        ],
    )?;
    Ok(())
}

/// The deterministic counter: successful code-changing prompts not yet packaged.
pub fn count_unpackaged_code_prompts(
    conn: &Connection,
    project_id: &str,
    delivery_stream: &str,
) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT count(*) FROM prompt_change_assessments WHERE project_id=? AND delivery_stream=? AND increments_counter=1 AND package_id IS NULL",
        params![project_id, delivery_stream],
        |row| row.get(0),
    )
}

pub fn unpackaged_assessment_ids(
    conn: &Connection,
    project_id: &str,
    delivery_stream: &str,
) -> rusqlite::Result<Vec<String>> {
    let mut statement = conn.prepare(
        "SELECT assessment_id FROM prompt_change_assessments WHERE project_id=? AND delivery_stream=? AND increments_counter=1 AND package_id IS NULL ORDER BY created_at,assessment_id",
    )?;
    let ids = statement.query_map(params![project_id, delivery_stream], |row| row.get(0))?;
    ids.collect()
}

/// obs.py:539 close the batch: one package row, every counted assessment linked to it.
pub fn record_package(
    conn: &Connection,
    package: &ChangePackage,
    assessment_ids: &[String],
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO change_packages(package_id,project_id,delivery_stream,delivery_mode,commit_sha,pr_url,prompt_count,created_at) VALUES(?,?,?,?,?,?,?,?)",
        params![
            package.package_id,
            package.project_id,
            package.delivery_stream,
            package.delivery_mode,
            package.commit_sha,
            package.pr_url,
            package.prompt_count,
            package.created_at,
        ],
    )?;

    for assessment_id in assessment_ids {
        conn.execute(
            "UPDATE prompt_change_assessments SET package_id=? WHERE assessment_id=?",
            params![package.package_id, assessment_id],
        )?;
    }
    Ok(())
}
