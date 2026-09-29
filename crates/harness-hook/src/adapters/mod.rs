//! Adapter contract (`adapters/base.py`): each agent's adapter turns native hook payloads into common events
//! and turns consumer results back into the agent's native hook stdout.
//!
//! - `shared_mapping`: native events whose payloads are identical across Codex CLI and Claude Code.
//! - `session_context_response`: SessionStart stdout (context text wrapped, else herdr-bridge's envelope).

pub mod claude;
pub mod codex;

use obs_cli::python_compat::{to_python_json, JsonLayout};
use serde_json::{json, Map, Value};

use crate::consumer::ConsumerResult;
use crate::events::{is_python_truthy, Event};
use crate::runtime_env::{python_strip, EnvMap};

/// Consumer stdout beyond this many bytes is dropped (`MAX_CONSUMER_STDOUT`, `core/router/harness_hook.py:25`).
pub const MAX_CONSUMER_STDOUT: usize = 65_536;

pub trait Adapter: Send + Sync {
    fn name(&self) -> &'static str;

    fn instance(&self, env: &EnvMap) -> String;

    /// `None` means "not an event the harness handles": the router prints `safe_default`.
    fn normalize(
        &self,
        native_event: Option<&str>,
        payload: &Map<String, Value>,
        env: &EnvMap,
    ) -> Option<Event>;

    fn respond(&self, event: &Event, results: &[ConsumerResult]) -> String;

    fn safe_default(&self, native_event: Option<&str>) -> String;
}

/// `load_adapter(name)`: only names matching `^[a-z][a-z0-9_]*$` that have an adapter module.
pub fn load_adapter(name: &str) -> Option<&'static dyn Adapter> {
    match name {
        claude::NAME => Some(&claude::ClaudeAdapter),
        codex::NAME => Some(&codex::CodexAdapter),
        _ => None,
    }
}

/// `native_event or str(payload.get("hook_event_name") or "")`.
pub fn native_event_name(native_event: Option<&str>, payload: &Map<String, Value>) -> String {
    if let Some(native) = native_event.filter(|native| !native.is_empty()) {
        return native.to_owned();
    }
    match payload.get("hook_event_name") {
        Some(value) if is_python_truthy(value) => crate::events::python_str(value),
        _ => String::new(),
    }
}

/// `env.get(key) or default`.
#[inline]
pub fn env_or<'a>(env: &'a EnvMap, key: &str, default: &'a str) -> &'a str {
    env.get(key)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(default)
}

/// `payload.get(key)` (missing → `None`/null).
#[inline]
pub fn field(payload: &Map<String, Value>, key: &str) -> Value {
    payload.get(key).cloned().unwrap_or(Value::Null)
}

/// `SHARED_MAPPING[native]` → `(common event, extracted data)`.
pub fn shared_mapping(
    native: &str,
    payload: &Map<String, Value>,
) -> Option<(&'static str, Map<String, Value>)> {
    let mapped = match native {
        "SessionStart" => ("session.start", json!({"source": field(payload, "source")})),
        "SessionEnd" => ("session.end", json!({"reason": field(payload, "reason")})),
        "UserPromptSubmit" => ("prompt.submit", json!({"prompt": field(payload, "prompt")})),
        "PostToolUse" => ("tool.completed", tool_data(payload)),
        "Stop" => ("turn.stop", stop_data(payload)),
        "SubagentStart" => ("subagent.start", subagent_data(payload)),
        "SubagentStop" => ("subagent.stop", subagent_stop_data(payload)),
        _ => return None,
    };
    Some((mapped.0, into_object(mapped.1)))
}

/// `successful_output(results, name)`: the first matching result with exit 0 and non-blank stdout, stripped.
pub fn successful_output(results: &[ConsumerResult], name: &str) -> String {
    results
        .iter()
        .find(|result| {
            result.name == name
                && result.exit_code == 0
                && !strip_ascii_bytes(result.stdout.as_bytes()).is_empty()
        })
        .map(|result| python_strip(&result.stdout).to_owned())
        .unwrap_or_default()
}

/// `session_context_response(results)`.
pub fn session_context_response(results: &[ConsumerResult]) -> String {
    let context = successful_output(results, "context");
    if context.is_empty() {
        return successful_output(results, "herdr-bridge");
    }
    let envelope = json!({"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": context}});
    to_python_json(&envelope, JsonLayout::Spaced, false)
}

/// `stdout[:MAX_CONSUMER_STDOUT]` then `.decode("utf-8", errors="replace")`.
pub fn bounded_stdout(stdout: &str) -> String {
    let bytes = stdout.as_bytes();
    if bytes.len() <= MAX_CONSUMER_STDOUT {
        return stdout.to_owned();
    }
    String::from_utf8_lossy(&bytes[..MAX_CONSUMER_STDOUT]).into_owned()
}

fn tool_data(payload: &Map<String, Value>) -> Value {
    json!({"tool": {
        "name": field(payload, "tool_name"),
        "input": field(payload, "tool_input"),
        "response": field(payload, "tool_response"),
    }})
}

fn stop_data(payload: &Map<String, Value>) -> Value {
    let is_active = payload
        .get("stop_hook_active")
        .is_some_and(is_python_truthy);
    json!({"last_message": field(payload, "last_assistant_message"), "stop_hook_active": is_active})
}

fn subagent_data(payload: &Map<String, Value>) -> Value {
    json!({"subagent": {"id": field(payload, "agent_id"), "type": field(payload, "agent_type")}})
}

fn subagent_stop_data(payload: &Map<String, Value>) -> Value {
    let mut data = into_object(subagent_data(payload));
    data.insert(
        "last_message".into(),
        field(payload, "last_assistant_message"),
    );
    Value::Object(data)
}

#[inline]
fn into_object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// Python `bytes.strip()` (ASCII whitespace including `\x0b`).
fn strip_ascii_bytes(bytes: &[u8]) -> &[u8] {
    let is_space = |byte: &u8| matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c);
    let start = bytes
        .iter()
        .position(|byte| !is_space(byte))
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !is_space(byte))
        .map_or(start, |index| index + 1);
    &bytes[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_and_escaping_names_have_no_adapter() {
        assert!(load_adapter("does-not-exist").is_none());
        assert!(load_adapter("..core").is_none());
        assert_eq!(
            load_adapter("claude").map(|adapter| adapter.name()),
            Some("claude")
        );
    }

    #[test]
    fn successful_output_needs_exit_zero_and_text() {
        let results = [
            ConsumerResult::failed("context", 1),
            ConsumerResult::succeeded("context", "  \n"),
            ConsumerResult::succeeded("context", "  Goal \n"),
        ];
        assert_eq!(successful_output(&results, "context"), "Goal");
        assert_eq!(successful_output(&results, "herdr-bridge"), "");
    }

    #[test]
    fn context_envelope_uses_python_default_separators_without_ascii_escapes() {
        let text = session_context_response(&[ConsumerResult::succeeded("context", "ñ 🙂")]);
        assert_eq!(
            text,
            r#"{"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": "ñ 🙂"}}"#
        );
    }

    #[test]
    fn stdout_is_bounded_with_replacement_characters() {
        let long = format!("{}é", "a".repeat(MAX_CONSUMER_STDOUT - 1));
        let bounded = bounded_stdout(&long);
        assert!(bounded.ends_with('\u{fffd}'));
    }
}
