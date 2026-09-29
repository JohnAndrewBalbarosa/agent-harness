//! `obs change finish` (obs.py:474 `finish_change` + CLI checks at :1412-1419): finalize the
//! prompt's assessment with actual counts, update the counter, and report `PACKAGE_NOW` once
//! the unpackaged successful code-changing prompts reach the threshold.

use rusqlite::Connection;
use serde_json::{json, Value};

use crate::argv::{parse, CommandSpec, OptionSpec, ParsedArgs};
use crate::change_assessment::classification::{classify_change, higher_class, ChangeClass};
use crate::change_assessment::counter::{
    counts_toward_package, finish_result, FinishSummary, DEFAULT_PACKAGE_THRESHOLD,
};
use crate::change_assessment::identity::{resolve_change_identity, ChangeIdentity};
use crate::change_assessment::source_urls::{
    merge_source_urls, parse_stored_list, validated_source_urls,
};
use crate::change_assessment::stored_list_text;
use crate::commands::output::print_command_result;
use crate::failure::{CliFailure, CliResult};
use crate::operation::build_operation;
use crate::python_compat::python_utc_now;
use crate::settings::Settings;
use crate::spool::begin_write;
use crate::spool::change_ledger::{
    count_unpackaged_code_prompts, finalize_assessment, find_assessment, FinalizedAssessment,
    StoredAssessment,
};
use crate::spool::flush::HubSink;
use crate::spool::open::open_spool;
use crate::spool::queue::enqueue_and_flush;

const OUTCOMES: &[&str] = &["success", "stopped", "failed"];
const PLANNED_STATUS: &str = "planned";

const SPEC: CommandSpec = CommandSpec {
    prog: "obs change finish",
    usage: "obs change finish [-h] --outcome {success,stopped,failed} [--lines-added LINES_ADDED] [--lines-deleted LINES_DELETED] [--files-changed FILES_CHANGED] [--input-tokens INPUT_TOKENS] [--output-tokens OUTPUT_TOKENS] [--cached-input-tokens CACHED_INPUT_TOKENS] [--reasoning-tokens REASONING_TOKENS] [--source-url SOURCE_URL] [--threshold THRESHOLD] [--prompt-id PROMPT_ID] [--delivery-stream DELIVERY_STREAM]",
    options: &[
        OptionSpec::one_of("--outcome", OUTCOMES).required(),
        OptionSpec::integer("--lines-added"),
        OptionSpec::integer("--lines-deleted"),
        OptionSpec::integer("--files-changed"),
        OptionSpec::integer("--input-tokens"),
        OptionSpec::integer("--output-tokens"),
        OptionSpec::integer("--cached-input-tokens"),
        OptionSpec::integer("--reasoning-tokens"),
        OptionSpec::repeated("--source-url"),
        OptionSpec::integer("--threshold"),
        OptionSpec::text("--prompt-id"),
        OptionSpec::text("--delivery-stream"),
    ],
    positionals: &[],
};

struct TokenUsage {
    input: Option<i64>,
    output: Option<i64>,
    cached_input: Option<i64>,
    reasoning: Option<i64>,
}

struct FinishRequest {
    outcome: String,
    lines_added: i64,
    lines_deleted: i64,
    files_changed: i64,
    usage: TokenUsage,
    source_urls: Vec<String>,
    threshold: i64,
    prompt_id: Option<String>,
    delivery_stream: Option<String>,
}

impl FinishRequest {
    fn from_args(args: &ParsedArgs) -> Self {
        Self {
            outcome: args.text("--outcome").unwrap_or_default().to_owned(),
            lines_added: args.integer("--lines-added").unwrap_or(0),
            lines_deleted: args.integer("--lines-deleted").unwrap_or(0),
            files_changed: args.integer("--files-changed").unwrap_or(0),
            usage: TokenUsage {
                input: args.integer("--input-tokens"),
                output: args.integer("--output-tokens"),
                cached_input: args.integer("--cached-input-tokens"),
                reasoning: args.integer("--reasoning-tokens"),
            },
            source_urls: args.texts("--source-url"),
            threshold: args
                .integer("--threshold")
                .unwrap_or(DEFAULT_PACKAGE_THRESHOLD),
            prompt_id: args.text("--prompt-id").map(str::to_owned),
            delivery_stream: args.text("--delivery-stream").map(str::to_owned),
        }
    }

    /// obs.py:1413-1418, checked before any spool or project work.
    fn validate(&self) -> CliResult<()> {
        if self.threshold < 1 {
            return Err(CliFailure::refusal("Threshold must be at least 1."));
        }

        let counts = [
            ("lines-added", Some(self.lines_added)),
            ("lines-deleted", Some(self.lines_deleted)),
            ("files-changed", Some(self.files_changed)),
            ("input-tokens", self.usage.input),
            ("output-tokens", self.usage.output),
            ("cached-input-tokens", self.usage.cached_input),
            ("reasoning-tokens", self.usage.reasoning),
        ];
        match counts
            .iter()
            .find(|(_, value)| value.is_some_and(|count| count < 0))
        {
            Some((name, _)) => Err(CliFailure::refusal(format!("{name} cannot be negative."))),
            None => Ok(()),
        }
    }
}

/// What the finish decided, before it is written.
struct FinishDecision {
    effective_class: ChangeClass,
    increments_counter: i64,
    newly_counted: bool,
    source_urls: Vec<String>,
    completed_at: String,
}

pub fn run(arguments: &[String]) -> CliResult<()> {
    let args = parse(&SPEC, arguments)?;
    let request = FinishRequest::from_args(&args);
    request.validate()?;
    let settings = Settings::from_env();
    let conn = open_spool()?;

    let result = finish_change(&conn, &settings, &request)?;
    print_command_result(&result)
}

/// Mental model: identity → load the planned row → decide class/counter (a finalized row keeps
/// its counter and outcome) → write + recount in one transaction → spool the operation.
fn finish_change(
    conn: &Connection,
    settings: &Settings,
    request: &FinishRequest,
) -> CliResult<Value> {
    let identity = resolve_change_identity(
        conn,
        request.prompt_id.as_deref(),
        request.delivery_stream.as_deref(),
    )?;
    let stored = find_assessment(
        conn,
        &identity.project_id,
        &identity.prompt_id,
        &identity.delivery_stream,
    )?
    .ok_or_else(|| {
        CliFailure::refusal(
            "No planned change assessment exists for this prompt and delivery stream.",
        )
    })?;

    let decision = decide_finish(&stored, request)?;
    let counted_prompts = record_finish(conn, &identity, &stored, request, &decision)?;

    let data = finish_operation_data(&identity, &stored, request, &decision);
    let operation = build_operation("change-assessment.finish", &data);
    enqueue_and_flush(conn, settings, &HubSink { settings }, &[operation])?;

    Ok(finish_result(&FinishSummary {
        assessment_id: &stored.assessment_id,
        project_id: &identity.project_id,
        prompt_id: &identity.prompt_id,
        delivery_stream: &identity.delivery_stream,
        effective_class: decision.effective_class,
        counted_prompts,
        threshold: request.threshold,
        newly_counted: decision.newly_counted,
    }))
}

fn decide_finish(stored: &StoredAssessment, request: &FinishRequest) -> CliResult<FinishDecision> {
    let already_finalized = stored.status != PLANNED_STATUS;
    if already_finalized && stored.status != request.outcome {
        return Err(CliFailure::refusal(
            "A finalized change assessment cannot change its outcome.",
        ));
    }

    let is_high_risk = !parse_stored_list(&stored.risk_flags_json)?.is_empty();
    let actual_class = classify_change(
        request.lines_added + request.lines_deleted,
        request.files_changed,
        is_high_risk,
    )?;
    let effective_class = higher_class(
        ChangeClass::parse_stored(&stored.effective_class)?,
        actual_class,
    );
    let increments_counter = if already_finalized {
        stored.increments_counter
    } else {
        i64::from(counts_toward_package(
            &request.outcome,
            &stored.change_kind,
            request.files_changed,
        ))
    };
    let new_urls = validated_source_urls(&request.source_urls)?;

    Ok(FinishDecision {
        effective_class,
        increments_counter,
        newly_counted: increments_counter != 0 && !already_finalized,
        source_urls: merge_source_urls(&stored.source_urls_json, &new_urls)?,
        completed_at: python_utc_now(),
    })
}

/// One write transaction: finalize the row, then count unpackaged code-changing prompts.
fn record_finish(
    conn: &Connection,
    identity: &ChangeIdentity,
    stored: &StoredAssessment,
    request: &FinishRequest,
    decision: &FinishDecision,
) -> CliResult<i64> {
    let transaction = begin_write(conn)?;

    finalize_assessment(
        &transaction,
        &FinalizedAssessment {
            assessment_id: stored.assessment_id.clone(),
            effective_class: decision.effective_class.as_str().to_owned(),
            lines_added: request.lines_added,
            lines_deleted: request.lines_deleted,
            files_changed: request.files_changed,
            source_urls_json: stored_list_text(&decision.source_urls),
            input_tokens: request.usage.input,
            output_tokens: request.usage.output,
            cached_input_tokens: request.usage.cached_input,
            reasoning_tokens: request.usage.reasoning,
            status: request.outcome.clone(),
            increments_counter: decision.increments_counter,
            completed_at: decision.completed_at.clone(),
        },
    )?;
    let counted = count_unpackaged_code_prompts(
        &transaction,
        &identity.project_id,
        &identity.delivery_stream,
    )?;

    transaction.commit()?;
    Ok(counted)
}

fn finish_operation_data(
    identity: &ChangeIdentity,
    stored: &StoredAssessment,
    request: &FinishRequest,
    decision: &FinishDecision,
) -> Value {
    json!({
        "assessmentId": stored.assessment_id,
        "projectId": identity.project_id,
        "promptId": identity.prompt_id,
        "deliveryStream": identity.delivery_stream,
        "effectiveClass": decision.effective_class.as_str(),
        "actualLinesAdded": request.lines_added,
        "actualLinesDeleted": request.lines_deleted,
        "actualFiles": request.files_changed,
        "sourceUrls": decision.source_urls,
        "usage": {
            "input": request.usage.input,
            "output": request.usage.output,
            "cachedInput": request.usage.cached_input,
            "reasoning": request.usage.reasoning,
        },
        "status": request.outcome,
        "incrementsCounter": decision.increments_counter != 0,
        "completedAt": decision.completed_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(outcome: &str, files: i64) -> FinishRequest {
        FinishRequest {
            outcome: outcome.to_owned(),
            lines_added: 10,
            lines_deleted: 2,
            files_changed: files,
            usage: TokenUsage {
                input: None,
                output: None,
                cached_input: None,
                reasoning: None,
            },
            source_urls: Vec::new(),
            threshold: DEFAULT_PACKAGE_THRESHOLD,
            prompt_id: None,
            delivery_stream: None,
        }
    }

    fn stored(status: &str, increments: i64) -> StoredAssessment {
        StoredAssessment {
            assessment_id: "a".to_owned(),
            change_kind: "code".to_owned(),
            effective_class: "routine".to_owned(),
            status: status.to_owned(),
            risk_flags_json: "[]".to_owned(),
            source_urls_json: "[]".to_owned(),
            increments_counter: increments,
        }
    }

    #[test]
    fn successful_code_finish_counts_once() {
        let decision = decide_finish(&stored("planned", 0), &request("success", 1)).unwrap();
        assert_eq!(decision.increments_counter, 1);
        assert!(decision.newly_counted);

        let replay = decide_finish(&stored("success", 1), &request("success", 1)).unwrap();
        assert_eq!(replay.increments_counter, 1);
        assert!(!replay.newly_counted);
    }

    #[test]
    fn finalized_outcome_cannot_change() {
        assert!(matches!(
            decide_finish(&stored("success", 1), &request("failed", 1)),
            Err(CliFailure::Refusal(_))
        ));
    }

    #[test]
    fn effective_class_never_drops_below_actual() {
        let mut big = request("success", 4);
        big.lines_added = 100;
        let decision = decide_finish(&stored("planned", 0), &big).unwrap();
        assert_eq!(decision.effective_class, ChangeClass::Substantial);
    }

    #[test]
    fn negative_counts_are_refused_with_obs_py_names() {
        let mut negative = request("success", 1);
        negative.usage.cached_input = Some(-1);
        assert!(matches!(
            negative.validate(),
            Err(CliFailure::Refusal(message)) if message == "cached-input-tokens cannot be negative."
        ));

        let mut low_threshold = request("success", 1);
        low_threshold.threshold = 0;
        assert!(matches!(
            low_threshold.validate(),
            Err(CliFailure::Refusal(_))
        ));
    }

    #[test]
    fn fifth_counted_prompt_triggers_package_now() {
        let conn = Connection::open_in_memory().unwrap();
        crate::spool::open::prepare_spool_connection(&conn).unwrap();
        for index in 0..5 {
            conn.execute(
                "INSERT INTO prompt_change_assessments(assessment_id,project_id,prompt_id,delivery_stream,change_kind,declared_class,effective_class,created_at) VALUES(?,'p',?,'main','code','routine','routine','t')",
                rusqlite::params![format!("a{index}"), format!("q{index}")],
            )
            .unwrap();
        }
        let identity = ChangeIdentity {
            project_id: "p".to_owned(),
            prompt_id: "q".to_owned(),
            delivery_stream: "main".to_owned(),
        };

        let mut counts = Vec::new();
        for index in 0..5 {
            let mut row = stored("planned", 0);
            row.assessment_id = format!("a{index}");
            let decision = decide_finish(&row, &request("success", 1)).unwrap();
            counts.push(
                record_finish(&conn, &identity, &row, &request("success", 1), &decision).unwrap(),
            );
        }

        assert_eq!(counts, vec![1, 2, 3, 4, 5]);
        let summary = FinishSummary {
            assessment_id: "a4",
            project_id: "p",
            prompt_id: "q4",
            delivery_stream: "main",
            effective_class: ChangeClass::Routine,
            counted_prompts: counts[4],
            threshold: DEFAULT_PACKAGE_THRESHOLD,
            newly_counted: true,
        };
        assert_eq!(finish_result(&summary)["directive"], "PACKAGE_NOW");
    }
}
