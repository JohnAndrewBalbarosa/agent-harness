//! Port of `tests/unit/test_claude_adapter.py`, `tests/unit/test_codex_adapter.py` and the adapter conformance
//! kit (`tests/conformance/*`), driven by the shared fixtures under `<repo>/adapters/<agent>/fixtures`.

use std::path::{Path, PathBuf};

use harness_hook::adapters::{load_adapter, Adapter};
use harness_hook::consumer::ConsumerResult;
use harness_hook::events::{make_event, to_json, Event, EVENT_TYPES};
use harness_hook::router::tested_versions;
use harness_hook::runtime_env::EnvMap;
use serde_json::{json, Map, Value};

const REQUIRED_MANIFEST_KEYS: [&str; 4] = ["agent", "events", "rulesync_target", "tested_versions"];

fn adapters_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("adapters")
}

fn fixture(agent: &str, name: &str) -> Map<String, Value> {
    let path = adapters_dir()
        .join(agent)
        .join("fixtures")
        .join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path).unwrap();
    serde_json::from_str::<Value>(&text)
        .unwrap()
        .as_object()
        .unwrap()
        .clone()
}

fn adapter(agent: &str) -> &'static dyn Adapter {
    load_adapter(agent).unwrap()
}

/// Normalizes a fixture with its own `hook_event_name` (Claude tests) or an explicit native name (Codex tests).
fn normalize(agent: &str, name: &str, native: Option<&str>) -> Option<Event> {
    let payload = fixture(agent, name);
    let hook_event = payload["hook_event_name"].as_str().unwrap().to_owned();
    adapter(agent).normalize(
        Some(native.unwrap_or(&hook_event)),
        &payload,
        &EnvMap::new(),
    )
}

fn claude(name: &str) -> Event {
    normalize("claude", name, None).unwrap()
}

fn codex(name: &str) -> Event {
    normalize("codex", name, Some(name)).unwrap()
}

// ---- Claude -------------------------------------------------------------------------------------------

#[test]
fn claude_every_fixture_maps_to_its_common_event() {
    let expected = [
        ("SessionStart", "session.start"),
        ("UserPromptSubmit", "prompt.submit"),
        ("PostToolUse", "tool.completed"),
        ("Stop", "turn.stop"),
        ("SubagentStart", "subagent.start"),
        ("SubagentStop", "subagent.stop"),
        ("SessionEnd", "session.end"),
        ("Notification", "attention.needed"),
        ("Notification.idle", "attention.needed"),
    ];
    for (name, common) in expected {
        let event = claude(name);
        assert_eq!(
            (
                event.event.as_str(),
                event.agent.as_str(),
                event.instance.as_str()
            ),
            (common, "claude", "default"),
            "{name}"
        );
    }
}

#[test]
fn claude_notification_kinds() {
    assert_eq!(
        claude("Notification").data["attention"]["kind"],
        "permission"
    );
    let idle = &claude("Notification.idle").data["attention"];
    assert_eq!(idle["kind"], "idle");
    assert_eq!(idle["message"], "Claude is waiting for your input");
}

#[test]
fn claude_other_notification_types_are_ignored() {
    assert!(normalize("claude", "Notification.other", None).is_none());
}

#[test]
fn claude_prompt_keeps_unicode() {
    assert_eq!(
        claude("UserPromptSubmit").data["prompt"],
        "gawin mo na ñ 🙂"
    );
}

#[test]
fn claude_subagent_identity() {
    assert_eq!(
        claude("SubagentStart").data["subagent"],
        json!({"id": "ab2bf5fbb7190fd00", "type": "general-purpose"})
    );
}

#[test]
fn claude_stop_carries_last_message() {
    assert_eq!(claude("Stop").data["last_message"], "Tapos na.");
}

#[test]
fn claude_unknown_event_is_ignored() {
    let payload = json!({"hook_event_name": "PostToolBatch", "session_id": "s"});
    assert!(adapter("claude")
        .normalize(
            Some("PostToolBatch"),
            payload.as_object().unwrap(),
            &EnvMap::new()
        )
        .is_none());
}

#[test]
fn claude_session_start_context_envelope() {
    let out = adapter("claude").respond(
        &claude("SessionStart"),
        &[ConsumerResult::succeeded("context", "Goal: ship")],
    );
    let parsed: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        parsed,
        json!({"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": "Goal: ship"}})
    );
}

#[test]
fn claude_other_events_print_nothing() {
    for name in ["Stop", "PostToolUse", "Notification"] {
        assert_eq!(
            adapter("claude").respond(&claude(name), &[ConsumerResult::succeeded("context", "x")]),
            "",
            "{name}"
        );
    }
}

// ---- Codex --------------------------------------------------------------------------------------------

#[test]
fn codex_every_fixture_maps_to_its_common_event() {
    let expected = [
        ("SessionStart", "session.start"),
        ("UserPromptSubmit", "prompt.submit"),
        ("PostToolUse", "tool.completed"),
        ("Stop", "turn.stop"),
        ("SubagentStart", "subagent.start"),
        ("SubagentStop", "subagent.stop"),
        ("SessionEnd", "session.end"),
        ("PermissionRequest", "attention.needed"),
    ];
    for (native, common) in expected {
        let event = codex(native);
        assert_eq!(
            (
                event.event.as_str(),
                event.agent.as_str(),
                event.native_event.as_str()
            ),
            (common, "codex", native)
        );
        assert_eq!(event.session_id, "sess-codex-1");
    }
}

#[test]
fn codex_prompt_and_turn_id() {
    let event = codex("UserPromptSubmit");
    assert_eq!(event.data["prompt"], "ayusin ang bug ñ");
    assert_eq!(event.turn_id.as_deref(), Some("turn-1"));
}

#[test]
fn codex_tool_details() {
    let tool = &codex("PostToolUse").data["tool"];
    assert_eq!(tool["name"], "shell");
    assert_eq!(tool["input"], json!({"command": ["echo", "hi"]}));
    assert_eq!(tool["response"], json!({"output": "hi"}));
}

#[test]
fn codex_permission_request_is_permission_attention() {
    let attention = &codex("PermissionRequest").data["attention"];
    assert_eq!(
        attention,
        &json!({"kind": "permission", "tool": "shell", "message": "Permission requested: shell"})
    );
}

#[test]
fn codex_subagent_identity() {
    assert_eq!(
        codex("SubagentStop").data["subagent"],
        json!({"id": "agent-7", "type": "worker"})
    );
}

#[test]
fn codex_stop_carries_last_message() {
    let data = codex("Stop").data;
    assert_eq!(
        (
            data["last_message"].clone(),
            data["stop_hook_active"].clone()
        ),
        (json!("Tapos na."), json!(false))
    );
}

#[test]
fn codex_native_event_falls_back_to_payload() {
    let event = adapter("codex")
        .normalize(None, &fixture("codex", "Stop"), &EnvMap::new())
        .unwrap();
    assert_eq!(event.event, "turn.stop");
}

#[test]
fn codex_unknown_event_is_ignored() {
    let payload = json!({"hook_event_name": "FutureEvent", "session_id": "s"});
    assert!(adapter("codex")
        .normalize(
            Some("FutureEvent"),
            payload.as_object().unwrap(),
            &EnvMap::new()
        )
        .is_none());
}

#[test]
fn codex_stop_without_output_continues() {
    assert_eq!(
        adapter("codex").respond(&codex("Stop"), &[]),
        r#"{"continue": true}"#
    );
}

#[test]
fn codex_context_output_is_wrapped_for_session_start() {
    let out = adapter("codex").respond(
        &codex("SessionStart"),
        &[ConsumerResult::succeeded("context", "Vision: X")],
    );
    let parsed: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        parsed["hookSpecificOutput"],
        json!({"hookEventName": "SessionStart", "additionalContext": "Vision: X"})
    );
}

#[test]
fn codex_bridge_envelope_passes_through() {
    let envelope = r#"{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"from bridge"}}"#;
    let out = adapter("codex").respond(
        &codex("SessionStart"),
        &[ConsumerResult::succeeded("herdr-bridge", envelope)],
    );
    assert_eq!(out, envelope);
}

#[test]
fn codex_failed_consumer_output_is_ignored() {
    let partial = ConsumerResult {
        name: "context",
        exit_code: 1,
        stdout: "partial".into(),
    };
    assert_eq!(
        adapter("codex").respond(&codex("SessionStart"), &[partial]),
        ""
    );
}

// ---- Conformance kit ----------------------------------------------------------------------------------

/// `conformance_violations(name, adapter, adapter_dir)`; `[]` means conformant.
fn conformance_violations(name: &str, adapter: &dyn Adapter, adapter_dir: &Path) -> Vec<String> {
    let mut violations = Vec::new();
    if adapter.name() != name {
        violations.push(format!("adapter.name {:?} != {name:?}", adapter.name()));
    }
    violations.extend(manifest_violations(adapter_dir));

    let probe = json!({"hook_event_name": "FutureEvent", "session_id": "s"});
    if adapter
        .normalize(
            Some("FutureEvent"),
            probe.as_object().unwrap(),
            &EnvMap::new(),
        )
        .is_some()
    {
        violations.push("unknown native events must normalize to None".into());
    }
    for path in fixture_paths(adapter_dir) {
        violations.extend(fixture_violations(name, adapter, &path));
    }
    violations
}

fn manifest_violations(adapter_dir: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(adapter_dir.join("manifest.toml")) else {
        return vec!["manifest unreadable".into()];
    };
    let missing: Vec<&str> = REQUIRED_MANIFEST_KEYS
        .into_iter()
        .filter(|key| {
            !text
                .lines()
                .any(|line| line.trim_start().starts_with(&format!("{key} ")))
        })
        .collect();
    let mut violations = Vec::new();
    if !missing.is_empty() {
        violations.push(format!("manifest missing keys: {missing:?}"));
    }
    if tested_versions(&text).is_none_or(|versions| versions.is_empty()) {
        violations.push("manifest tested_versions unreadable".into());
    }
    violations
}

fn fixture_paths(adapter_dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(adapter_dir.join("fixtures"))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .collect()
        })
        .unwrap_or_default();
    paths.retain(|path| {
        path.extension()
            .is_some_and(|extension| extension == "json")
    });
    paths.sort();
    paths
}

fn fixture_violations(name: &str, adapter: &dyn Adapter, path: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(path).unwrap();
    let payload: Value = serde_json::from_str(&text).unwrap();
    let payload = payload.as_object().unwrap();
    let native = payload.get("hook_event_name").and_then(Value::as_str);
    let Some(event) = adapter.normalize(native, payload, &EnvMap::new()) else {
        return Vec::new();
    };

    let file = path.file_name().unwrap().to_string_lossy().into_owned();
    let mut violations = Vec::new();
    if !EVENT_TYPES.contains(&event.event.as_str()) {
        violations.push(format!("{file}: unknown common event {:?}", event.event));
    }
    if event.agent != name {
        violations.push(format!(
            "{file}: event agent {:?} != adapter {name:?}",
            event.agent
        ));
    }
    if !round_trips(&event) {
        violations.push(format!("{file}: event does not round-trip through JSON"));
    }
    let _always_a_string: String = adapter.respond(&event, &[]);
    violations
}

/// `from_json(to_json(event)) == event` for the fields `from_json` restores.
fn round_trips(event: &Event) -> bool {
    let document: Value = serde_json::from_str(&to_json(event)).unwrap();
    document["schema"] == "agent-harness.event/v1"
        && document["event"] == event.event.as_str()
        && document["agent"] == event.agent.as_str()
        && document["instance"] == event.instance.as_str()
        && document["session_id"] == event.session_id.as_str()
        && document["turn_id"] == json!(event.turn_id)
        && document["cwd"] == json!(event.cwd)
        && document["transcript_path"] == json!(event.transcript_path)
        && document["ts"] == event.ts.as_str()
        && document["data"] == Value::Object(event.data.clone())
        && document["native"]["event"] == event.native_event.as_str()
        && document["native"]["payload"] == Value::Object(event.native_payload.clone())
}

#[test]
fn every_adapter_conforms() {
    for name in ["codex", "claude"] {
        assert_eq!(
            conformance_violations(name, adapter(name), &adapters_dir().join(name)),
            Vec::<String>::new(),
            "{name}"
        );
    }
}

/// Violates the contract on purpose: wrong event agent.
struct BrokenAdapter;

impl Adapter for BrokenAdapter {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn instance(&self, _env: &EnvMap) -> String {
        "x".into()
    }

    fn normalize(
        &self,
        native_event: Option<&str>,
        payload: &Map<String, Value>,
        _env: &EnvMap,
    ) -> Option<Event> {
        Some(make_event(
            "turn.stop",
            "someone-else",
            "x",
            payload,
            native_event.unwrap_or(""),
            Map::new(),
        ))
    }

    fn respond(&self, _event: &Event, _results: &[ConsumerResult]) -> String {
        String::new()
    }

    fn safe_default(&self, _native_event: Option<&str>) -> String {
        String::new()
    }
}

#[test]
fn kit_reports_a_broken_adapter() {
    let joined =
        conformance_violations("codex", &BrokenAdapter, &adapters_dir().join("codex")).join("\n");
    assert!(joined.contains("agent"));
    assert!(joined.contains("unknown native events"));
}

#[test]
fn kit_reports_missing_manifest() {
    let violations = conformance_violations(
        "codex",
        adapter("codex"),
        &adapters_dir().join("does-not-exist"),
    );
    assert!(violations
        .iter()
        .any(|violation| violation.contains("manifest")));
}
