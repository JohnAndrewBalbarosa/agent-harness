//! Agent behaviour audit (hook.py `behavior_audit_enabled`, `wait_threshold`, `tool_observation` and the
//! PostToolUse background/wait counting). Classifies tool calls without retaining input or response text.

use obs_cli::python_compat::{parse_python_int, python_utc_now, uuid7_text};
use serde_json::{json, Value};

use super::event_records::{
    error_record_data, event_emit_data, PolicyViolation, Signal, TraceSlot,
};
use super::operations::operation;
use super::payload_fields::{text_or, Payload};
use crate::events::python_str;
use crate::hook_context::HookContext;

const AUDIT_ENV: &str = "CODEX_OBS_BEHAVIOR_AUDIT";
const WAIT_THRESHOLD_ENV: &str = "CODEX_OBS_WAIT_THRESHOLD";
const DISABLED_VALUES: [&str; 4] = ["0", "false", "off", "no"];
const DEFAULT_WAIT_THRESHOLD: i64 = 1;
const MAX_WAIT_THRESHOLD: i64 = 100;
const TOOL_NAME_LIMIT: usize = 120;
const WAIT_SUFFIXES: [&str; 3] = [".wait", ".wait_agent", ".write_stdin"];
const WAIT_NAMES: [&str; 3] = ["wait", "wait_agent", "write_stdin"];
const CELL_MARKER: &str = "script running with cell id";
const BEHAVIOUR_CATEGORY: i64 = 3;
const POLLING_SUMMARY: &str = "Repeated wait calls exceeded the configured per-prompt threshold.";

/// `os.environ.get("CODEX_OBS_BEHAVIOR_AUDIT", "on").strip().lower() not in {"0","false","off","no"}`.
pub fn is_enabled(context: &HookContext) -> bool {
    let setting = context
        .env_value(AUDIT_ENV)
        .unwrap_or("on")
        .trim()
        .to_lowercase();
    !DISABLED_VALUES.contains(&setting.as_str())
}

/// `max(0, min(100, int(os.environ.get("CODEX_OBS_WAIT_THRESHOLD", "1"))))`, 1 when unparseable.
pub fn wait_threshold(context: &HookContext) -> i64 {
    context
        .env_value(WAIT_THRESHOLD_ENV)
        .map_or(Some(DEFAULT_WAIT_THRESHOLD), parse_python_int)
        .map_or(DEFAULT_WAIT_THRESHOLD, |value| {
            value.clamp(0, MAX_WAIT_THRESHOLD)
        })
}

/// hook.py `tool_observation(value)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolObservation {
    pub tool: String,
    pub wait_like: bool,
    pub background_started: bool,
}

pub fn observe_tool(payload: &Payload) -> ToolObservation {
    let tool = text_or(
        payload,
        &["tool_name", "toolName"],
        "unknown",
        TOOL_NAME_LIMIT,
    );
    // `str(value.get("tool_response") or value.get("toolResponse") or "").lower()`; a dict is rendered as
    // JSON instead of repr, which keeps every key/phrase substring this check looks for.
    let response =
        text_or(payload, &["tool_response", "toolResponse"], "", usize::MAX).to_lowercase();
    ToolObservation {
        wait_like: is_wait_tool(&normalized_tool_name(&tool)),
        background_started: is_background_transition(&response),
        tool,
    }
}

/// `tool.lower().replace("-", "_")`.
#[inline]
pub fn normalized_tool_name(tool: &str) -> String {
    tool.to_lowercase().replace('-', "_")
}

#[inline]
fn is_wait_tool(normalized: &str) -> bool {
    WAIT_SUFFIXES
        .iter()
        .any(|suffix| normalized.ends_with(suffix))
        || WAIT_NAMES.contains(&normalized)
}

#[inline]
fn is_background_transition(response_text: &str) -> bool {
    response_text.contains(CELL_MARKER)
        || (response_text.contains("session_id") && !response_text.contains("exit_code"))
}

/// `agent.behavior.audit_state` signal emitted at prompt start.
pub fn audit_state_signal(enabled: bool, threshold: i64) -> Signal<'static> {
    Signal {
        code: "agent.behavior.audit_state",
        category: 1,
        severity: 1,
        outcome: 1,
        summary: format!(
            "Behavior audit {}.",
            if enabled { "enabled" } else { "disabled" }
        ),
        attributes: json!({"enabled": enabled, "waitThreshold": threshold, "backgroundDetectionEnabled": enabled}),
    }
}

/// Per-prompt counters carried in the `context` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolCounters {
    pub sequence_no: i64,
    pub wait_count: i64,
    pub background_count: i64,
}

/// Mental model: background transition → wait observation → polling-threshold violation.
pub fn behaviour_operations(
    context: &HookContext,
    slot: &TraceSlot,
    observation: &ToolObservation,
    counters: &mut ToolCounters,
) -> Vec<Value> {
    let mut operations = Vec::new();
    if observation.background_started {
        counters.background_count += 1;
        counters.sequence_no += 1;
        operations.push(background_operation(slot, observation, counters));
    }
    if observation.wait_like {
        counters.wait_count += 1;
        counters.sequence_no += 1;
        operations.extend(wait_operations(
            slot,
            observation,
            counters,
            wait_threshold(context),
        ));
    }
    operations
}

fn background_operation(
    slot: &TraceSlot,
    observation: &ToolObservation,
    counters: &ToolCounters,
) -> Value {
    let signal = Signal {
        code: "agent.behavior.background_started",
        category: BEHAVIOUR_CATEGORY,
        severity: 1,
        outcome: 1,
        summary: "A tool transitioned to background execution.".to_owned(),
        attributes: json!({"tool": observation.tool, "backgroundCount": counters.background_count, "auditEnabled": true}),
    };
    let data = event_emit_data(&uuid7_text(), slot, counters.sequence_no, signal);
    operation("event.emit", Value::Object(data), None)
}

fn wait_operations(
    slot: &TraceSlot,
    observation: &ToolObservation,
    counters: &ToolCounters,
    threshold: i64,
) -> Vec<Value> {
    let exceeded = counters.wait_count > threshold;
    let observed_at = python_utc_now();
    let event_id = uuid7_text();
    let signal = Signal {
        code: if exceeded {
            "agent.behavior.polling_threshold_exceeded"
        } else {
            "agent.behavior.wait_observed"
        },
        category: BEHAVIOUR_CATEGORY,
        severity: if exceeded { 3 } else { 1 },
        outcome: if exceeded { 3 } else { 1 },
        summary: if exceeded {
            POLLING_SUMMARY
        } else {
            "A bounded wait call was observed."
        }
        .to_owned(),
        attributes: json!({"tool": observation.tool, "waitCount": counters.wait_count, "waitThreshold": threshold, "auditEnabled": true}),
    };
    let data = event_emit_data(&event_id, slot, counters.sequence_no, signal);
    let mut operations = vec![operation(
        "event.emit",
        Value::Object(data),
        Some(&observed_at),
    )];

    if exceeded {
        let project_text = python_str(slot.project_id);
        let violation = PolicyViolation {
            fingerprint_source: format!("{project_text}|agent.behavior.polling_threshold_exceeded"),
            exception_class: "BehaviorPolicyViolation",
            summary: POLLING_SUMMARY,
            event_id: &event_id,
            observed_at: &observed_at,
        };
        let data = error_record_data(slot.project_id, &violation);
        operations.push(operation("error.record", data, Some(&observed_at)));
    }
    operations
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(value: Value) -> Payload {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn classifies_wait_and_background_tools() {
        let wait = observe_tool(&payload(
            json!({"tool_name": "functions.wait", "tool_response": "x"}),
        ));
        assert!(wait.wait_like && !wait.background_started);
        let stdin = observe_tool(&payload(json!({"toolName": "Write-Stdin"})));
        assert!(stdin.wait_like);
        let cell = observe_tool(&payload(
            json!({"tool_name": "functions.exec", "tool_response": "Script running with cell ID 42"}),
        ));
        assert!(cell.background_started && !cell.wait_like);
        let session = observe_tool(&payload(
            json!({"tool_name": "exec", "tool_response": {"session_id": 3}}),
        ));
        assert!(session.background_started);
        let finished = observe_tool(&payload(
            json!({"tool_name": "exec", "tool_response": {"session_id": 3, "exit_code": 0}}),
        ));
        assert!(!finished.background_started);
        assert_eq!(observe_tool(&payload(json!({}))).tool, "unknown");
    }
}
