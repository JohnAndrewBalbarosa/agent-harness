//! `obs` consumer: the in-process port of `core/consumers/obs.py` (identity wrapper) and
//! `tools/observability-client/hook.py` (the whole hook flow) on top of obs.py's spool.
//!
//! Mental model:
//! native payload → hook scope (event, native session, cwd, instance key)
//! → project registration (marker or global project) → session chain (agent + session rows)
//! → event phase (prompt begin / tool activity / turn completion / session end)
//! → one spool transaction → one detached flush → bounded invocation audit.
//!
//! Deliberate deviation (spec "obs: no synchronous flush"): obs.py's `enqueue(flush_now=True)` inside
//! `register_project`/`global_project` and hook.py's `schedule_flush()` are replaced by enqueue-only plus
//! one detached `bin/obs-rs(.exe) flush` (Python `flush_spool.py` when obs-rs is absent).

pub mod agent_identity;
pub mod behaviour_audit;
pub mod code_version;
pub mod event_records;
pub mod fingerprints;
pub mod flush_trigger;
pub mod hook_scope;
pub mod instruction_sources;
pub mod invocation_audit;
pub mod language_policy;
pub mod operations;
pub mod origin_remote;
pub mod payload_fields;
pub mod project_registration;
pub mod prompt_lifecycle;
pub mod session_chain;
pub mod spool_store;
pub mod tool_activity;
pub mod turn_completion;

use std::time::{Duration, Instant};

use serde_json::Value;

use crate::consumer::{Consumer, ConsumerResult};
use crate::hook_context::HookContext;

use self::hook_scope::HookScope;
use self::operations::operation;
use self::payload_fields::Payload;
use self::project_registration::{ProjectIdentity, ProjectRegistration};
use self::session_chain::ContextRow;
use self::spool_store::{Spool, SpoolLocation};

const CONSUMER_NAME: &str = "obs";
/// `core/consumers/obs.py` `TIMEOUT_S`.
const CONSUMER_TIMEOUT: Duration = Duration::from_secs(20);
const UNSUBSCRIBED_EVENTS: [&str; 3] = ["subagent.start", "subagent.stop", "attention.needed"];

pub struct ObsConsumer;

impl Consumer for ObsConsumer {
    fn name(&self) -> &'static str {
        CONSUMER_NAME
    }

    fn subscribes_to(&self, event: &str) -> bool {
        !UNSUBSCRIBED_EVENTS.contains(&event)
    }

    fn timeout(&self) -> Duration {
        CONSUMER_TIMEOUT
    }

    /// hook.py `main()`: never fails its agent. Failures only reach the invocation audit
    /// (the Python wrapper discards the hook's stdout and always exits 0).
    fn run(&self, context: &HookContext) -> anyhow::Result<ConsumerResult> {
        let started = Instant::now();
        let payload = native_payload_object(context);
        let location = SpoolLocation::resolve(context);
        let audit_event = payload_fields::text_or(
            &payload,
            &["hook_event_name", "hookEventName"],
            "unknown",
            usize::MAX,
        );

        let outcome = {
            let _spool_access = crate::shared_state::spool_access::lock_spool_access();
            process_hook(context, &payload, &location)
        };
        invocation_audit::record(
            &location.var_dir,
            &audit_event,
            started,
            outcome.as_ref().err(),
        );
        Ok(ConsumerResult::succeeded(CONSUMER_NAME, ""))
    }
}

/// hook.py `process(value)`.
///
/// Mental model: scope → project → spool → session chain → event phase → enqueue → flush.
pub fn process_hook(
    context: &HookContext,
    payload: &Payload,
    location: &SpoolLocation,
) -> anyhow::Result<()> {
    let scope = HookScope::from_hook(context, payload);
    let ProjectRegistration {
        project,
        operation: register_operation,
    } = project_registration::register(&scope.cwd_path)?;
    let spool = Spool::open(location, spool_store::queue_cap(context))?;

    // hook.py enqueues `project.register` in its own call just before; here it leads the single batch.
    let mut operations = vec![register_operation];
    let row = session_chain::attach(&scope, &spool, &project, &mut operations)?;
    run_event_phase(&scope, &spool, &project, &row, &mut operations)?;

    spool.enqueue(&operations)?;
    flush_trigger::schedule(context, location);
    Ok(())
}

/// The `if event == ...` chain of hook.py `process`.
fn run_event_phase(
    scope: &HookScope,
    spool: &Spool,
    project: &ProjectIdentity,
    row: &ContextRow,
    operations: &mut Vec<Value>,
) -> anyhow::Result<()> {
    match scope.hook_event.as_str() {
        "UserPromptSubmit" => {
            prompt_lifecycle::begin_prompt(scope, spool, project, row, operations)
        }
        "PostToolUse" if row.has_live_execution() => {
            tool_activity::record_tool_use(scope, spool, project, row, operations)
        }
        "Stop" if row.has_live_execution() => {
            turn_completion::finish_turn(scope, spool, project, row, operations)
        }
        "SessionEnd" => {
            operations.push(operation(
                "session.end",
                serde_json::json!({"sessionId": row.session_id}),
                None,
            ));
            Ok(())
        }
        _ => Ok(()),
    }
}

/// hook.py `payload()`: a non-object payload is treated as `{}`.
#[inline]
fn native_payload_object(context: &HookContext) -> Payload {
    context
        .native_payload
        .as_object()
        .cloned()
        .unwrap_or_default()
}
