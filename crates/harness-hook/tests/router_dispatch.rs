//! Port of `tests/unit/test_router.py`: the router with fake in-process consumers injected through
//! `run_hook_with`. The harness root is the repository (for `adapters/<agent>/manifest.toml`); every log
//! goes to a temp `AH_LOG_DIR`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use harness_hook::consumer::{Consumer, ConsumerResult};
use harness_hook::events::to_json;
use harness_hook::hook_context::HookContext;
use harness_hook::router::{run_hook_with, RouterSetup, LIFECYCLE_LOG_NAME};
use serde_json::{json, Value};

const DEFAULT_EVENTS: &[&str] = &[
    "session.start",
    "prompt.submit",
    "turn.stop",
    "tool.completed",
];

#[derive(Clone)]
enum Behavior {
    /// `echo_event.py` with `stdin="event"`: writes the common event JSON to `FAKE_OUT`.
    EchoEvent,
    /// `echo_event.py` with `stdin="native"`: writes the raw payload bytes to `FAKE_OUT`.
    EchoNative,
    /// Writes the `AH_*` identity it received to `FAKE_OUT`.
    DumpIdentity,
    /// `sleeper.py`.
    Sleep(Duration),
    /// `crasher.py` (exit code).
    Exit(i32),
    Panic,
    /// `printer.py`: prints `FAKE_PRINT`.
    Print,
}

struct FakeConsumer {
    name: &'static str,
    events: &'static [&'static str],
    behavior: Behavior,
    timeout: Duration,
    enabled: bool,
}

impl FakeConsumer {
    fn new(name: &'static str, behavior: Behavior) -> Self {
        Self {
            name,
            events: DEFAULT_EVENTS,
            behavior,
            timeout: Duration::from_secs(10),
            enabled: true,
        }
    }

    fn events(self, events: &'static [&'static str]) -> Self {
        Self { events, ..self }
    }

    fn with_timeout(self, timeout: Duration) -> Self {
        Self { timeout, ..self }
    }

    fn disabled(self) -> Self {
        Self {
            enabled: false,
            ..self
        }
    }

    fn boxed(self) -> Box<dyn Consumer> {
        Box::new(self)
    }
}

impl Consumer for FakeConsumer {
    fn name(&self) -> &'static str {
        self.name
    }

    fn subscribes_to(&self, event: &str) -> bool {
        self.events.contains(&event)
    }

    fn is_enabled(&self, _context: &HookContext) -> bool {
        self.enabled
    }

    fn timeout(&self) -> Duration {
        self.timeout
    }

    fn run(&self, context: &HookContext) -> anyhow::Result<ConsumerResult> {
        let out = context.env_value("FAKE_OUT").map(PathBuf::from);
        match &self.behavior {
            Behavior::EchoEvent => write_out(out, to_json(&context.event).as_bytes()),
            Behavior::EchoNative => write_out(out, &context.native_bytes),
            Behavior::DumpIdentity => {
                let identity = json!({
                    "AH_AGENT": context.env_value("AH_AGENT"),
                    "AH_INSTANCE": context.env_value("AH_INSTANCE"),
                    "AH_EVENT": context.env_value("AH_EVENT"),
                });
                write_out(out, identity.to_string().as_bytes())
            }
            Behavior::Sleep(duration) => std::thread::sleep(*duration),
            Behavior::Exit(code) => return Ok(ConsumerResult::failed(self.name, *code)),
            Behavior::Panic => panic!("fake consumer crashed"),
            Behavior::Print => {
                let text = context
                    .env_value("FAKE_PRINT")
                    .unwrap_or_default()
                    .to_owned();
                return Ok(ConsumerResult::succeeded(self.name, text));
            }
        }
        Ok(ConsumerResult::succeeded(self.name, ""))
    }
}

fn write_out(out: Option<PathBuf>, bytes: &[u8]) {
    if let Some(path) = out {
        std::fs::write(path, bytes).unwrap();
    }
}

struct Harness {
    dir: tempfile::TempDir,
}

impl Harness {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn tmp(&self) -> &Path {
        self.dir.path()
    }

    fn run(
        &self,
        agent: &str,
        native: Option<&str>,
        payload: Value,
        consumers: Vec<Box<dyn Consumer>>,
        env: &[(&str, &str)],
    ) -> String {
        let stdin = serde_json::to_vec(&payload).unwrap();
        self.run_bytes(agent, native, &stdin, consumers, env)
    }

    fn run_bytes(
        &self,
        agent: &str,
        native: Option<&str>,
        stdin: &[u8],
        consumers: Vec<Box<dyn Consumer>>,
        env: &[(&str, &str)],
    ) -> String {
        let mut merged = vec![(
            "AH_LOG_DIR".to_owned(),
            self.tmp().to_string_lossy().into_owned(),
        )];
        merged.extend(
            env.iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned())),
        );
        let setup = RouterSetup {
            harness_root: repository_root(),
            env: merged.into_iter().collect(),
            registry: consumers,
        };
        run_hook_with(setup, agent, native, stdin)
    }

    fn out_path(&self, name: &str) -> String {
        self.tmp().join(name).to_string_lossy().into_owned()
    }

    fn log_records(&self) -> Vec<Value> {
        let path = self.tmp().join(LIFECYCLE_LOG_NAME);
        std::fs::read_to_string(path)
            .map(|text| {
                text.lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn records(&self, operation: &str) -> Vec<Value> {
        self.log_records()
            .into_iter()
            .filter(|record| record["operation"] == operation)
            .collect()
    }
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn stop_payload() -> Value {
    json!({"hook_event_name": "Stop", "session_id": "s"})
}

#[test]
fn malformed_stdin_returns_safe_default() {
    let harness = Harness::new();
    let out = harness.run_bytes("codex", Some("Stop"), b"{not json", Vec::new(), &[]);
    assert_eq!(out, r#"{"continue": true}"#);
    let invalid = harness.records("hook.payload.invalid");
    assert_eq!(invalid.len(), 1);
    assert_eq!(
        invalid[0]["details"],
        json!({"agent": "codex", "nativeEvent": "Stop"})
    );
}

#[test]
fn empty_stdin_returns_safe_default() {
    let harness = Harness::new();
    assert_eq!(
        harness.run_bytes("codex", Some("Stop"), b"", Vec::new(), &[]),
        r#"{"continue": true}"#
    );
}

#[test]
fn utf8_payload_round_trip() {
    let harness = Harness::new();
    let out_file = harness.out_path("seen.json");
    let payload =
        json!({"hook_event_name": "UserPromptSubmit", "session_id": "s", "prompt": "ñá 🙂"});
    let consumers = vec![FakeConsumer::new("echo", Behavior::EchoEvent).boxed()];
    harness.run(
        "claude",
        None,
        payload,
        consumers,
        &[("FAKE_OUT", &out_file)],
    );

    let received: Value =
        serde_json::from_str(&std::fs::read_to_string(&out_file).unwrap()).unwrap();
    assert_eq!(received["event"], "prompt.submit");
    assert_eq!(received["data"]["prompt"], "ñá 🙂");
}

#[test]
fn native_consumer_gets_raw_payload() {
    let harness = Harness::new();
    let out_file = harness.out_path("raw.json");
    let payload = json!({"hook_event_name": "Stop", "session_id": "s", "extra": "kept"});
    let consumers = vec![FakeConsumer::new("legacy", Behavior::EchoNative).boxed()];
    harness.run(
        "codex",
        Some("Stop"),
        payload,
        consumers,
        &[("FAKE_OUT", &out_file)],
    );

    let received: Value =
        serde_json::from_str(&std::fs::read_to_string(&out_file).unwrap()).unwrap();
    assert_eq!(received["extra"], "kept");
}

#[test]
fn consumers_receive_the_event_identity() {
    let harness = Harness::new();
    let out_file = harness.out_path("identity.json");
    let consumers = vec![FakeConsumer::new("identity", Behavior::DumpIdentity).boxed()];
    harness.run(
        "codex",
        Some("Stop"),
        stop_payload(),
        consumers,
        &[("FAKE_OUT", &out_file), ("AH_INSTANCE", "cy")],
    );

    let identity: Value =
        serde_json::from_str(&std::fs::read_to_string(&out_file).unwrap()).unwrap();
    assert_eq!(
        identity,
        json!({"AH_AGENT": "codex", "AH_INSTANCE": "cy", "AH_EVENT": "turn.stop"})
    );
}

#[test]
fn consumer_timeout_does_not_block() {
    let harness = Harness::new();
    let slow = FakeConsumer::new("slow", Behavior::Sleep(Duration::from_secs(6)))
        .with_timeout(Duration::from_secs(1));
    let started = Instant::now();
    let out = harness.run(
        "codex",
        Some("Stop"),
        stop_payload(),
        vec![slow.boxed()],
        &[],
    );

    assert!(started.elapsed() < Duration::from_secs(4));
    assert_eq!(out, r#"{"continue": true}"#);
    let timeout = harness.records("consumer.timeout");
    assert_eq!(
        timeout[0]["details"],
        json!({"consumer": "slow", "timeoutS": 1})
    );
    assert_eq!(
        harness.records("hook.dispatch")[0]["details"]["failures"],
        json!({"slow": 124})
    );
}

#[test]
fn consumer_crash_is_isolated() {
    let harness = Harness::new();
    let out_file = harness.out_path("ok.json");
    let consumers = vec![
        FakeConsumer::new("crash", Behavior::Exit(3)).boxed(),
        FakeConsumer::new("echo", Behavior::EchoEvent).boxed(),
    ];
    let out = harness.run(
        "codex",
        Some("Stop"),
        stop_payload(),
        consumers,
        &[("FAKE_OUT", &out_file)],
    );

    assert!(Path::new(&out_file).exists());
    assert_eq!(out, r#"{"continue": true}"#);
    let dispatch = harness.records("hook.dispatch").pop().unwrap();
    assert_eq!(dispatch["details"]["failures"], json!({"crash": 3}));
    assert_eq!(dispatch["details"]["consumers"], json!(["crash", "echo"]));
}

#[test]
fn consumer_panic_is_isolated_as_exit_one() {
    let harness = Harness::new();
    let consumers = vec![FakeConsumer::new("boom", Behavior::Panic).boxed()];
    let out = harness.run("codex", Some("Stop"), stop_payload(), consumers, &[]);

    assert_eq!(out, r#"{"continue": true}"#);
    let exited = harness.records("consumer.exited");
    assert_eq!(exited[0]["outcome"], "failed");
    assert_eq!(exited[0]["details"]["exitCode"], 1);
    assert_eq!(
        harness.records("hook.dispatch")[0]["details"]["failures"],
        json!({"boom": 1})
    );
}

#[test]
fn context_output_reaches_agent() {
    let harness = Harness::new();
    let payload =
        json!({"hook_event_name": "SessionStart", "session_id": "s", "source": "startup"});
    let consumers = vec![FakeConsumer::new("context", Behavior::Print).boxed()];
    let out = harness.run(
        "claude",
        Some("SessionStart"),
        payload,
        consumers,
        &[("FAKE_PRINT", "Goal: X ñ\n")],
    );

    assert_eq!(
        out,
        r#"{"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": "Goal: X ñ"}}"#
    );
}

#[test]
fn unknown_native_event_gets_safe_default_without_consumers() {
    let harness = Harness::new();
    let out_file = harness.out_path("never.json");
    let consumers = vec![FakeConsumer::new("echo", Behavior::EchoEvent).boxed()];
    let out = harness.run(
        "codex",
        Some("FutureEvent"),
        json!({"session_id": "s"}),
        consumers,
        &[("FAKE_OUT", &out_file)],
    );

    assert_eq!(out, "");
    assert!(!Path::new(&out_file).exists());
}

#[test]
fn only_subscribed_and_enabled_consumers_run() {
    let harness = Harness::new();
    let out_file = harness.out_path("sub.json");
    let consumers = vec![
        FakeConsumer::new("echo", Behavior::EchoEvent)
            .events(&["subagent.start"])
            .boxed(),
        FakeConsumer::new("off", Behavior::EchoEvent)
            .disabled()
            .boxed(),
    ];
    harness.run(
        "codex",
        Some("Stop"),
        stop_payload(),
        consumers,
        &[("FAKE_OUT", &out_file)],
    );

    assert!(!Path::new(&out_file).exists());
    assert_eq!(
        harness.records("hook.dispatch")[0]["details"]["consumers"],
        json!([])
    );
}

#[test]
fn untested_version_is_logged_not_blocked() {
    let harness = Harness::new();
    let payload = json!({"hook_event_name": "SessionStart", "session_id": "s", "source": "startup", "version": "9.9.9"});
    let out = harness.run("claude", Some("SessionStart"), payload, Vec::new(), &[]);

    assert_eq!(out, "");
    let record = &harness.records("agent.version.untested")[0];
    assert_eq!(record["outcome"], "observed");
    assert_eq!(
        record["details"],
        json!({"agent": "claude", "version": "9.9.9"})
    );
}

#[test]
fn tested_version_is_not_logged() {
    let harness = Harness::new();
    let manifest =
        std::fs::read_to_string(repository_root().join("adapters/claude/manifest.toml")).unwrap();
    let tested = harness_hook::router::tested_versions(&manifest).unwrap();
    let payload =
        json!({"hook_event_name": "SessionStart", "session_id": "s", "version": tested[0]});
    harness.run("claude", Some("SessionStart"), payload, Vec::new(), &[]);

    assert!(harness.records("agent.version.untested").is_empty());
}

#[test]
fn dispatch_is_logged_with_python_detail_order() {
    let harness = Harness::new();
    let consumers = vec![FakeConsumer::new("quiet", Behavior::EchoEvent).boxed()];
    harness.run("claude", Some("Stop"), stop_payload(), consumers, &[]);

    let dispatch = &harness.records("hook.dispatch")[0];
    assert_eq!(dispatch["component"], "router");
    assert_eq!(dispatch["outcome"], "succeeded");
    let details = dispatch["details"].as_object().unwrap();
    let keys: Vec<&str> = details.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "agent",
            "instance",
            "event",
            "nativeEvent",
            "consumers",
            "failures",
            "durationMs"
        ]
    );
    assert_eq!(
        (details["agent"].as_str(), details["event"].as_str()),
        (Some("claude"), Some("turn.stop"))
    );
    assert!(details["durationMs"].is_f64());

    let exited = &harness.records("consumer.exited")[0];
    let exited_keys: Vec<&str> = exited["details"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(exited_keys, ["consumer", "exitCode", "durationMs"]);
}

#[test]
fn unknown_agent_prints_nothing_and_is_logged() {
    let harness = Harness::new();
    assert_eq!(
        harness.run_bytes("nope", Some("Stop"), b"{}", Vec::new(), &[]),
        ""
    );
    assert_eq!(
        harness.records("hook.agent.unknown")[0]["details"],
        json!({"agent": "nope"})
    );
}

#[test]
fn log_records_are_compact_single_lines() {
    let harness = Harness::new();
    harness.run("claude", Some("Stop"), stop_payload(), Vec::new(), &[]);
    let text = std::fs::read_to_string(harness.tmp().join(LIFECYCLE_LOG_NAME)).unwrap();
    assert!(text.ends_with('\n'));
    assert!(text.starts_with(r#"{"timestamp":""#));
    assert!(
        text.contains(r#""component":"router","operation":"hook.dispatch","outcome":"succeeded""#)
    );
}
