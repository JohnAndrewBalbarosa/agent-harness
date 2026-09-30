//! Herdr bridge hook side (`tools/herdr-bridge/hook.py`): keeps `var/herdr-bridge/sessions/<id>.json`, projects
//! the canonical agent state onto the Herdr pane through the `herdr` CLI, queues the binding / state / usage /
//! evidence observability records, and on `SessionStart` prints the hub's project context envelope.
//!
//! Mental model:
//! native payload → identity (session, turn, project, pane) → [inside Herdr: ensure subscriber]
//! → merged session record written → observability records queued → [inside Herdr: report session / state,
//! capture evidence on transitions] → `SessionEnd`: release pane + mark inactive → `SessionStart`: context
//! envelope on stdout → queued records delivered to the spool (always, even after a failure).

pub mod background_launch;
pub mod bridge_log;
pub mod evidence;
pub mod herdr_cli;
pub mod identity;
pub mod observability_outbox;
pub mod project_context;
pub mod session_state;
pub mod state_machine;
pub mod usage;

use std::time::{SystemTime, UNIX_EPOCH};

use obs_cli::python_compat::{to_python_json, with_platform_newlines, JsonLayout};
use serde_json::{json, Map, Value};

use crate::consumer::{Consumer, ConsumerResult};
use crate::events::{is_known_event, is_python_truthy, python_str};
use crate::hook_context::HookContext;

use bridge_log::BridgeLog;
use evidence::{capture_evidence, EvidenceRequest};
use herdr_cli::{
    herdr_binary, HerdrRunner, ProcessHerdrRunner, DEFAULT_OUTPUT_LIMIT, DEFAULT_TIMEOUT,
};
use identity::{payload_value, profile_name, project_id_for, safe_id};
use observability_outbox::ObservabilityOutbox;
use session_state::{
    is_state_change, merged_record, next_sequence, read_state, session_state_path, write_state,
    SessionIdentity,
};
use state_machine::{decision_for_hook, StateDecision};

const NAME: &str = "herdr-bridge";
const HERDR_SCOPE: &str = "herdr";
const OUTSIDE_SCOPE: &str = "outside_herdr";
const STATE_DETAIL_LIMIT: usize = 300;

pub struct HerdrBridgeConsumer;

impl Consumer for HerdrBridgeConsumer {
    fn name(&self) -> &'static str {
        NAME
    }

    /// `core/router/consumers.py` `_herdr_bridge`: every common event except the subagent pair.
    fn subscribes_to(&self, event: &str) -> bool {
        is_known_event(event) && !event.starts_with("subagent.")
    }

    /// Only inside a Herdr pane: `HERDR_ENV == "1"` and a non-empty `HERDR_PANE_ID`.
    fn is_enabled(&self, context: &HookContext) -> bool {
        context.env.get("HERDR_ENV").map(String::as_str) == Some("1")
            && context.env_value("HERDR_PANE_ID").is_some()
    }

    fn run(&self, context: &HookContext) -> anyhow::Result<ConsumerResult> {
        let log = BridgeLog::under_root(&context.harness_root);
        let runner = ProcessHerdrRunner::new(herdr_binary(&context.env), &log);
        Ok(ConsumerResult::succeeded(
            NAME,
            handle_hook_with(context, &runner, &log)?,
        ))
    }
}

/// One bridge hook with an injectable `herdr` runner; returns the stdout (`SessionStart` envelope or `""`).
pub fn handle_hook_with(
    context: &HookContext,
    runner: &dyn HerdrRunner,
    log: &BridgeLog,
) -> anyhow::Result<String> {
    let hook = BridgeHook::from_context(context);
    if hook.is_inside_herdr() {
        background_launch::ensure_subscriber(&context.harness_root, &context.env, log);
    }
    let mut outbox = ObservabilityOutbox::default();
    let result = drive_hook(context, &hook, runner, log, &mut outbox);
    outbox.deliver(&context.harness_root, &context.env, log);
    result
}

/// Everything `main()` derives from the payload and environment before acting.
struct BridgeHook {
    payload: Map<String, Value>,
    event: String,
    identity: SessionIdentity,
}

impl BridgeHook {
    fn from_context(context: &HookContext) -> Self {
        let payload = context
            .native_payload
            .as_object()
            .cloned()
            .unwrap_or_default();
        let event = payload_value(&payload, &["hook_event_name", "hookEventName"])
            .map(python_str)
            .unwrap_or_else(|| {
                context
                    .env
                    .get("CODEX_STATUS_EVENT")
                    .cloned()
                    .unwrap_or_default()
            });
        let cwd = payload_value(&payload, &["cwd", "working_directory"])
            .map(python_str)
            .unwrap_or_else(current_dir_text);
        let pane_id = env_trimmed(context, "HERDR_PANE_ID");
        let is_herdr =
            !pane_id.is_empty() && context.env.get("HERDR_ENV").map(String::as_str) == Some("1");

        let identity = SessionIdentity {
            native_session: safe_id(
                payload_value(&payload, &["session_id", "sessionId"]),
                "unknown",
            ),
            native_turn: safe_id(payload_value(&payload, &["turn_id", "turnId"]), ""),
            project_id: project_id_for(&cwd),
            profile: profile_name(&context.env),
            cwd,
            runtime_scope: if is_herdr { HERDR_SCOPE } else { OUTSIDE_SCOPE },
            workspace_id: env_trimmed(context, "HERDR_WORKSPACE_ID"),
            tab_id: env_trimmed(context, "HERDR_TAB_ID"),
            pane_id,
        };
        Self {
            payload,
            event,
            identity,
        }
    }

    #[inline]
    fn is_inside_herdr(&self) -> bool {
        self.identity.runtime_scope == HERDR_SCOPE
    }
}

// Mental model: persist → queue records → project to Herdr → SessionEnd bookkeeping → SessionStart context.
fn drive_hook(
    context: &HookContext,
    hook: &BridgeHook,
    runner: &dyn HerdrRunner,
    log: &BridgeLog,
    outbox: &mut ObservabilityOutbox,
) -> anyhow::Result<String> {
    let state_path = session_state_path(&context.harness_root, &hook.identity.native_session);
    let previous = read_state(&state_path);
    let decision = decision_for_hook(&hook.event, &hook.payload);
    let sequence = next_sequence(&previous);

    let mut record = merged_record(
        &previous,
        &hook.identity,
        decision.as_ref(),
        sequence,
        now_seconds(),
    );
    write_state(&state_path, &record)?;

    queue_records(hook, &previous, decision.as_ref(), sequence, outbox);
    if hook.is_inside_herdr() {
        let reported = project_to_herdr(
            hook,
            &previous,
            decision.as_ref(),
            sequence,
            runner,
            log,
            outbox,
        );
        if let Some(report) = reported {
            remember_report(&mut record, &report);
            write_state(&state_path, &record)?;
        }
    }

    if hook.event == "SessionEnd" {
        if hook.is_inside_herdr() {
            runner.run(
                &owned(&[
                    "pane",
                    "release-agent",
                    &hook.identity.pane_id,
                    "--source",
                    "shared:codex-state",
                    "--agent",
                    "codex",
                ]),
                DEFAULT_TIMEOUT,
                DEFAULT_OUTPUT_LIMIT,
            );
        }
        record.insert("active".into(), Value::Bool(false));
        write_state(&state_path, &record)?;
    }

    Ok(session_start_output(context, hook))
}

/// `herdr.binding` always; `agent-state.record` for a decision; `turn-usage.record` on `Stop`.
fn queue_records(
    hook: &BridgeHook,
    previous: &Map<String, Value>,
    decision: Option<&StateDecision>,
    sequence: i64,
    outbox: &mut ObservabilityOutbox,
) {
    let identity = &hook.identity;
    outbox.record(
        "herdr.binding",
        json!({
            "bindingId": new_uuid(), "projectId": identity.project_id, "nativeSession": identity.native_session,
            "profileName": identity.profile, "workspaceId": non_empty(&identity.workspace_id),
            "tabId": non_empty(&identity.tab_id), "paneId": non_empty(&identity.pane_id),
            "runtimeScope": identity.runtime_scope, "cwd": identity.cwd,
        }),
    );
    if let Some(decision) = decision {
        outbox.record(
            "agent-state.record",
            agent_state_data(identity, previous, decision, sequence),
        );
    }
    if hook.event == "Stop" {
        outbox.record("turn-usage.record", turn_usage_data(hook, sequence));
    }
}

fn agent_state_data(
    identity: &SessionIdentity,
    previous: &Map<String, Value>,
    decision: &StateDecision,
    sequence: i64,
) -> Value {
    json!({
        "stateEventId": new_uuid(), "projectId": identity.project_id, "nativeSession": identity.native_session,
        "nativeTurn": non_empty(&identity.native_turn), "profileName": identity.profile,
        "previousState": previous.get("canonical_state").cloned().unwrap_or(Value::Null),
        "canonicalState": decision.canonical, "projectedState": decision.projected, "reason": decision.reason,
        "authority": decision.authority, "signalSource": decision.signal,
        "sequenceNo": sequence, "conflict": false,
        "attributes": {"runtimeScope": identity.runtime_scope, "paneId": non_empty(&identity.pane_id)},
    })
}

/// Kept as Python: `source` stays `codex_hook` for every agent (changing it would collide with the
/// `claude_hook` rows written by usage-fallback).
fn turn_usage_data(hook: &BridgeHook, sequence: i64) -> Value {
    let identity = &hook.identity;
    let (usage, availability) = usage::exact_usage(&hook.payload);
    let mut data = Map::new();
    data.insert("usageId".into(), json!(new_uuid()));
    data.insert("projectId".into(), json!(identity.project_id));
    data.insert("nativeSession".into(), json!(identity.native_session));
    data.insert("nativeTurn".into(), non_empty(&identity.native_turn));
    data.insert("promptId".into(), Value::Null);
    data.extend(usage);
    data.insert("availability".into(), json!(availability));
    data.insert("source".into(), json!("codex_hook"));
    data.insert(
        "sourceEventId".into(),
        json!(format!(
            "{}:{}:{}:{sequence}",
            identity.native_session, identity.native_turn, hook.event
        )),
    );
    Value::Object(data)
}

/// What was last sent to Herdr for this session (kept in the session state file).
struct HerdrReport {
    projected: String,
    message: String,
    is_ok: bool,
}

const REPORTED_STATE_KEY: &str = "herdr_reported_state";
const REPORTED_MESSAGE_KEY: &str = "herdr_reported_message";
const REPORTED_AT_KEY: &str = "herdr_reported_at";
const REPORT_OK_KEY: &str = "herdr_report_ok";
/// Herdr still gets a report at least this often while nothing changes (liveness).
const REPORT_HEARTBEAT_SECONDS: f64 = 30.0;

/// A report is due when the projected state or message changed, the last report failed, or the
/// heartbeat interval passed. (Python reported on every hook: one `herdr` process per tool call.)
fn is_report_due(previous: &Map<String, Value>, projected: &str, message: &str, now: f64) -> bool {
    let is_same_state = previous.get(REPORTED_STATE_KEY).and_then(Value::as_str) == Some(projected);
    let is_same_message =
        previous.get(REPORTED_MESSAGE_KEY).and_then(Value::as_str) == Some(message);
    let was_ok = previous.get(REPORT_OK_KEY).and_then(Value::as_bool) == Some(true);
    let is_fresh = previous
        .get(REPORTED_AT_KEY)
        .and_then(Value::as_f64)
        .is_some_and(|reported_at| now - reported_at < REPORT_HEARTBEAT_SECONDS);
    !(is_same_state && is_same_message && was_ok && is_fresh)
}

fn remember_report(record: &mut Map<String, Value>, report: &HerdrReport) {
    record.insert(REPORTED_STATE_KEY.into(), json!(report.projected));
    record.insert(REPORTED_MESSAGE_KEY.into(), json!(report.message));
    record.insert(REPORTED_AT_KEY.into(), json!(now_seconds()));
    record.insert(REPORT_OK_KEY.into(), json!(report.is_ok));
}

// Mental model: SessionStart → report-agent-session; decision → report-agent only when due (logged) →
// evidence when the canonical state changed or the projection failed.
fn project_to_herdr(
    hook: &BridgeHook,
    previous: &Map<String, Value>,
    decision: Option<&StateDecision>,
    sequence: i64,
    runner: &dyn HerdrRunner,
    log: &BridgeLog,
    outbox: &mut ObservabilityOutbox,
) -> Option<HerdrReport> {
    let identity = &hook.identity;
    let pane = identity.pane_id.as_str();
    let sequence_text = sequence.to_string();
    if hook.event == "SessionStart" {
        runner.run(
            &owned(&[
                "pane",
                "report-agent-session",
                pane,
                "--source",
                "herdr:codex",
                "--agent",
                "codex",
                "--seq",
                &sequence_text,
                "--agent-session-id",
                &identity.native_session,
            ]),
            DEFAULT_TIMEOUT,
            DEFAULT_OUTPUT_LIMIT,
        );
    }
    let decision = decision?;

    let message = format!("{}: {}", decision.canonical, decision.reason);
    let is_state_change = is_state_change(previous, decision);
    if !is_state_change && !is_report_due(previous, decision.projected, &message, now_seconds()) {
        return None;
    }
    let projected = runner.run(
        &owned(&[
            "pane",
            "report-agent",
            pane,
            "--source",
            "shared:codex-state",
            "--agent",
            "codex",
            "--state",
            decision.projected,
            "--message",
            &message,
            "--seq",
            &sequence_text,
        ]),
        DEFAULT_TIMEOUT,
        DEFAULT_OUTPUT_LIMIT,
    );
    let is_projected = projected.exit_code == 0;
    let detail: String = projected.output.chars().take(STATE_DETAIL_LIMIT).collect();
    log.event(
        "state.projected",
        if is_projected { "succeeded" } else { "failed" },
        vec![
            ("session", json!(identity.native_session)),
            ("state", json!(decision.canonical)),
            ("projected", json!(decision.projected)),
            ("pane", json!(pane)),
            ("detail", json!(detail)),
        ],
    );

    if !identity.project_id.is_empty() && (is_state_change || !is_projected) {
        let request = EvidenceRequest {
            project_id: &identity.project_id,
            native_session: &identity.native_session,
            pane_id: pane,
            trigger: if is_projected {
                "state.transition"
            } else {
                "state.projection_failed"
            },
        };
        capture_evidence(&request, runner, outbox);
    }
    Some(HerdrReport {
        projected: decision.projected.to_owned(),
        message,
        is_ok: is_projected,
    })
}

/// `print(json.dumps({"hookSpecificOutput": {...}}, ensure_ascii=False))` when the hub has context.
fn session_start_output(context: &HookContext, hook: &BridgeHook) -> String {
    if hook.event != "SessionStart" || hook.identity.project_id.is_empty() {
        return String::new();
    }
    let source = hook
        .payload
        .get("source")
        .filter(|value| is_python_truthy(value))
        .map(python_str)
        .unwrap_or_else(|| "startup".to_owned());
    let endpoint = context
        .env_value("OBS_ENDPOINT")
        .unwrap_or(project_context::DEFAULT_ENDPOINT);
    let text = project_context::build_context(
        endpoint,
        &hook.identity.project_id,
        &hook.identity.profile,
        &source,
    );
    if text.is_empty() {
        return String::new();
    }
    session_start_envelope(&text)
}

/// The exact stdout bytes, including `print`'s trailing newline (text-mode stdout: CRLF on Windows).
pub fn session_start_envelope(additional_context: &str) -> String {
    let envelope = json!({"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": additional_context}});
    let text = format!("{}\n", to_python_json(&envelope, JsonLayout::Spaced, false));
    with_platform_newlines(&text)
}

#[inline]
fn env_trimmed(context: &HookContext, key: &str) -> String {
    context
        .env
        .get(key)
        .map(|value| value.trim().to_owned())
        .unwrap_or_default()
}

#[inline]
fn current_dir_text() -> String {
    std::env::current_dir()
        .map(|path| path.display().to_string())
        .unwrap_or_default()
}

/// `value or None`.
#[inline]
fn non_empty(value: &str) -> Value {
    if value.is_empty() {
        Value::Null
    } else {
        Value::from(value)
    }
}

#[inline]
fn new_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[inline]
fn owned(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_owned()).collect()
}

/// `time.time()`.
#[inline]
fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod report_throttle_tests {
    use super::*;

    fn previous(state: &str, message: &str, at: f64, ok: bool) -> Map<String, Value> {
        let mut record = Map::new();
        remember_report(
            &mut record,
            &HerdrReport {
                projected: state.to_owned(),
                message: message.to_owned(),
                is_ok: ok,
            },
        );
        record.insert(REPORTED_AT_KEY.into(), json!(at));
        record
    }

    #[test]
    fn unchanged_fresh_successful_report_is_skipped() {
        let last = previous("working", "working: tool completed", 1000.0, true);
        assert!(!is_report_due(
            &last,
            "working",
            "working: tool completed",
            1010.0
        ));
    }

    #[test]
    fn changes_failures_and_heartbeat_trigger_a_report() {
        let last = previous("working", "working: tool completed", 1000.0, true);
        assert!(is_report_due(&last, "done", "ready: turn finished", 1001.0));
        assert!(is_report_due(
            &last,
            "working",
            "working: prompt submitted",
            1001.0
        ));
        assert!(is_report_due(
            &last,
            "working",
            "working: tool completed",
            1031.0
        ));
        let failed = previous("working", "working: tool completed", 1000.0, false);
        assert!(is_report_due(
            &failed,
            "working",
            "working: tool completed",
            1001.0
        ));
        assert!(is_report_due(
            &Map::new(),
            "working",
            "working: tool completed",
            1001.0
        ));
    }
}
