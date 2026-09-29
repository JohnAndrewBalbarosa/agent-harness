//! Code provenance and change accounting: `code-version.register`, `attribution.record`,
//! `change-assessment.plan|finish|package` (server.mjs:274-315).

use anyhow::Result;
use rusqlite::{params, Connection};

use super::fields::Fields;
use super::Operation;

const STREAM_MAX: usize = 240;
const CLASS_MAX: usize = 24;

pub fn register_code_version(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    connection.execute(
        "INSERT INTO code_versions(code_version_id,project_id,commit_sha,branch_name,dirty_fingerprint,build_version)
         VALUES(?1,?2,?3,?4,?5,?6)
         ON CONFLICT(code_version_id) DO UPDATE SET branch_name=excluded.branch_name,
         build_version=COALESCE(excluded.build_version,code_versions.build_version)",
        params![
            data.uuid("codeVersionId")?,
            data.uuid("projectId")?,
            data.raw_text_if_truthy("commitSha"),
            data.text_or_null("branchName", 240),
            data.hex_bytes_if_truthy("dirtyFingerprint", 32)?,
            data.text_or_null("buildVersion", 120),
        ],
    )?;
    Ok(())
}

pub fn record_attribution(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    connection.execute(
        "INSERT INTO change_attributions(attribution_id,execution_id,code_version_id,artifact_kind,locator_hash,
         locator_label,content_hash,metadata)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(attribution_id) DO NOTHING",
        params![
            data.uuid("attributionId")?,
            data.uuid("executionId")?,
            data.uuid_if_truthy("codeVersionId")?,
            data.integer_or("artifactKind", 1)?,
            data.hex_bytes("locatorHash", 32)?,
            data.text("locatorLabel", 500),
            data.hex_bytes_if_truthy("contentHash", 32)?,
            data.json_object_text("metadata"),
        ],
    )?;
    Ok(())
}

pub fn plan_change(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    connection.execute(
        "INSERT INTO prompt_change_assessments(assessment_id,project_id,prompt_id,delivery_stream,change_kind,
         declared_class,effective_class,classification_reason,estimated_lines,estimated_files,risk_flags,
         source_urls,created_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)
         ON CONFLICT(assessment_id) DO UPDATE SET change_kind=excluded.change_kind,
         declared_class=excluded.declared_class,effective_class=excluded.effective_class,
         classification_reason=excluded.classification_reason,estimated_lines=excluded.estimated_lines,
         estimated_files=excluded.estimated_files,risk_flags=excluded.risk_flags,source_urls=excluded.source_urls",
        params![
            data.uuid("assessmentId")?,
            data.uuid("projectId")?,
            data.uuid_if_truthy("promptId")?,
            data.text("deliveryStream", STREAM_MAX),
            data.text("changeKind", CLASS_MAX),
            data.text("declaredClass", CLASS_MAX),
            data.text("effectiveClass", CLASS_MAX),
            data.text("classificationReason", 500),
            data.integer_or("estimatedLines", 0)?,
            data.integer_or("estimatedFiles", 0)?,
            data.json_array_text("riskFlags"),
            data.json_array_text("sourceUrls"),
            operation.occurred_at,
        ],
    )?;
    Ok(())
}

pub fn finish_change(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    let usage = Fields::of(data.raw("usage"));

    connection.execute(
        "UPDATE prompt_change_assessments SET effective_class=?2,actual_lines_added=?3,actual_lines_deleted=?4,
         actual_files=?5,source_urls=?6,input_tokens=?7,output_tokens=?8,cached_input_tokens=?9,
         reasoning_tokens=?10,status=?11,increments_counter=?12,completed_at=?13
         WHERE assessment_id=?1",
        params![
            data.uuid("assessmentId")?,
            data.text("effectiveClass", CLASS_MAX),
            data.integer_or("actualLinesAdded", 0)?,
            data.integer_or("actualLinesDeleted", 0)?,
            data.integer_or("actualFiles", 0)?,
            data.json_array_text("sourceUrls"),
            usage.integer("input")?,
            usage.integer("output")?,
            usage.integer("cachedInput")?,
            usage.integer("reasoning")?,
            data.text("status", CLASS_MAX),
            data.is_truthy("incrementsCounter"),
            data.timestamp_or("completedAt", &operation.occurred_at)?,
        ],
    )?;
    Ok(())
}

// Mental model:
// record the package → attach every counted, still-unpackaged assessment of the stream to it
pub fn package_changes(connection: &Connection, operation: &Operation) -> Result<()> {
    let data = operation.fields();
    let project_id = data.uuid("projectId")?;
    let package_id = data.uuid("packageId")?;
    let delivery_stream = data.text("deliveryStream", STREAM_MAX);

    connection.execute(
        "INSERT INTO change_packages(package_id,project_id,delivery_stream,delivery_mode,commit_sha,pr_url,
         prompt_count,created_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(package_id) DO NOTHING",
        params![
            package_id,
            project_id,
            delivery_stream,
            data.text("deliveryMode", CLASS_MAX),
            data.text_or_null("commitSha", 64),
            data.text_or_null("prUrl", 1200),
            data.integer("promptCount")?,
            data.timestamp_or("createdAt", &operation.occurred_at)?,
        ],
    )?;
    connection.execute(
        "UPDATE prompt_change_assessments SET package_id=?2
         WHERE project_id=?1 AND delivery_stream=?3 AND increments_counter=1 AND package_id IS NULL",
        params![project_id, package_id, delivery_stream],
    )?;
    Ok(())
}
