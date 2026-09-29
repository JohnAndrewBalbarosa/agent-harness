//! UserPromptSubmit (hook.py `if event == "UserPromptSubmit"`): opens a prompt + execution chained to
//! the previous prompt, closing a prompt whose Stop hook never arrived.

use obs_cli::python_compat::{python_utc_now, uuid7_text};
use rusqlite::params;
use serde_json::{json, Value};

use super::behaviour_audit::{audit_state_signal, is_enabled, wait_threshold};
use super::event_records::{event_emit_data, Signal, TraceSlot};
use super::fingerprints::random_hex;
use super::hook_scope::HookScope;
use super::instruction_sources::{instruction_load_state, InstructionState};
use super::operations::operation;
use super::payload_fields::{is_present, optional_json, text_or};
use super::project_registration::ProjectIdentity;
use super::session_chain::ContextRow;
use super::spool_store::Spool;

const NATIVE_TURN_KEYS: [&str; 4] = ["turn_id", "turnId", "prompt_id", "promptId"];
const RAW_PROMPT_KEYS: [&str; 3] = ["prompt", "user_prompt", "userPrompt"];
const NATIVE_TURN_LIMIT: usize = 240;
const TRACE_ID_BYTES: usize = 16;
const SUPERSEDED_SUMMARY: &str = "Superseded by the next prompt; Stop hook was not observed.";

/// Identifiers of the prompt being opened.
struct PromptOpening {
    ordinal: i64,
    prompt_id: String,
    execution_id: String,
    trace_id: String,
    native_turn: String,
    audit_enabled: bool,
}

/// Mental model: skip a replayed turn → supersede an open prompt → open prompt/execution with its
/// audit + instruction signals → persist the new chain head.
pub fn begin_prompt(
    scope: &HookScope,
    spool: &Spool,
    project: &ProjectIdentity,
    row: &ContextRow,
    operations: &mut Vec<Value>,
) -> anyhow::Result<()> {
    let native_turn = text_or(scope.payload, &NATIVE_TURN_KEYS, "", NATIVE_TURN_LIMIT);
    if is_replayed_turn(row, &native_turn) {
        return Ok(());
    }
    let opening = PromptOpening {
        ordinal: row.prompt_ordinal + 1,
        prompt_id: uuid7_text(),
        execution_id: uuid7_text(),
        trace_id: random_hex(TRACE_ID_BYTES),
        native_turn,
        audit_enabled: is_enabled(scope.context),
    };

    if row.has_live_execution() && is_present(row.prompt_id.as_deref()) {
        operations.extend(supersede_open_prompt(project, row));
    }
    operations.extend(open_prompt_operations(
        scope, spool, project, row, &opening,
    )?);
    persist_chain_head(scope, spool, &opening)
}

/// `native_turn and row["native_turn"] == native_turn and row["execution_id"]`.
#[inline]
fn is_replayed_turn(row: &ContextRow, native_turn: &str) -> bool {
    !native_turn.is_empty()
        && row.native_turn.as_deref() == Some(native_turn)
        && row.has_live_execution()
}

fn supersede_open_prompt(project: &ProjectIdentity, row: &ContextRow) -> Vec<Value> {
    let execution_id = row.execution_id_json();
    let trace_id = row.trace_id_json();
    let slot = TraceSlot {
        execution_id: &execution_id,
        project_id: &project.id,
        trace_id: &trace_id,
    };
    let signal = Signal {
        code: "agent.prompt.stop_missing",
        category: 1,
        severity: 2,
        outcome: 3,
        summary: "Previous prompt was superseded without a Stop hook.".to_owned(),
        attributes: json!({}),
    };
    let event = event_emit_data(&uuid7_text(), &slot, row.sequence_no + 1, signal);
    vec![
        operation("event.emit", Value::Object(event), None),
        operation(
            "execution.finish",
            json!({"executionId": execution_id, "status": 4, "outcome": 3, "resultSummary": SUPERSEDED_SUMMARY}),
            None,
        ),
        operation(
            "prompt.finish",
            json!({"promptId": row.prompt_id_json(), "status": 4, "resultSummary": SUPERSEDED_SUMMARY}),
            None,
        ),
    ]
}

/// prompt.begin → execution.start → audit_state (seq 1) → load_state (seq 2) → [native-link] → [acknowledged (seq 3)].
fn open_prompt_operations(
    scope: &HookScope,
    spool: &Spool,
    project: &ProjectIdentity,
    row: &ContextRow,
    opening: &PromptOpening,
) -> anyhow::Result<Vec<Value>> {
    let raw_prompt = text_or(scope.payload, &RAW_PROMPT_KEYS, "", usize::MAX);
    let execution_id = Value::String(opening.execution_id.clone());
    let trace_id = Value::String(opening.trace_id.clone());
    let slot = TraceSlot {
        execution_id: &execution_id,
        project_id: &project.id,
        trace_id: &trace_id,
    };
    let instructions = instruction_load_state(
        &scope.context.harness_root,
        &scope.instance_key,
        &scope.cwd_path,
    );

    let mut operations = vec![
        operation(
            "prompt.begin",
            json!({
                "promptId": opening.prompt_id, "sessionId": row.session_id,
                "parentPromptId": optional_json(row.last_prompt_id.as_deref()), "ordinal": opening.ordinal,
                "promptHmac": spool.keyed_digest(&raw_prompt)?, "rawPrompt": raw_prompt, "languageCode": "tl-en",
            }),
            None,
        ),
        operation(
            "execution.start",
            json!({
                "executionId": opening.execution_id, "projectId": project.id, "sessionId": row.session_id,
                "promptId": opening.prompt_id, "executionKind": 1, "attemptNo": 1, "traceId": opening.trace_id,
            }),
            None,
        ),
        emit(
            &slot,
            1,
            audit_state_signal(opening.audit_enabled, wait_threshold(scope.context)),
            None,
        ),
        instruction_state_operation(&slot, &scope.instance_key, instructions),
    ];
    if !opening.native_turn.is_empty() {
        operations.push(operation(
            "prompt.native-link",
            json!({
                "projectId": project.id, "promptId": opening.prompt_id,
                "nativeSession": scope.native_session, "nativeTurn": opening.native_turn,
            }),
            None,
        ));
    }
    if opening.ordinal == 1 {
        operations.push(emit(
            &slot,
            3,
            acknowledgement_signal(&scope.instance_key, project),
            None,
        ));
    }
    Ok(operations)
}

fn instruction_state_operation(
    slot: &TraceSlot,
    instance_key: &str,
    state: InstructionState,
) -> Value {
    let (severity, outcome, verdict) = if state.loaded {
        (1, 1, "loaded")
    } else {
        (4, 3, "missing")
    };
    let signal = Signal {
        code: "agent.instructions.load_state",
        category: 1,
        severity,
        outcome,
        summary: format!("Global instruction source {verdict} for {instance_key}."),
        attributes: state.document.clone(),
    };
    emit(slot, 2, signal, Some(state.document))
}

fn acknowledgement_signal(instance_key: &str, project: &ProjectIdentity) -> Signal<'static> {
    Signal {
        code: "agent.instructions.acknowledged",
        category: 1,
        severity: 1,
        outcome: 1,
        summary: "Session instructions acknowledged.".to_owned(),
        attributes: json!({"instance": instance_key, "scope": project.scope_label()}),
    }
}

#[inline]
fn emit(slot: &TraceSlot, sequence_no: i64, signal: Signal, raw_payload: Option<Value>) -> Value {
    let mut data = event_emit_data(&uuid7_text(), slot, sequence_no, signal);
    if let Some(raw_payload) = raw_payload {
        data.insert("rawPayload".to_owned(), raw_payload);
    }
    operation("event.emit", Value::Object(data), None)
}

/// `starting_sequence = 3 if ordinal == 1 else 2`; counters reset for the new prompt.
fn persist_chain_head(
    scope: &HookScope,
    spool: &Spool,
    opening: &PromptOpening,
) -> anyhow::Result<()> {
    let starting_sequence = if opening.ordinal == 1 { 3 } else { 2 };
    let native_turn = (!opening.native_turn.is_empty()).then_some(opening.native_turn.as_str());
    spool.conn().execute(
        "UPDATE context SET prompt_id=?,execution_id=?,trace_id=?,last_prompt_id=?,native_turn=?,prompt_ordinal=?,sequence_no=?,wait_count=0,background_count=0,behavior_audit_enabled=?,cwd=?,updated_at=? WHERE native_session=?",
        params![
            opening.prompt_id,
            opening.execution_id,
            opening.trace_id,
            opening.prompt_id,
            native_turn,
            opening.ordinal,
            starting_sequence,
            i64::from(opening.audit_enabled),
            scope.cwd_text(),
            python_utc_now(),
            scope.native_session,
        ],
    )?;
    Ok(())
}
