//! Port of tests/unit/test_notify_consumer.py (dry-run delivery, temp state dirs).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use harness_hook::consumers::notify::deliver::{CommandOutput, CommandRunner, LaunchFailure};
use harness_hook::consumers::notify::{handle, EVENTS_FILE, LOG_MAX_RECORDS};
use harness_hook::events::{make_event, Event};
use serde_json::{json, Map, Value};

/// Dry-run must never launch anything.
struct PanickingRunner;

impl CommandRunner for PanickingRunner {
    fn run(&self, _args: &[String], _timeout: Duration) -> Result<CommandOutput, LaunchFailure> {
        panic!("dry-run must not deliver");
    }
}

struct Sandbox {
    _dir: tempfile::TempDir,
    root: PathBuf,
    env: BTreeMap<String, String>,
}

impl Sandbox {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let env = BTreeMap::from([
            (
                "AH_NOTIFY_DIR".to_owned(),
                root.join("notify").display().to_string(),
            ),
            (
                "AH_TASKS_DIR".to_owned(),
                root.join("tasks").display().to_string(),
            ),
            ("AH_NOTIFY_DRY_RUN".to_owned(), "1".to_owned()),
        ]);
        Self {
            _dir: dir,
            root,
            env,
        }
    }

    fn handle(&self, event: &Event) -> Value {
        handle(event, &self.env, &self.root, &PanickingRunner).unwrap()
    }

    fn log_path(&self) -> PathBuf {
        self.root.join("notify").join(EVENTS_FILE)
    }

    fn records(&self) -> Vec<Value> {
        read_lines(&self.log_path())
            .iter()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

fn read_lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn ev(event_type: &str, agent: &str, instance: &str, session: &str, data: Value) -> Event {
    let native = match event_type {
        "prompt.submit" => "UserPromptSubmit",
        "turn.stop" => "Stop",
        _ => "Notification",
    };
    let payload = json!({"session_id": session, "cwd": "C:/work/demo"})
        .as_object()
        .cloned()
        .unwrap();
    let data: Map<String, Value> = data.as_object().cloned().unwrap_or_default();
    make_event(event_type, agent, instance, &payload, native, data)
}

fn claude(event_type: &str) -> Event {
    ev(event_type, "claude", "default", "s1", json!({}))
}

#[test]
fn prompt_then_stop_toasts_done() {
    let sandbox = Sandbox::new();
    sandbox.handle(&ev(
        "prompt.submit",
        "claude",
        "default",
        "s1",
        json!({"prompt": "x"}),
    ));
    sandbox.handle(&claude("turn.stop"));
    let records = sandbox.records();
    let (mark, toast) = (&records[0], &records[1]);
    assert_eq!(
        (
            mark["decision"].as_str(),
            toast["decision"].as_str(),
            toast["sound"].as_str()
        ),
        (Some("mark"), Some("toast"), Some("done"))
    );
    assert_eq!(toast["toastResult"], "dry-run");
    assert_eq!(toast["title"], "Claude Code");
    assert!(toast["message"].as_str().unwrap().contains("[demo]"));
    assert_eq!(mark["message"], Value::Null);
}

#[test]
fn record_keys_follow_python_order() {
    let sandbox = Sandbox::new();
    let record = sandbox.handle(&claude("prompt.submit"));
    let keys: Vec<&str> = record
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "tsUtc",
            "agent",
            "instance",
            "event",
            "nativeEvent",
            "sessionId",
            "transcriptPath",
            "decision",
            "reason",
            "sound",
            "title",
            "message",
            "toastResult",
            "outstanding",
            "herdr",
            "windowsNotificationState",
            "durationMs"
        ]
    );
    let line = read_lines(&sandbox.log_path()).pop().unwrap();
    assert!(line.starts_with(r#"{"tsUtc": ""#));
}

#[test]
fn state_files_use_python_json_text() {
    let sandbox = Sandbox::new();
    sandbox.handle(&claude("prompt.submit"));
    let notify_dir = sandbox.root.join("notify");
    assert_eq!(
        std::fs::read_to_string(notify_dir.join("claude-s1.json")).unwrap(),
        r#"{"pending": true}"#
    );
    assert_eq!(
        std::fs::read_to_string(notify_dir.join("_rate.json")).unwrap(),
        "[]"
    );
    assert!(notify_dir.join("notify-state.lock").exists());
    assert!(notify_dir.join("notify-events.lock").exists());
}

#[test]
fn second_stop_is_skipped() {
    let sandbox = Sandbox::new();
    for event in [
        claude("prompt.submit"),
        claude("turn.stop"),
        claude("turn.stop"),
    ] {
        sandbox.handle(&event);
    }
    assert_eq!(
        sandbox.records().last().unwrap()["reason"],
        "no-pending-prompt"
    );
}

#[test]
fn stop_defers_while_subagent_runs() {
    let sandbox = Sandbox::new();
    let tasks = sandbox.root.join("tasks");
    std::fs::create_dir_all(&tasks).unwrap();
    let started = chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.6f+00:00")
        .to_string();
    std::fs::write(
        tasks.join("claude-s1.json"),
        json!({"ag1": started}).to_string(),
    )
    .unwrap();

    sandbox.handle(&claude("prompt.submit"));
    sandbox.handle(&claude("turn.stop"));
    let last = sandbox.records().pop().unwrap();
    assert_eq!(
        (last["decision"].as_str(), &last["outstanding"]),
        (Some("defer"), &json!(["ag1"]))
    );
}

#[test]
fn stale_subagents_do_not_defer() {
    let sandbox = Sandbox::new();
    let tasks = sandbox.root.join("tasks");
    std::fs::create_dir_all(&tasks).unwrap();
    std::fs::write(
        tasks.join("claude-s1.json"),
        r#"{"ag1": "2020-01-01T00:00:00+00:00"}"#,
    )
    .unwrap();

    sandbox.handle(&claude("prompt.submit"));
    let record = sandbox.handle(&claude("turn.stop"));
    assert_eq!(record["decision"], "toast");
}

#[test]
fn sessions_are_isolated() {
    let sandbox = Sandbox::new();
    sandbox.handle(&ev("prompt.submit", "claude", "default", "a", json!({})));
    sandbox.handle(&ev("turn.stop", "claude", "default", "b", json!({})));
    assert_eq!(
        sandbox.records().last().unwrap()["reason"],
        "no-pending-prompt"
    );
}

#[test]
fn codex_instance_title() {
    let sandbox = Sandbox::new();
    sandbox.handle(&ev("prompt.submit", "codex", "cy", "s1", json!({})));
    sandbox.handle(&ev("turn.stop", "codex", "cy", "s1", json!({})));
    assert_eq!(sandbox.records().last().unwrap()["title"], "Codex (cy)");
}

#[test]
fn log_is_bounded() {
    let sandbox = Sandbox::new();
    let log = sandbox.log_path();
    std::fs::create_dir_all(log.parent().unwrap()).unwrap();
    let seeded: String = (0..LOG_MAX_RECORDS)
        .map(|index| format!("{{\"n\": {index}}}\n"))
        .collect();
    std::fs::write(&log, seeded).unwrap();

    sandbox.handle(&claude("prompt.submit"));
    let lines = read_lines(&log);
    assert_eq!(lines.len(), LOG_MAX_RECORDS);
    let last: Value = serde_json::from_str(lines.last().unwrap()).unwrap();
    assert_eq!(last["decision"], "mark");
    let first: Value = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!(first["n"], 1);
}
