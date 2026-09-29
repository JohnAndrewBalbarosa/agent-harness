//! Notify consumer (`core/notify/consumer.py`): decides and delivers done / needs-you notifications for
//! every agent.
//!
//! State lives under `AH_NOTIFY_DIR` (default `<root>/var/notify`): per-session pending flags and one
//! global rate window, read-modify-written under the `notify-state.lock` sidecar. Every decision is
//! appended to a bounded `notify-events.jsonl` (under `notify-events.lock`) that `core.notify.report`
//! cross-references for premature or missed notifications.

pub mod background_work;
pub mod bounded_jsonl;
pub mod decide;
pub mod deliver;
pub mod display;
pub mod state_files;
pub mod windows_state;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Context;
use chrono::{DateTime, Utc};
use obs_cli::python_compat::{parse_python_int, to_python_json, JsonLayout};
use serde_json::{json, Value};

use self::decide::{decide, Action, Decision, RateLimit};
use self::deliver::{
    deliver, herdr_binary, toast_script_path, CommandRunner, SystemRunner, ToastRequest,
};
use self::state_files::{load_state, save_state, session_file_name, RATE_FILE};
use crate::consumer::{Consumer, ConsumerResult};
use crate::events::Event;
use crate::hook_context::HookContext;
use crate::shared_state::file_lock::with_sidecar_lock;

pub const NAME: &str = "notify";
pub const EVENTS_FILE: &str = "notify-events.jsonl";
pub const LOG_MAX_RECORDS: usize = 1000;
const STATE_LOCK_NAME: &str = "notify-state";
const EVENTS_LOCK_NAME: &str = "notify-events";

pub struct NotifyConsumer;

impl Consumer for NotifyConsumer {
    fn name(&self) -> &'static str {
        NAME
    }

    fn subscribes_to(&self, event: &str) -> bool {
        matches!(event, "prompt.submit" | "turn.stop" | "attention.needed")
    }

    fn run(&self, context: &HookContext) -> anyhow::Result<ConsumerResult> {
        handle(
            &context.event,
            &context.env,
            &context.harness_root,
            &SystemRunner,
        )?;
        Ok(ConsumerResult::succeeded(NAME, ""))
    }
}

/// `handle(event, env)`: returns the record appended to `notify-events.jsonl`.
pub fn handle(
    event: &Event,
    env: &BTreeMap<String, String>,
    harness_root: &Path,
    runner: &dyn CommandRunner,
) -> anyhow::Result<Value> {
    // Mental model: outstanding background work → locked decide + state write → optional delivery
    // → one bounded decision record.
    let started = Instant::now();
    let directory = notify_dir(env, harness_root);
    std::fs::create_dir_all(&directory)
        .with_context(|| format!("unable to create {}", directory.display()))?;
    let now = Utc::now();

    let outstanding = outstanding_work(event, env, harness_root, now)?;
    let decision = decide_under_lock(event, &directory, &outstanding, now, rate_limit(env))?;
    let message = compose_message(&decision, event);
    let title = display::title(&event.agent, &event.instance);
    let toast_result = deliver_if_toast(&decision, &title, &message, env, harness_root, runner);

    let record = decision_record(&DecisionRecord {
        event,
        env,
        now,
        decision: &decision,
        title: &title,
        message: &message,
        toast_result: toast_result.as_deref(),
        outstanding: &outstanding,
        started,
    });
    append_decision_record(&directory, &record)?;
    Ok(record)
}

/// `AH_NOTIFY_DIR` or `<root>/var/notify`.
#[inline]
fn notify_dir(env: &BTreeMap<String, String>, harness_root: &Path) -> PathBuf {
    env.get("AH_NOTIFY_DIR")
        .filter(|directory| !directory.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| harness_root.join("var").join("notify"))
}

/// `_rate(env)`: `NOTIFY_RATE_LIMIT=<limit>/<window>`, default 5/60.
pub fn rate_limit(env: &BTreeMap<String, String>) -> RateLimit {
    let text = env
        .get("NOTIFY_RATE_LIMIT")
        .map(String::as_str)
        .unwrap_or_default();
    let parts: Vec<&str> = text.split('/').collect();
    match parts.as_slice() {
        [limit, window] => match (parse_python_int(limit), parse_python_int(window)) {
            (Some(limit), Some(window_s)) => RateLimit { limit, window_s },
            _ => RateLimit::default(),
        },
        _ => RateLimit::default(),
    }
}

/// Only stops and attention requests wait for background work.
#[inline]
fn outstanding_work(
    event: &Event,
    env: &BTreeMap<String, String>,
    harness_root: &Path,
    now: DateTime<Utc>,
) -> anyhow::Result<Vec<String>> {
    if !matches!(event.event.as_str(), "turn.stop" | "attention.needed") {
        return Ok(Vec::new());
    }
    let store = background_work::tasks_store(env, harness_root);
    background_work::outstanding(
        &store,
        &event.agent,
        &event.session_id,
        event.transcript_path.as_deref(),
        now,
    )
    .context("unable to read outstanding background work")
}

/// Read-modify-write of the session flag and rate window under `notify-state.lock`.
fn decide_under_lock(
    event: &Event,
    directory: &Path,
    outstanding: &[String],
    now: DateTime<Utc>,
    rate: RateLimit,
) -> anyhow::Result<Decision> {
    let session_path = directory.join(session_file_name(&event.agent, &event.session_id));
    let rate_path = directory.join(RATE_FILE);
    with_sidecar_lock(&directory.join(STATE_LOCK_NAME), || {
        let state = load_state(&session_path, &rate_path);
        let (decision, new_state) = decide(event, &state, outstanding, now.timestamp(), rate);
        save_state(&session_path, &rate_path, &new_state)?;
        Ok(decision)
    })
    .context("unable to update notify state")
}

/// `_message(decision, event)`: `"<message>\n[<cwd leaf>]"`, or the bare message without a cwd.
pub fn compose_message(decision: &Decision, event: &Event) -> String {
    let project = event
        .cwd
        .as_deref()
        .map(windows_path_leaf)
        .unwrap_or_default();
    match (&decision.message, project.is_empty()) {
        (message, false) => format!("{}\n[{project}]", message.as_deref().unwrap_or("None")),
        (Some(message), true) => message.clone(),
        (None, true) => String::new(),
    }
}

/// `PureWindowsPath(path).name`: last component, ignoring drive/UNC anchors, `.` parts and trailing
/// separators.
pub fn windows_path_leaf(path: &str) -> String {
    let unified = path.replace('\\', "/");
    let is_unc = unified.starts_with("//");
    let without_drive = strip_drive(&unified);
    let parts: Vec<&str> = without_drive
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    let anchor_parts = if is_unc { 2 } else { 0 };
    if parts.len() <= anchor_parts {
        return String::new();
    }
    parts
        .last()
        .map(|leaf| (*leaf).to_owned())
        .unwrap_or_default()
}

#[inline]
fn strip_drive(path: &str) -> &str {
    let mut characters = path.chars();
    match (characters.next(), characters.next()) {
        (Some(letter), Some(':')) if letter.is_ascii_alphabetic() => &path[2..],
        _ => path,
    }
}

/// Toasts are delivered (or dry-run) and yield a result string; other decisions yield `None`.
fn deliver_if_toast(
    decision: &Decision,
    title: &str,
    message: &str,
    env: &BTreeMap<String, String>,
    harness_root: &Path,
    runner: &dyn CommandRunner,
) -> Option<String> {
    if decision.action != Action::Toast {
        return None;
    }
    if is_dry_run(env) {
        return Some("dry-run".to_owned());
    }
    let request = ToastRequest {
        title,
        message,
        sound: decision.sound.unwrap_or(decide::Sound::Done),
    };
    let herdr = herdr_binary(env);
    Some(deliver(
        &request,
        herdr.as_deref(),
        runner,
        &toast_script_path(harness_root),
    ))
}

/// `AH_NOTIFY_DRY_RUN=1` records decisions without delivering.
#[inline]
pub fn is_dry_run(env: &BTreeMap<String, String>) -> bool {
    env.get("AH_NOTIFY_DRY_RUN").map(String::as_str) == Some("1")
}

struct DecisionRecord<'a> {
    event: &'a Event,
    env: &'a BTreeMap<String, String>,
    now: DateTime<Utc>,
    decision: &'a Decision,
    title: &'a str,
    message: &'a str,
    toast_result: Option<&'a str>,
    outstanding: &'a [String],
    started: Instant,
}

/// The `notify-events.jsonl` record in the Python key order.
fn decision_record(parts: &DecisionRecord<'_>) -> Value {
    let event = parts.event;
    let pane_id = parts.env.get("HERDR_PANE_ID");
    let in_herdr = parts.env.get("HERDR_ENV").map(String::as_str) == Some("1")
        && pane_id.is_some_and(|pane| !pane.is_empty());
    json!({
        "tsUtc": parts.now.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
        "agent": event.agent,
        "instance": event.instance,
        "event": event.event,
        "nativeEvent": event.native_event,
        "sessionId": event.session_id,
        "transcriptPath": event.transcript_path,
        "decision": parts.decision.action.as_str(),
        "reason": parts.decision.reason,
        "sound": parts.decision.sound.map(decide::Sound::as_str),
        "title": parts.title,
        "message": parts.toast_result.map(|_| parts.message),
        "toastResult": parts.toast_result,
        "outstanding": parts.outstanding,
        "herdr": {"inHerdr": in_herdr, "paneId": pane_id},
        "windowsNotificationState": windows_state::windows_notification_state(),
        "durationMs": elapsed_ms_one_decimal(parts.started),
    })
}

/// `round((perf_counter() - started) * 1000, 1)`.
#[inline]
fn elapsed_ms_one_decimal(started: Instant) -> f64 {
    (started.elapsed().as_secs_f64() * 10_000.0).round() / 10.0
}

/// Appends under `notify-events.lock`, keeping the newest `LOG_MAX_RECORDS`.
fn append_decision_record(directory: &Path, record: &Value) -> anyhow::Result<()> {
    let line = to_python_json(record, JsonLayout::Spaced, false);
    let log_path = directory.join(EVENTS_FILE);
    with_sidecar_lock(&directory.join(EVENTS_LOCK_NAME), || {
        bounded_jsonl::append_keeping_newest(&log_path, LOG_MAX_RECORDS, &line)
    })
    .with_context(|| format!("unable to append {}", log_path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cwd_leaf_follows_pure_windows_path() {
        assert_eq!(windows_path_leaf("C:/work/demo"), "demo");
        assert_eq!(windows_path_leaf("C:\\work\\demo\\"), "demo");
        assert_eq!(windows_path_leaf("C:/"), "");
        assert_eq!(windows_path_leaf("\\\\server\\share"), "");
        assert_eq!(windows_path_leaf("\\\\server\\share\\x"), "x");
        assert_eq!(windows_path_leaf("/home/u/proj"), "proj");
    }

    #[test]
    fn rate_limit_parses_or_falls_back() {
        let mut env = BTreeMap::new();
        assert_eq!(rate_limit(&env), RateLimit::default());
        env.insert("NOTIFY_RATE_LIMIT".to_owned(), "3/10".to_owned());
        assert_eq!(
            rate_limit(&env),
            RateLimit {
                limit: 3,
                window_s: 10
            }
        );
        env.insert("NOTIFY_RATE_LIMIT".to_owned(), "3/x".to_owned());
        assert_eq!(rate_limit(&env), RateLimit::default());
    }
}
