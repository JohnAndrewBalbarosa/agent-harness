//! turn.stop consumer (`core/usage/fallback.py`): per-turn token usage from the agent's transcript →
//! `turn-usage.record` (`source='<agent>_hook'`).
//!
//! Complements OTel (`<agent>_otel`): it carries reasoning tokens and exact per-turn totals for agents
//! whose OTel events have no turn id. A bounded local copy is kept in `<root>/var/usage/turn-usage.jsonl`
//! (`AH_USAGE_DIR`), locked through `turn-usage.jsonl.lock`.

pub mod spool_enqueue;
pub mod usage_record;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use chrono::Utc;
use obs_cli::python_compat::{to_python_json, JsonLayout};
use serde_json::Value;

use self::usage_record::{build_record, global_project_id, local_log_record};
use crate::consumer::{Consumer, ConsumerResult};
use crate::consumers::notify::bounded_jsonl::append_keeping_newest;
use crate::events::Event;
use crate::hook_context::HookContext;
use crate::shared_state::file_lock::with_sidecar_lock;
use crate::transcripts::TranscriptAgent;

pub const NAME: &str = "usage-fallback";
pub const LOCAL_LOG_FILE: &str = "turn-usage.jsonl";
pub const LOG_MAX_RECORDS: usize = 500;

pub struct UsageFallbackConsumer;

impl Consumer for UsageFallbackConsumer {
    fn name(&self) -> &'static str {
        NAME
    }

    fn subscribes_to(&self, event: &str) -> bool {
        event == "turn.stop"
    }

    fn run(&self, context: &HookContext) -> anyhow::Result<ConsumerResult> {
        handle(&context.event, &context.env, &context.harness_root)?;
        Ok(ConsumerResult::succeeded(NAME, ""))
    }
}

/// `handle(event, env)`: the enqueued record, or `None` when the event carries no usable transcript
/// (not a stop, no transcript path, or an agent without a transcript reader).
pub fn handle(
    event: &Event,
    env: &BTreeMap<String, String>,
    harness_root: &Path,
) -> anyhow::Result<Option<Value>> {
    // Mental model: transcript → usage → record (project from the spool) → enqueue + detached flush
    // (unless dry-run) → bounded local copy.
    if event.event != "turn.stop" {
        return Ok(None);
    }
    let Some(transcript) = event.transcript_path.as_deref() else {
        return Ok(None);
    };
    let Some(reader) = TranscriptAgent::from_agent(&event.agent) else {
        return Ok(None);
    };

    let usage = reader.turn_usage(Path::new(transcript), event.turn_id.as_deref());
    let spool = spool_enqueue::spool_path(env, harness_root);
    let _spool_access = crate::shared_state::spool_access::lock_spool_access();
    let project_id = spool_enqueue::spool_project(&spool, &event.session_id)
        .filter(|project| !project.is_empty())
        .or_else(|| marker_project(event.cwd.as_deref()))
        .unwrap_or_else(global_project_id);
    let record = build_record(event, &usage, &project_id);

    let enqueued =
        !is_dry_run(env) && enqueue_and_schedule_flush(&record, env, harness_root, &spool)?;
    let local = local_log_record(&record, event, &usage, enqueued, Utc::now());
    append_local(&usage_dir(env, harness_root), &local)?;
    Ok(Some(record))
}

/// `AH_USAGE_DRY_RUN=1` skips the spool (the local copy is still written with `enqueued: false`).
#[inline]
fn is_dry_run(env: &BTreeMap<String, String>) -> bool {
    env.get("AH_USAGE_DRY_RUN").map(String::as_str) == Some("1")
}

#[inline]
fn enqueue_and_schedule_flush(
    record: &Value,
    env: &BTreeMap<String, String>,
    harness_root: &Path,
    spool: &Path,
) -> anyhow::Result<bool> {
    spool_enqueue::enqueue_usage(spool, record, env)
        .with_context(|| format!("unable to enqueue turn usage into {}", spool.display()))?;
    spool_enqueue::schedule_flush(env, harness_root, spool);
    Ok(true)
}

/// `AH_USAGE_DIR` or `<root>/var/usage`.
#[inline]
fn usage_dir(env: &BTreeMap<String, String>, harness_root: &Path) -> PathBuf {
    env.get("AH_USAGE_DIR")
        .filter(|directory| !directory.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| harness_root.join("var").join("usage"))
}

/// `_append_local(directory, record)`: newest 500 records under the file's own sidecar lock.
fn append_local(directory: &Path, record: &Value) -> anyhow::Result<()> {
    std::fs::create_dir_all(directory)
        .with_context(|| format!("unable to create {}", directory.display()))?;
    let path = directory.join(LOCAL_LOG_FILE);
    let line = to_python_json(record, JsonLayout::Spaced, false);
    with_sidecar_lock(&path, || {
        append_keeping_newest(&path, LOG_MAX_RECORDS, &line)
    })
    .with_context(|| format!("unable to append {}", path.display()))
}

/// The obs consumer clears the session's context row at `turn.stop`, possibly before this consumer
/// reads it; the project marker of `cwd` is what obs registered for that session, so it is the
/// deterministic fallback (Python only got the row because its obs process was slower).
fn marker_project(cwd: Option<&str>) -> Option<String> {
    let cwd = cwd?;
    obs_cli::project_resolution::find_project_marker(Path::new(cwd))
        .ok()
        .flatten()
        .and_then(|marker| marker.project_id_text().ok())
        .filter(|project| !project.is_empty())
}
