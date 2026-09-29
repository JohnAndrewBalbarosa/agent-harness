//! `tasks` consumer (`core/tasks/consumer.py` + `core/tasks/state.py`): records running subagents per agent
//! session from `subagent.start` / `subagent.stop`.
//!
//! File contract (shared with Python readers such as the notify consumer):
//! - `<AH_TASKS_DIR or <root>/var/tasks>/<agent>-<session>.json`, unsafe characters replaced by `_`, the
//!   session part capped at 120 characters.
//! - Content: `json.dumps({agent_id: started_isoformat, ...})` (default separators, ASCII escapes, insertion
//!   order), written to `<stem>.tmp` then replaced, under the sidecar lock `<file>.lock`.
//! - Entries older than `max_age()` (3 h) are not "outstanding" (a lost stop event cannot block forever).

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use obs_cli::python_compat::{to_python_json, JsonLayout};
use serde_json::{Map, Value};

use crate::consumer::{Consumer, ConsumerResult};
use crate::events::{python_str, Event};
use crate::hook_context::HookContext;
use crate::shared_state::atomic_write::replace_file;
use crate::shared_state::file_lock::with_sidecar_lock;

pub const NAME: &str = "tasks";
pub const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_AGE_HOURS: i64 = 3;
const SESSION_PART_LIMIT: usize = 120;

pub struct TasksConsumer;

impl Consumer for TasksConsumer {
    fn name(&self) -> &'static str {
        NAME
    }

    fn subscribes_to(&self, event: &str) -> bool {
        is_subagent_event(event)
    }

    fn timeout(&self) -> Duration {
        TIMEOUT
    }

    /// Python catches `ValueError/KeyError/OSError`, reports them on stderr and still exits 0; stderr is
    /// discarded by the router, so a failed write is exit 0 with no output here too.
    fn run(&self, context: &HookContext) -> anyhow::Result<ConsumerResult> {
        let _ = record(&task_store(context), &context.event, Utc::now());
        Ok(ConsumerResult::succeeded(NAME, ""))
    }
}

/// `default_store(env)`.
pub fn task_store(context: &HookContext) -> PathBuf {
    context
        .env_value("AH_TASKS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| context.var_dir().join("tasks"))
}

/// `record(store, event, now)`: load → apply → write, all under the sidecar lock.
pub fn record(store: &Path, event: &Event, now: DateTime<Utc>) -> io::Result<()> {
    let path = task_state_path(store, &event.agent, &event.session_id);
    std::fs::create_dir_all(store)?;

    with_sidecar_lock(&path, || {
        let updated = apply_event(load_task_state(&path), event, now);
        let text = to_python_json(&Value::Object(updated), JsonLayout::Spaced, true);
        replace_file(&path, &temp_file_name(&path), text.as_bytes(), false)
    })
}

/// `apply_event(state, event, now)`: start adds/refreshes the id in place, stop removes it.
pub fn apply_event(
    state: Map<String, Value>,
    event: &Event,
    now: DateTime<Utc>,
) -> Map<String, Value> {
    let agent_id = subagent_id(event);
    if agent_id.is_empty() || !is_subagent_event(&event.event) {
        return state;
    }
    if event.event == "subagent.start" {
        let mut started = state;
        started.insert(agent_id, Value::String(python_isoformat(now)));
        return started;
    }
    state
        .into_iter()
        .filter(|(key, _)| *key != agent_id)
        .collect()
}

/// `fresh_agents(state, now)`: ids whose start time parses and is at most `MAX_AGE` old.
pub fn fresh_agents(state: &Map<String, Value>, now: DateTime<Utc>) -> Vec<String> {
    state
        .iter()
        .filter(|(_, started)| {
            started
                .as_str()
                .and_then(parse_aware_isoformat)
                .is_some_and(|at| now - at <= max_age())
        })
        .map(|(agent_id, _)| agent_id.clone())
        .collect()
}

/// `MAX_AGE = timedelta(hours=3)`.
#[inline]
pub fn max_age() -> chrono::TimeDelta {
    chrono::TimeDelta::hours(MAX_AGE_HOURS)
}

/// `_path(store, agent, session_id)`.
pub fn task_state_path(store: &Path, agent: &str, session_id: &str) -> PathBuf {
    let session_part: String = sanitize(session_id)
        .chars()
        .take(SESSION_PART_LIMIT)
        .collect();
    store.join(format!("{}-{}.json", sanitize(agent), session_part))
}

/// `_load(path)`: a JSON object, else empty (missing, unreadable, invalid, or not an object).
pub fn load_task_state(path: &Path) -> Map<String, Value> {
    let parsed = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    match parsed {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    }
}

/// `str((event.data.get("subagent") or {}).get("id") or "")`.
fn subagent_id(event: &Event) -> String {
    event
        .data
        .get("subagent")
        .and_then(Value::as_object)
        .and_then(|subagent| subagent.get("id"))
        .filter(|id| crate::events::is_python_truthy(id))
        .map(python_str)
        .unwrap_or_default()
}

#[inline]
fn is_subagent_event(event: &str) -> bool {
    event == "subagent.start" || event == "subagent.stop"
}

/// `_UNSAFE.sub("_", text)` with `_UNSAFE = [^A-Za-z0-9_-]` (one `_` per code point).
#[inline]
fn sanitize(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                character
            } else {
                '_'
            }
        })
        .collect()
}

/// `path.with_suffix(".tmp")`.
#[inline]
fn temp_file_name(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!("{stem}.tmp")
}

/// `datetime.now(timezone.utc).isoformat()`: microseconds only when non-zero, `+00:00` offset.
pub fn python_isoformat(instant: DateTime<Utc>) -> String {
    let seconds = instant.format("%Y-%m-%dT%H:%M:%S");
    let micros = instant.timestamp_subsec_micros().min(999_999);
    if micros == 0 {
        format!("{seconds}+00:00")
    } else {
        format!("{seconds}.{micros:06}+00:00")
    }
}

/// `datetime.fromisoformat(value)` for offset-carrying values (naive values cannot be compared with an
/// aware `now` in Python either, so they are ignored).
fn parse_aware_isoformat(text: &str) -> Option<DateTime<Utc>> {
    let normalized = text.replace('Z', "+00:00");
    DateTime::parse_from_rfc3339(&normalized)
        .or_else(|_| DateTime::parse_from_str(&normalized, "%Y-%m-%dT%H:%M:%S%.f%:z"))
        .or_else(|_| DateTime::parse_from_str(&normalized, "%Y-%m-%d %H:%M:%S%.f%:z"))
        .ok()
        .map(|instant| instant.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::make_event;
    use chrono::TimeZone;
    use serde_json::json;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 28, 12, 0, 0).unwrap()
    }

    fn subagent(event_type: &str, agent_id: &str, agent: &str) -> Event {
        let native = if event_type == "subagent.start" {
            "SubagentStart"
        } else {
            "SubagentStop"
        };
        let data = json!({"subagent": {"id": agent_id, "type": "general-purpose"}});
        make_event(
            event_type,
            agent,
            "default",
            json!({"session_id": "s1"}).as_object().unwrap(),
            native,
            data.as_object().unwrap().clone(),
        )
    }

    fn outstanding(store: &Path, agent: &str) -> Vec<String> {
        fresh_agents(
            &load_task_state(&task_state_path(store, agent, "s1")),
            now(),
        )
    }

    #[test]
    fn start_then_stop_without_mutation() {
        let started = apply_event(
            Map::new(),
            &subagent("subagent.start", "a1", "claude"),
            now(),
        );
        assert_eq!(started.keys().collect::<Vec<_>>(), ["a1"]);
        assert_eq!(
            apply_event(
                started.clone(),
                &subagent("subagent.stop", "a1", "claude"),
                now()
            ),
            Map::new()
        );
        assert_eq!(started.keys().collect::<Vec<_>>(), ["a1"]);
    }

    #[test]
    fn other_events_and_missing_ids_leave_state_unchanged() {
        let state: Map<String, Value> = [("a1".to_owned(), json!(python_isoformat(now())))]
            .into_iter()
            .collect();
        let stop = make_event(
            "turn.stop",
            "claude",
            "default",
            json!({"session_id": "s1"}).as_object().unwrap(),
            "Stop",
            Map::new(),
        );
        assert_eq!(apply_event(state.clone(), &stop, now()), state);
        assert_eq!(
            apply_event(
                state.clone(),
                &subagent("subagent.start", "", "claude"),
                now()
            ),
            state
        );
    }

    #[test]
    fn fresh_agents_only() {
        let state = json!({"old": "2026-09-28T08:00:00+00:00", "new": "2026-09-28T11:59:00+00:00"});
        assert_eq!(fresh_agents(state.as_object().unwrap(), now()), ["new"]);
    }

    #[test]
    fn record_and_read_back_in_python_format() {
        let dir = tempfile::tempdir().unwrap();
        record(
            dir.path(),
            &subagent("subagent.start", "a1", "claude"),
            now(),
        )
        .unwrap();
        let path = task_state_path(dir.path(), "claude", "s1");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            r#"{"a1": "2026-09-28T12:00:00+00:00"}"#
        );
        assert_eq!(outstanding(dir.path(), "claude"), ["a1"]);
        assert_eq!(outstanding(dir.path(), "codex"), Vec::<String>::new());

        record(
            dir.path(),
            &subagent("subagent.stop", "a1", "claude"),
            now(),
        )
        .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}");
        assert!(!dir.path().join("claude-s1.tmp").exists());
    }

    #[test]
    fn hostile_session_ids_stay_inside_store() {
        let path = task_state_path(Path::new("store"), "claude", "..\\..\\evil");
        assert_eq!(path, Path::new("store").join("claude-______evil.json"));
    }

    #[test]
    fn isoformat_matches_python() {
        let with_micros = Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap()
            + chrono::TimeDelta::microseconds(120);
        assert_eq!(
            python_isoformat(with_micros),
            "2026-01-02T03:04:05.000120+00:00"
        );
        assert_eq!(python_isoformat(now()), "2026-09-28T12:00:00+00:00");
    }
}
