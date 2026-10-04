//! Operation batches: validation, then one handler per operation concept, all inside a
//! single transaction so a failing operation rolls back the whole batch (server.mjs:384-402).

pub mod batch_validation;
pub mod fields;
pub mod js_values;
pub mod json_body;

mod benchmarks;
mod code_changes;
mod error_tracking;
mod events;
mod executions;
mod herdr_runtime;
mod markdown_context;
mod project_context;
mod projects;
mod prompts;
mod sessions;
mod usage;

use anyhow::{bail, Result};
use rusqlite::{Connection, TransactionBehavior};
use serde_json::{Map, Value};

use fields::Fields;

pub use batch_validation::{client_error_message, parse_operation_batch, ValidationIssue};

/// One validated operation; `occurred_at` is already normalized UTC millisecond text.
#[derive(Debug, Clone)]
pub struct Operation {
    pub operation_id: String,
    pub kind: String,
    pub occurred_at: String,
    pub data: Map<String, Value>,
}

impl Operation {
    pub fn fields(&self) -> Fields<'_> {
        Fields::new(&self.data)
    }
}

// Mental model:
// begin immediate → apply every operation in order → commit, or roll back on the first failure
pub fn apply_batch(connection: &mut Connection, operations: &[Operation]) -> Result<usize> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;

    for operation in operations {
        apply_operation(&transaction, operation)?;
    }

    transaction.commit()?;
    Ok(operations.len())
}

fn apply_operation(connection: &Connection, operation: &Operation) -> Result<()> {
    match operation.kind.as_str() {
        "project.register" => projects::register_project(connection, operation),
        "agent.register" => projects::register_agent(connection, operation),
        "session.start" => sessions::start_session(connection, operation),
        "session.end" => sessions::end_session(connection, operation),
        "prompt.begin" => prompts::begin_prompt(connection, operation),
        "prompt.enrich" => prompts::enrich_prompt(connection, operation),
        "prompt.finish" => prompts::finish_prompt(connection, operation),
        "prompt.native-link" => prompts::link_native_turn(connection, operation),
        "execution.start" => executions::start_execution(connection, operation),
        "execution.finish" => executions::finish_execution(connection, operation),
        "execution.code-version" => executions::attach_code_version(connection, operation),
        "event.emit" => events::emit_event(connection, operation),
        "context.record" => project_context::record_context(connection, operation),
        "herdr.binding" => herdr_runtime::bind_pane(connection, operation),
        "agent-state.record" => herdr_runtime::record_agent_state(connection, operation),
        "herdr-event.record" => herdr_runtime::record_runtime_event(connection, operation),
        "turn-usage.record" => usage::record_turn_usage(connection, operation),
        "tool-use.record" => usage::record_tool_use(connection, operation),
        "markdown-context.record" => markdown_context::record(connection, operation),
        "handoff.record" => project_context::record_handoff(connection, operation),
        "handoff.promote" => project_context::promote_handoff(connection, operation),
        "pane-evidence.record" => herdr_runtime::record_pane_evidence(connection, operation),
        "code-version.register" => code_changes::register_code_version(connection, operation),
        "attribution.record" => code_changes::record_attribution(connection, operation),
        "change-assessment.plan" => code_changes::plan_change(connection, operation),
        "change-assessment.finish" => code_changes::finish_change(connection, operation),
        "change-assessment.package" => code_changes::package_changes(connection, operation),
        "benchmark.init" => benchmarks::initialize_program(connection, operation),
        "benchmark.observe" => benchmarks::record_observation(connection, operation),
        "benchmark.run" => benchmarks::record_run(connection, operation),
        "error.record" => error_tracking::record_error(connection, operation),
        other => bail!("unsupported operation kind: {other}"),
    }
}
