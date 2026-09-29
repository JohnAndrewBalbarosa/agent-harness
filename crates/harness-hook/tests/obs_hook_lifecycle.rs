//! Port of `hub/tests/test_hook.py` (HookLifecycleTests) against the in-process obs consumer.
//! Each test runs in a sandboxed harness root with its own spool; no hub, no obs-rs, no Python.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use harness_hook::consumer::Consumer;
use harness_hook::consumers::obs::fingerprints::hmac_sha256_hex;
use harness_hook::consumers::obs::ObsConsumer;
use harness_hook::events::make_event;
use harness_hook::hook_context::HookContext;
use obs_cli::python_compat::uuid5_url_text;
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Map, Value};
use tempfile::TempDir;

const PROJECT_ID: &str = "00000000-0000-7000-8000-000000000001";
const MARKER: &str = "[project]\nid=\"00000000-0000-7000-8000-000000000001\"\nname=\"Test\"\nslug=\"test\"\nschema_version=1\n";

struct Sandbox {
    _temp: TempDir,
    harness: PathBuf,
    project: PathBuf,
    env: BTreeMap<String, String>,
    seen_operations: usize,
}

impl Sandbox {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let harness = temp.path().join("harness");
        let project = temp.path().join("project");
        std::fs::create_dir_all(harness.join("policy")).unwrap();
        std::fs::write(
            harness.join("policy").join("AGENTS.md"),
            "test global instructions",
        )
        .unwrap();
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("observability.project.toml"), MARKER).unwrap();
        Self {
            _temp: temp,
            harness,
            project,
            env: BTreeMap::new(),
            seen_operations: 0,
        }
    }

    fn var_dir(&self) -> PathBuf {
        self.harness
            .join("tools")
            .join("observability-client")
            .join("var")
    }

    fn spool(&self) -> Connection {
        Connection::open(self.var_dir().join("spool.sqlite3")).unwrap()
    }

    /// Runs one hook in the project cwd and returns the operations it enqueued.
    fn invoke(&mut self, event: &str, extra: Value) -> Vec<Value> {
        let cwd = self.project.clone();
        self.invoke_in(&cwd, event, extra)
    }

    fn invoke_in(&mut self, cwd: &Path, event: &str, extra: Value) -> Vec<Value> {
        let mut payload = json!({"hook_event_name": event, "session_id": "native-session", "cwd": cwd.to_string_lossy()});
        for (key, value) in extra.as_object().cloned().unwrap_or_default() {
            payload.as_object_mut().unwrap().insert(key, value);
        }
        let native = payload.as_object().cloned().unwrap();
        let context = HookContext {
            event: make_event(
                common_event(event),
                "codex",
                "personal",
                &native,
                event,
                Map::new(),
            ),
            native_bytes: serde_json::to_vec(&payload).unwrap(),
            native_payload: payload,
            env: self.env.clone(),
            harness_root: self.harness.clone(),
        };

        let result = ObsConsumer.run(&context).unwrap();
        assert_eq!((result.exit_code, result.stdout.as_str()), (0, ""));
        assert_eq!(
            self.last_audit()["outcome"],
            "succeeded",
            "hook failed: {}",
            self.last_audit()
        );
        self.new_operations()
    }

    fn new_operations(&mut self) -> Vec<Value> {
        let conn = self.spool();
        let mut statement = conn
            .prepare("SELECT payload FROM queue ORDER BY rowid")
            .unwrap();
        let payloads: Vec<String> = statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let fresh = payloads[self.seen_operations..]
            .iter()
            .map(|text| serde_json::from_str(text).unwrap())
            .collect();
        self.seen_operations = payloads.len();
        fresh
    }

    fn row(&self) -> Map<String, Value> {
        let conn = self.spool();
        conn.query_row(
            "SELECT session_id,prompt_id,last_prompt_id,prompt_ordinal,execution_id FROM context",
            [],
            |row| {
                let mut map = Map::new();
                map.insert("session_id".into(), json!(row.get::<_, String>(0)?));
                map.insert("prompt_id".into(), json!(row.get::<_, Option<String>>(1)?));
                map.insert(
                    "last_prompt_id".into(),
                    json!(row.get::<_, Option<String>>(2)?),
                );
                map.insert("prompt_ordinal".into(), json!(row.get::<_, i64>(3)?));
                map.insert(
                    "execution_id".into(),
                    json!(row.get::<_, Option<String>>(4)?),
                );
                Ok(map)
            },
        )
        .optional()
        .unwrap()
        .unwrap()
    }

    fn last_audit(&self) -> Value {
        let text = std::fs::read_to_string(self.var_dir().join("hook-invocations.jsonl")).unwrap();
        serde_json::from_str(text.lines().last().unwrap()).unwrap()
    }
}

fn common_event(native: &str) -> &'static str {
    match native {
        "SessionStart" => "session.start",
        "UserPromptSubmit" => "prompt.submit",
        "PostToolUse" => "tool.completed",
        "Stop" => "turn.stop",
        _ => "session.end",
    }
}

fn find<'a>(operations: &'a [Value], kind: &str) -> Option<&'a Value> {
    operations.iter().find(|op| op["kind"] == kind)
}

fn find_code<'a>(operations: &'a [Value], code: &str) -> Option<&'a Value> {
    operations.iter().find(|op| op["data"]["code"] == code)
}

fn emitted_codes(operations: &[Value]) -> Vec<String> {
    operations
        .iter()
        .filter(|op| op["kind"] == "event.emit")
        .map(|op| op["data"]["code"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn subscribes_to_everything_except_subagent_and_attention_events() {
    for event in [
        "session.start",
        "session.end",
        "prompt.submit",
        "tool.completed",
        "turn.stop",
    ] {
        assert!(ObsConsumer.subscribes_to(event), "{event}");
    }
    for event in ["subagent.start", "subagent.stop", "attention.needed"] {
        assert!(!ObsConsumer.subscribes_to(event), "{event}");
    }
}

#[test]
fn uninstrumented_session_uses_global_project_without_marker() {
    let mut sandbox = Sandbox::new();
    let plain = sandbox.harness.parent().unwrap().join("plain");
    std::fs::create_dir_all(&plain).unwrap();

    let operations = sandbox.invoke_in(&plain, "SessionStart", json!({}));

    let global_id = uuid5_url_text("codex-observability:global-agent-project");
    assert_eq!(operations[0]["kind"], "project.register");
    assert_eq!(operations[0]["data"]["projectId"], global_id.as_str());
    assert_eq!(operations[0]["data"]["slug"], "codex-global-sessions");
    assert_eq!(
        find(&operations, "session.start").unwrap()["data"]["projectId"],
        global_id.as_str()
    );
    assert!(!plain.join("observability.project.toml").exists());
}

#[test]
fn marked_project_registers_with_path_alias() {
    let mut sandbox = Sandbox::new();
    let operations = sandbox.invoke("SessionStart", json!({"source": "startup"}));
    let register = &operations[0];
    assert_eq!(register["kind"], "project.register");
    assert_eq!(register["data"]["projectId"], PROJECT_ID);
    assert_eq!(register["data"]["aliases"][0]["kind"], 2);
    assert_eq!(
        find(&operations, "agent.register").unwrap()["data"]["agentInstanceId"],
        uuid5_url_text("codex-observability:personal").as_str()
    );
}

#[test]
fn hook_self_audit_records_trigger_outcome_and_duration_without_payload() {
    let mut sandbox = Sandbox::new();
    sandbox.invoke(
        "SessionStart",
        json!({"source": "startup", "secret": "must-not-be-copied"}),
    );
    let text = std::fs::read_to_string(sandbox.var_dir().join("hook-invocations.jsonl")).unwrap();
    let record = sandbox.last_audit();
    assert_eq!(record["hookEvent"], "SessionStart");
    assert_eq!(record["outcome"], "succeeded");
    assert!(record["durationMs"].as_f64().unwrap() >= 0.0);
    assert!(!text.contains("must-not-be-copied"));
}

#[test]
fn session_fingerprint_uses_the_spool_hmac_key() {
    let mut sandbox = Sandbox::new();
    let operations = sandbox.invoke("SessionStart", json!({"source": "startup"}));
    let key = std::fs::read(sandbox.var_dir().join("hmac.key")).unwrap();
    assert_eq!(key.len(), 32);
    let expected = hmac_sha256_hex(&key, "personal|native-session").unwrap();
    assert_eq!(
        find(&operations, "session.start").unwrap()["data"]["sessionFingerprint"],
        expected.as_str()
    );
}

#[test]
fn resume_reuses_session_and_turns_form_a_parent_chain() {
    let mut sandbox = Sandbox::new();
    sandbox.invoke("SessionStart", json!({"source": "startup"}));
    let first_session = sandbox.row()["session_id"].clone();
    let resumed = sandbox.invoke("SessionStart", json!({"source": "resume"}));
    assert_eq!(sandbox.row()["session_id"], first_session);
    assert_eq!(
        find(&resumed, "session.start").unwrap()["data"]["sessionId"],
        first_session
    );

    let operations = sandbox.invoke(
        "UserPromptSubmit",
        json!({"turn_id": "turn-1", "prompt": "sensitive raw prompt"}),
    );
    let first_prompt = sandbox.row()["prompt_id"].clone();
    let prompt_begin = find(&operations, "prompt.begin").unwrap();
    assert_eq!(prompt_begin["data"]["rawPrompt"], "sensitive raw prompt");
    assert_eq!(prompt_begin["data"]["parentPromptId"], Value::Null);

    let stop = sandbox.invoke("Stop", json!({"turn_id": "turn-1"}));
    let stopped = sandbox.row();
    assert_eq!(stopped["prompt_id"], Value::Null);
    assert_eq!(stopped["last_prompt_id"], first_prompt);
    assert!(find(&stop, "attribution.record").is_some());
    assert!(find(&stop, "code-version.register").is_some());

    let operations = sandbox.invoke(
        "UserPromptSubmit",
        json!({"turn_id": "turn-2", "prompt": "next"}),
    );
    assert_eq!(
        find(&operations, "prompt.begin").unwrap()["data"]["parentPromptId"],
        first_prompt
    );
}

#[test]
fn duplicate_prompt_hook_is_idempotent() {
    let mut sandbox = Sandbox::new();
    sandbox.invoke("SessionStart", json!({"source": "startup"}));
    sandbox.invoke(
        "UserPromptSubmit",
        json!({"turn_id": "same", "prompt": "one"}),
    );
    let ordinal = sandbox.row()["prompt_ordinal"].clone();
    let operations = sandbox.invoke(
        "UserPromptSubmit",
        json!({"turn_id": "same", "prompt": "one"}),
    );
    assert_eq!(sandbox.row()["prompt_ordinal"], ordinal);
    assert!(find(&operations, "prompt.begin").is_none());
}

#[test]
fn prompt_without_stop_supersedes_the_open_prompt() {
    let mut sandbox = Sandbox::new();
    sandbox.invoke("UserPromptSubmit", json!({"turn_id": "a", "prompt": "one"}));
    let operations = sandbox.invoke("UserPromptSubmit", json!({"turn_id": "b", "prompt": "two"}));
    assert_eq!(emitted_codes(&operations)[0], "agent.prompt.stop_missing");
    assert_eq!(
        find(&operations, "execution.finish").unwrap()["data"]["status"],
        4
    );
    assert!(find_code(&operations, "agent.instructions.acknowledged").is_none());
}

#[test]
fn prompt_records_instruction_source_load_state() {
    let mut sandbox = Sandbox::new();
    sandbox.invoke("SessionStart", json!({"source": "startup"}));
    let operations = sandbox.invoke(
        "UserPromptSubmit",
        json!({"turn_id": "instruction-state", "prompt": "hello"}),
    );
    let event = find_code(&operations, "agent.instructions.load_state").unwrap();
    let state = &event["data"]["attributes"];
    assert_eq!(state["loaded"], true);
    assert_eq!(state["instance"], "personal");
    assert!(state["globalPrompt"]["path"]
        .as_str()
        .unwrap()
        .ends_with("AGENTS.md"));
    assert_eq!(state["globalPrompt"]["sha256"].as_str().unwrap().len(), 64);
    assert_eq!(event["data"]["rawPayload"], *state);
    assert!(!event.to_string().contains("test global instructions"));
    let acknowledged = find_code(&operations, "agent.instructions.acknowledged").unwrap();
    assert_eq!(acknowledged["data"]["attributes"]["scope"], "project");
    assert!(find(&operations, "prompt.native-link").is_some());
}

#[test]
fn behavior_audit_records_state_and_repeated_wait_with_raw_tool_record() {
    let mut sandbox = Sandbox::new();
    sandbox.invoke("SessionStart", json!({"source": "startup"}));
    let operations = sandbox.invoke(
        "UserPromptSubmit",
        json!({"turn_id": "turn-wait", "prompt": "private request"}),
    );
    assert_eq!(
        find_code(&operations, "agent.behavior.audit_state").unwrap()["data"]["attributes"]
            ["enabled"],
        true
    );

    let first = sandbox.invoke(
        "PostToolUse",
        json!({"tool_name": "functions.wait", "tool_response": "secret raw response"}),
    );
    let tool_event = find_code(&first, "agent.tool.completed").unwrap();
    let wait_event = find_code(&first, "agent.behavior.wait_observed").unwrap();
    assert_eq!(
        tool_event["data"]["rawPayload"]["toolResponse"],
        "secret raw response"
    );
    assert!(!wait_event.to_string().contains("secret raw response"));
    assert!(find(&first, "tool-use.record").is_some());

    let second = sandbox.invoke(
        "PostToolUse",
        json!({"tool_name": "functions.wait", "tool_response": "another private response"}),
    );
    assert!(emitted_codes(&second)
        .iter()
        .any(|code| code == "agent.behavior.polling_threshold_exceeded"));
    assert!(find(&second, "error.record").is_some());
    let tool_event = find_code(&second, "agent.tool.completed").unwrap();
    assert_eq!(
        tool_event["data"]["rawPayload"]["toolResponse"],
        "another private response"
    );
}

#[test]
fn behavior_audit_off_state_is_recorded_and_suppresses_wait_events() {
    let mut sandbox = Sandbox::new();
    sandbox
        .env
        .insert("CODEX_OBS_BEHAVIOR_AUDIT".into(), "off".into());
    sandbox.invoke("SessionStart", json!({"source": "startup"}));
    let operations = sandbox.invoke(
        "UserPromptSubmit",
        json!({"turn_id": "turn-off", "prompt": "private request"}),
    );
    assert_eq!(
        find_code(&operations, "agent.behavior.audit_state").unwrap()["data"]["attributes"]
            ["enabled"],
        false
    );
    let operations = sandbox.invoke(
        "PostToolUse",
        json!({"tool_name": "functions.wait", "tool_response": "private response"}),
    );
    assert_eq!(emitted_codes(&operations), ["agent.tool.completed"]);
}

#[test]
fn background_transition_is_classified_without_storing_response() {
    let mut sandbox = Sandbox::new();
    sandbox.invoke("SessionStart", json!({"source": "startup"}));
    sandbox.invoke(
        "UserPromptSubmit",
        json!({"turn_id": "turn-background", "prompt": "private request"}),
    );
    let operations = sandbox.invoke(
        "PostToolUse",
        json!({"tool_name": "functions.exec", "tool_response": "Script running with cell ID 42; private payload"}),
    );
    let background = find_code(&operations, "agent.behavior.background_started").unwrap();
    let tool_event = find_code(&operations, "agent.tool.completed").unwrap();
    assert!(!background.to_string().contains("private payload"));
    assert_eq!(
        tool_event["data"]["rawPayload"]["toolResponse"],
        "Script running with cell ID 42; private payload"
    );
}

#[test]
fn tool_use_without_live_execution_only_registers_identity() {
    let mut sandbox = Sandbox::new();
    sandbox.invoke("SessionStart", json!({"source": "startup"}));
    let operations = sandbox.invoke("PostToolUse", json!({"tool_name": "Bash"}));
    let kinds: Vec<&str> = operations
        .iter()
        .map(|op| op["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["project.register", "agent.register"]);
}

#[test]
fn stop_records_exact_assistant_result_and_session_end_closes_session() {
    let mut sandbox = Sandbox::new();
    sandbox.invoke("SessionStart", json!({"source": "startup"}));
    sandbox.invoke(
        "UserPromptSubmit",
        json!({"turn_id": "turn-result", "prompt": "exact prompt"}),
    );
    let operations = sandbox.invoke(
        "Stop",
        json!({"turn_id": "turn-result", "last_assistant_message": "exact assistant result"}),
    );
    assert_eq!(
        find(&operations, "prompt.finish").unwrap()["data"]["rawResult"],
        "exact assistant result"
    );
    assert_eq!(sandbox.row()["execution_id"], Value::Null);

    let session_id = sandbox.row()["session_id"].clone();
    let operations = sandbox.invoke("SessionEnd", json!({}));
    assert_eq!(
        find(&operations, "session.end").unwrap()["data"]["sessionId"],
        session_id
    );
}
