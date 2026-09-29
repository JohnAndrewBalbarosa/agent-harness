//! Herdr bridge hook side: session record, `herdr` CLI projection, spool records, SessionStart output
//! (behaviour of `tools/herdr-bridge/hook.py`), with a recording `herdr` runner.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use harness_hook::consumer::Consumer;
use harness_hook::consumers::herdr_bridge::bridge_log::BridgeLog;
use harness_hook::consumers::herdr_bridge::herdr_cli::{HerdrOutput, HerdrRunner};
use harness_hook::consumers::herdr_bridge::{
    handle_hook_with, session_start_envelope, HerdrBridgeConsumer,
};
use harness_hook::events::make_event;
use harness_hook::hook_context::HookContext;
use obs_cli::python_compat::with_platform_newlines;
use serde_json::{json, Map, Value};

#[derive(Default)]
struct RecordingRunner {
    calls: RefCell<Vec<Vec<String>>>,
    exit_code: i32,
}

impl HerdrRunner for RecordingRunner {
    fn run(&self, args: &[String], _timeout: Duration, _output_limit: usize) -> HerdrOutput {
        self.calls.borrow_mut().push(args.to_vec());
        HerdrOutput {
            exit_code: self.exit_code,
            output: "token=abc ok".to_owned(),
        }
    }
}

impl RecordingRunner {
    fn subcommands(&self) -> Vec<String> {
        self.calls
            .borrow()
            .iter()
            .map(|call| call[..2].join(" "))
            .collect()
    }
}

struct Sandbox {
    root: tempfile::TempDir,
    project: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let project = tempfile::tempdir().unwrap();
        std::fs::write(
            project.path().join("observability.project.toml"),
            "[project]\nid = \"proj-1\"\n",
        )
        .unwrap();
        Self {
            root: tempfile::tempdir().unwrap(),
            project,
        }
    }

    fn spool(&self) -> std::path::PathBuf {
        self.root.path().join("spool.sqlite3")
    }

    fn context(&self, payload: Value, inside_herdr: bool) -> HookContext {
        let native = payload.as_object().cloned().unwrap();
        let native_event = native
            .get("hook_event_name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let mut env = BTreeMap::from([
            (
                "AH_OBS_SPOOL".to_owned(),
                self.spool().display().to_string(),
            ),
            ("OBS_ENDPOINT".to_owned(), "http://127.0.0.1:9".to_owned()),
        ]);
        if inside_herdr {
            env.insert("HERDR_ENV".into(), "1".into());
            env.insert("HERDR_PANE_ID".into(), " w1:p1 ".into());
            env.insert("HERDR_WORKSPACE_ID".into(), "w1".into());
        }
        HookContext {
            event: make_event(
                "turn.stop",
                "codex",
                "personal",
                &native,
                &native_event,
                Map::new(),
            ),
            native_bytes: serde_json::to_vec(&payload).unwrap(),
            native_payload: payload,
            env,
            harness_root: self.root.path().to_path_buf(),
        }
    }

    fn payload(&self, event: &str, extra: Value) -> Value {
        let mut payload = json!({"hook_event_name": event, "session_id": "sess-1", "turn_id": "t1",
                                 "cwd": self.project.path().display().to_string()});
        if let (Some(target), Some(fields)) = (payload.as_object_mut(), extra.as_object()) {
            target.extend(fields.clone());
        }
        payload
    }

    fn run(&self, payload: Value, inside_herdr: bool, runner: &RecordingRunner) -> String {
        let context = self.context(payload, inside_herdr);
        let log = BridgeLog::under_root(self.root.path());
        handle_hook_with(&context, runner, &log).unwrap()
    }

    fn state(&self) -> Map<String, Value> {
        let path = self
            .root
            .path()
            .join("var/herdr-bridge/sessions/sess-1.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn queued(&self) -> Vec<Value> {
        let conn = rusqlite::Connection::open(self.spool()).unwrap();
        let mut statement = conn
            .prepare("SELECT payload FROM queue ORDER BY rowid")
            .unwrap();
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap();
        rows.map(|row| serde_json::from_str(&row.unwrap()).unwrap())
            .collect()
    }

    fn queued_kinds(&self) -> Vec<String> {
        self.queued()
            .iter()
            .map(|operation| operation["kind"].as_str().unwrap().to_owned())
            .collect()
    }
}

#[test]
fn stop_inside_herdr_projects_state_and_queues_every_record() {
    let sandbox = Sandbox::new();
    let runner = RecordingRunner::default();
    let payload = sandbox.payload("Stop", json!({"usage": {"input_tokens": 7}}));

    let stdout = sandbox.run(payload, true, &runner);

    assert_eq!(stdout, "");
    assert_eq!(
        runner.subcommands(),
        ["pane report-agent", "pane read", "agent explain"]
    );
    assert_eq!(
        runner.calls.borrow()[0],
        [
            "pane",
            "report-agent",
            "w1:p1",
            "--source",
            "shared:codex-state",
            "--agent",
            "codex",
            "--state",
            "idle",
            "--message",
            "ready: turn_stopped",
            "--seq",
            "1",
        ]
    );
    let state = sandbox.state();
    assert_eq!(state["canonical_state"], "ready");
    assert_eq!(state["status"], "ready");
    assert_eq!(state["projected_state"], "idle");
    assert_eq!(state["runtime_scope"], "herdr");
    assert_eq!(state["project_id"], "proj-1");
    assert_eq!(state["sequence_no"], 1);

    assert_eq!(
        sandbox.queued_kinds(),
        [
            "herdr.binding",
            "agent-state.record",
            "turn-usage.record",
            "pane-evidence.record"
        ]
    );
    let queued = sandbox.queued();
    assert_eq!(queued[2]["data"]["inputTokens"], 7);
    assert_eq!(queued[2]["data"]["availability"], "exact");
    assert_eq!(queued[2]["data"]["source"], "codex_hook");
    assert_eq!(queued[2]["data"]["sourceEventId"], "sess-1:t1:Stop:1");
    let content = queued[3]["data"]["redactedContent"].as_str().unwrap();
    assert!(content.starts_with("token=") && content.ends_with(" ok") && !content.contains("abc"));
    assert_eq!(queued[3]["data"]["triggerCode"], "state.transition");
}

#[test]
fn repeated_state_keeps_entry_time_and_skips_evidence() {
    let sandbox = Sandbox::new();
    let runner = RecordingRunner::default();
    sandbox.run(
        sandbox.payload("UserPromptSubmit", json!({})),
        true,
        &runner,
    );
    let entered = sandbox.state()["state_entered_at"].clone();

    sandbox.run(
        sandbox.payload("PostToolUse", json!({"tool_name": "Bash"})),
        true,
        &runner,
    );

    let state = sandbox.state();
    assert_eq!(state["sequence_no"], 2);
    assert_eq!(state["status"], "running");
    assert_eq!(state["state_entered_at"], entered);
    let evidence_reads = runner
        .subcommands()
        .iter()
        .filter(|call| *call == "pane read")
        .count();
    assert_eq!(evidence_reads, 1);
}

#[test]
fn failed_projection_captures_evidence_even_without_transition() {
    let sandbox = Sandbox::new();
    let runner = RecordingRunner {
        exit_code: 2,
        ..RecordingRunner::default()
    };
    sandbox.run(
        sandbox.payload("UserPromptSubmit", json!({})),
        true,
        &runner,
    );

    // Both evidence commands failed as well, so no pane-evidence record is queued.
    assert_eq!(
        sandbox.queued_kinds(),
        ["herdr.binding", "agent-state.record"]
    );
    let log = std::fs::read_to_string(
        sandbox
            .root
            .path()
            .join("var/logs/herdr-bridge.lifecycle.jsonl"),
    )
    .unwrap();
    assert!(log.contains("\"event\":\"state.projected\",\"outcome\":\"failed\""));
}

#[test]
fn session_start_reports_the_agent_session_and_prints_nothing_without_a_hub() {
    let sandbox = Sandbox::new();
    let runner = RecordingRunner::default();

    let stdout = sandbox.run(
        sandbox.payload("SessionStart", json!({"source": "startup"})),
        true,
        &runner,
    );

    assert_eq!(stdout, "");
    assert_eq!(
        runner.calls.borrow()[0],
        [
            "pane",
            "report-agent-session",
            "w1:p1",
            "--source",
            "herdr:codex",
            "--agent",
            "codex",
            "--seq",
            "1",
            "--agent-session-id",
            "sess-1",
        ]
    );
    assert_eq!(sandbox.state()["canonical_state"], "idle");
}

#[test]
fn session_end_releases_the_pane_and_marks_inactive() {
    let sandbox = Sandbox::new();
    let runner = RecordingRunner::default();

    sandbox.run(sandbox.payload("SessionEnd", json!({})), true, &runner);

    assert_eq!(runner.subcommands(), ["pane release-agent"]);
    let state = sandbox.state();
    assert_eq!(state["active"], false);
    assert!(state.get("canonical_state").is_none());
    assert_eq!(sandbox.queued_kinds(), ["herdr.binding"]);
}

#[test]
fn outside_herdr_never_calls_the_cli() {
    let sandbox = Sandbox::new();
    let runner = RecordingRunner::default();

    sandbox.run(
        sandbox.payload("PermissionRequest", json!({"reason": "needs approval"})),
        false,
        &runner,
    );

    assert!(runner.calls.borrow().is_empty());
    let state = sandbox.state();
    assert_eq!(state["runtime_scope"], "outside_herdr");
    assert_eq!(state["reason"], "needs approval");
    assert_eq!(state["projected_state"], "blocked");
    assert_eq!(state["pane_id"], "");
}

#[test]
fn state_file_is_indented_ascii_json_with_platform_newlines() {
    let sandbox = Sandbox::new();
    sandbox.run(
        sandbox.payload("Stop", json!({"cwd": "C:/wörk"})),
        false,
        &RecordingRunner::default(),
    );

    let text = std::fs::read_to_string(
        sandbox
            .root
            .path()
            .join("var/herdr-bridge/sessions/sess-1.json"),
    )
    .unwrap();
    assert!(text.starts_with(&with_platform_newlines(
        "{\n  \"session_id\": \"sess-1\",\n"
    )));
    assert!(text.contains("C:/w\\u00f6rk"));
    assert!(text.ends_with('}'));
}

#[test]
fn session_start_envelope_is_python_print_output() {
    assert_eq!(
        session_start_envelope("Vision: é"),
        with_platform_newlines(
            "{\"hookSpecificOutput\": {\"hookEventName\": \"SessionStart\", \"additionalContext\": \"Vision: é\"}}\n"
        )
    );
}

#[test]
fn consumer_is_enabled_only_inside_a_herdr_pane() {
    let sandbox = Sandbox::new();
    let bridge = HerdrBridgeConsumer;
    assert!(bridge.subscribes_to("turn.stop"));
    assert!(!bridge.subscribes_to("subagent.stop"));
    assert!(!bridge.is_enabled(&sandbox.context(json!({}), false)));
    assert!(bridge.is_enabled(&sandbox.context(json!({}), true)));
}

#[test]
fn spool_path_defaults_under_the_observability_client() {
    let root = Path::new("/harness");
    let path = harness_hook::consumers::herdr_bridge::observability_outbox::spool_path(
        root,
        &BTreeMap::new(),
    );
    assert_eq!(
        path,
        root.join("tools")
            .join("observability-client")
            .join("var")
            .join("spool.sqlite3")
    );
}
