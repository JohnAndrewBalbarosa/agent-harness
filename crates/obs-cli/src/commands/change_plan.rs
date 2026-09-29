//! `obs change plan` (obs.py:437 `plan_change`): record the prompt's planned change class
//! before implementation. Planning never increments the counter.

use rusqlite::Connection;
use serde_json::{json, Value};

use crate::argv::{parse, CommandSpec, OptionSpec, ParsedArgs};
use crate::change_assessment::classification::{classify_change, higher_class, ChangeClass};
use crate::change_assessment::identity::{resolve_change_identity, ChangeIdentity};
use crate::change_assessment::source_urls::validated_source_urls;
use crate::change_assessment::stored_list_text;
use crate::commands::output::print_command_result;
use crate::failure::{CliFailure, CliResult};
use crate::operation::build_operation;
use crate::python_compat::{python_utc_now, truncate_chars, uuid5_url_text};
use crate::settings::Settings;
use crate::spool::begin_write;
use crate::spool::change_ledger::{
    assessment_status, upsert_planned_assessment, PlannedAssessment,
};
use crate::spool::flush::HubSink;
use crate::spool::open::open_spool;
use crate::spool::queue::enqueue_and_flush;

const CHANGE_KINDS: &[&str] = &["non-code", "docs", "media", "code"];
const DECLARED_CLASSES: &[&str] = &["routine", "moderate", "substantial"];
const CODE_KIND: &str = "code";
const PLANNED_STATUS: &str = "planned";
const MAX_RISK_FLAGS: usize = 20;
const MAX_RISK_FLAG_CHARS: usize = 120;
const MAX_REASON_CHARS: usize = 500;

const SPEC: CommandSpec = CommandSpec {
    prog: "obs change plan",
    usage: "obs change plan [-h] --kind {non-code,docs,media,code} [--class {routine,moderate,substantial}] [--estimated-lines ESTIMATED_LINES] [--estimated-files ESTIMATED_FILES] [--risk RISK] [--source-url SOURCE_URL] [--reason REASON] [--prompt-id PROMPT_ID] [--delivery-stream DELIVERY_STREAM]",
    options: &[
        OptionSpec::one_of("--kind", CHANGE_KINDS).required(),
        OptionSpec::one_of("--class", DECLARED_CLASSES),
        OptionSpec::integer("--estimated-lines"),
        OptionSpec::integer("--estimated-files"),
        OptionSpec::repeated("--risk"),
        OptionSpec::repeated("--source-url"),
        OptionSpec::text("--reason"),
        OptionSpec::text("--prompt-id"),
        OptionSpec::text("--delivery-stream"),
    ],
    positionals: &[],
};

struct PlanRequest {
    change_kind: String,
    declared_class: ChangeClass,
    estimated_lines: i64,
    estimated_files: i64,
    risk_flags: Vec<String>,
    source_urls: Vec<String>,
    reason: String,
    prompt_id: Option<String>,
    delivery_stream: Option<String>,
}

impl PlanRequest {
    fn from_args(args: &ParsedArgs) -> Self {
        Self {
            change_kind: args.text("--kind").unwrap_or_default().to_owned(),
            declared_class: args
                .text("--class")
                .and_then(ChangeClass::parse)
                .unwrap_or(ChangeClass::Routine),
            estimated_lines: args.integer("--estimated-lines").unwrap_or(0),
            estimated_files: args.integer("--estimated-files").unwrap_or(0),
            risk_flags: args.texts("--risk"),
            source_urls: args.texts("--source-url"),
            reason: truncate_chars(args.text("--reason").unwrap_or_default(), MAX_REASON_CHARS),
            prompt_id: args.text("--prompt-id").map(str::to_owned),
            delivery_stream: args.text("--delivery-stream").map(str::to_owned),
        }
    }
}

pub fn run(arguments: &[String]) -> CliResult<()> {
    let args = parse(&SPEC, arguments)?;
    let request = PlanRequest::from_args(&args);
    let settings = Settings::from_env();
    let conn = open_spool()?;

    let result = plan_change(&conn, &settings, &request)?;
    print_command_result(&result)
}

/// Mental model: identity → classify (never below the computed class for code) → record the
/// plan unless already finalized → spool the `change-assessment.plan` operation.
fn plan_change(conn: &Connection, settings: &Settings, request: &PlanRequest) -> CliResult<Value> {
    let identity = resolve_change_identity(
        conn,
        request.prompt_id.as_deref(),
        request.delivery_stream.as_deref(),
    )?;
    let risk_flags = bounded_risk_flags(&request.risk_flags);
    let computed = classify_change(
        request.estimated_lines,
        request.estimated_files,
        !risk_flags.is_empty(),
    )?;
    let (declared, effective) =
        declared_and_effective(&request.change_kind, request.declared_class, computed);

    let mut plan = PlannedAssessment {
        assessment_id: assessment_id_for(&identity),
        project_id: identity.project_id.clone(),
        prompt_id: identity.prompt_id.clone(),
        delivery_stream: identity.delivery_stream.clone(),
        change_kind: request.change_kind.clone(),
        declared_class: declared.as_str().to_owned(),
        effective_class: effective.as_str().to_owned(),
        classification_reason: request.reason.clone(),
        estimated_lines: request.estimated_lines,
        estimated_files: request.estimated_files,
        risk_flags_json: stored_list_text(&risk_flags),
        source_urls_json: String::new(),
        created_at: python_utc_now(),
    };
    let source_urls = record_plan(conn, &mut plan, &request.source_urls)?;

    let operation = build_operation(
        "change-assessment.plan",
        &plan_operation_data(&plan, &risk_flags, &source_urls),
    );
    enqueue_and_flush(conn, settings, &HubSink { settings }, &[operation])?;

    Ok(plan_result(&plan))
}

/// obs.py:439 `[item.strip()[:120] for item in args.risk if item.strip()][:20]`.
fn bounded_risk_flags(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| truncate_chars(value, MAX_RISK_FLAG_CHARS))
        .take(MAX_RISK_FLAGS)
        .collect()
}

/// Non-code kinds are always `non-code`; code takes the higher of declared and computed.
fn declared_and_effective(
    change_kind: &str,
    requested: ChangeClass,
    computed: ChangeClass,
) -> (ChangeClass, ChangeClass) {
    if change_kind == CODE_KIND {
        (requested, higher_class(requested, computed))
    } else {
        (ChangeClass::NonCode, ChangeClass::NonCode)
    }
}

#[inline]
fn assessment_id_for(identity: &ChangeIdentity) -> String {
    uuid5_url_text(&format!(
        "{}|{}|{}",
        identity.project_id, identity.prompt_id, identity.delivery_stream
    ))
}

/// One write transaction: refuse finalized assessments, validate links, upsert the plan.
fn record_plan(
    conn: &Connection,
    plan: &mut PlannedAssessment,
    raw_source_urls: &[String],
) -> CliResult<Vec<String>> {
    let transaction = begin_write(conn)?;

    let status = assessment_status(&transaction, &plan.assessment_id)?;
    if status.is_some_and(|status| status != PLANNED_STATUS) {
        return Err(CliFailure::refusal(
            "This prompt change assessment is already finalized.",
        ));
    }

    let source_urls = validated_source_urls(raw_source_urls)?;
    plan.source_urls_json = stored_list_text(&source_urls);
    upsert_planned_assessment(&transaction, plan)?;

    transaction.commit()?;
    Ok(source_urls)
}

fn plan_operation_data(
    plan: &PlannedAssessment,
    risk_flags: &[String],
    source_urls: &[String],
) -> Value {
    json!({
        "assessmentId": plan.assessment_id,
        "projectId": plan.project_id,
        "promptId": plan.prompt_id,
        "deliveryStream": plan.delivery_stream,
        "changeKind": plan.change_kind,
        "declaredClass": plan.declared_class,
        "effectiveClass": plan.effective_class,
        "classificationReason": plan.classification_reason,
        "estimatedLines": plan.estimated_lines,
        "estimatedFiles": plan.estimated_files,
        "riskFlags": risk_flags,
        "sourceUrls": source_urls,
    })
}

fn plan_result(plan: &PlannedAssessment) -> Value {
    json!({
        "assessmentId": plan.assessment_id,
        "projectId": plan.project_id,
        "promptId": plan.prompt_id,
        "deliveryStream": plan.delivery_stream,
        "changeKind": plan.change_kind,
        "declaredClass": plan.declared_class,
        "effectiveClass": plan.effective_class,
        "counterIncremented": false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risk_flags_are_trimmed_bounded_and_nonempty() {
        let values: Vec<String> = [" auth ", "  ", "x"]
            .iter()
            .map(|value| (*value).to_owned())
            .collect();
        assert_eq!(bounded_risk_flags(&values), vec!["auth", "x"]);

        let many: Vec<String> = (0..30).map(|index| format!("r{index}")).collect();
        assert_eq!(bounded_risk_flags(&many).len(), 20);
    }

    #[test]
    fn non_code_kinds_are_never_classified_above_non_code() {
        assert_eq!(
            declared_and_effective("docs", ChangeClass::Substantial, ChangeClass::HighRisk),
            (ChangeClass::NonCode, ChangeClass::NonCode)
        );
        assert_eq!(
            declared_and_effective("code", ChangeClass::Routine, ChangeClass::Moderate),
            (ChangeClass::Routine, ChangeClass::Moderate)
        );
    }

    #[test]
    fn finalized_assessment_cannot_be_replanned() {
        let conn = Connection::open_in_memory().unwrap();
        crate::spool::open::prepare_spool_connection(&conn).unwrap();
        conn.execute(
            "INSERT INTO prompt_change_assessments(assessment_id,project_id,prompt_id,delivery_stream,change_kind,declared_class,effective_class,status,created_at) VALUES('a','p','q','s','code','routine','routine','success','t')",
            [],
        )
        .unwrap();
        let mut plan = PlannedAssessment {
            assessment_id: "a".to_owned(),
            project_id: "p".to_owned(),
            prompt_id: "q".to_owned(),
            delivery_stream: "s".to_owned(),
            change_kind: "code".to_owned(),
            declared_class: "routine".to_owned(),
            effective_class: "routine".to_owned(),
            classification_reason: String::new(),
            estimated_lines: 1,
            estimated_files: 1,
            risk_flags_json: "[]".to_owned(),
            source_urls_json: String::new(),
            created_at: "t".to_owned(),
        };

        assert!(matches!(
            record_plan(&conn, &mut plan, &[]),
            Err(CliFailure::Refusal(_))
        ));
    }
}
