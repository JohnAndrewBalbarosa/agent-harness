//! Background work that defers a "done" toast (`core.tasks.state.outstanding`, read-only here): fresh
//! subagents from the tasks store written by the tasks consumer, plus Claude background shells from
//! the transcript.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde_json::Value;

use super::state_files::{read_json, session_file_name};
use crate::shared_state::file_lock::with_sidecar_lock;
use crate::transcripts::{background_shells, iso_time::parse_iso_instant};

/// `core.tasks.state.MAX_AGE`: a lost stop event cannot suppress notifications forever.
pub const SUBAGENT_MAX_AGE_HOURS: i64 = 3;

/// `tasks.default_store(env)`: `AH_TASKS_DIR` or `<root>/var/tasks`.
pub fn tasks_store(env: &BTreeMap<String, String>, harness_root: &Path) -> PathBuf {
    env.get("AH_TASKS_DIR")
        .filter(|directory| !directory.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| harness_root.join("var").join("tasks"))
}

/// `tasks.outstanding(store, agent, session_id, transcript_path, now)`: subagent ids, then shell ids.
pub fn outstanding(
    store: &Path,
    agent: &str,
    session_id: &str,
    transcript_path: Option<&str>,
    now: DateTime<Utc>,
) -> io::Result<Vec<String>> {
    let tasks_path = store.join(session_file_name(agent, session_id));
    let mut work = if tasks_path.exists() {
        fresh_subagents(&tasks_path, now)?
    } else {
        Vec::new()
    };
    work.extend(background_shells(agent, transcript_path, now));
    Ok(work)
}

/// Reads under the writer's lock: on Windows an open reader makes the writer's replace fail.
fn fresh_subagents(tasks_path: &Path, now: DateTime<Utc>) -> io::Result<Vec<String>> {
    let started_by_agent = with_sidecar_lock(tasks_path, || Ok(read_json(tasks_path)))?;
    let Some(Value::Object(started_by_agent)) = started_by_agent else {
        return Ok(Vec::new());
    };
    let max_age = Duration::hours(SUBAGENT_MAX_AGE_HOURS);
    Ok(started_by_agent
        .iter()
        .filter(|(_, started)| {
            started
                .as_str()
                .and_then(parse_iso_instant)
                .is_some_and(|started| now - started <= max_age)
        })
        .map(|(agent_id, _)| agent_id.clone())
        .collect())
}
