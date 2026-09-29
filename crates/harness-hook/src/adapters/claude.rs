//! Claude Code adapter (`adapters/claude/adapter.py`). Payload field names follow code.claude.com/docs/en/hooks.
//!
//! - Instance: `AH_INSTANCE`, else `default`.
//! - `Notification` becomes `attention.needed` only for `permission_prompt` / `idle_prompt`.
//! - Only SessionStart prints anything (the context envelope); the safe default is always empty.

use serde_json::{json, Map, Value};

use super::{env_or, field, native_event_name, session_context_response, shared_mapping, Adapter};
use crate::consumer::ConsumerResult;
use crate::events::{is_python_truthy, make_event, python_str, Event};
use crate::runtime_env::EnvMap;

pub const NAME: &str = "claude";
const DEFAULT_INSTANCE: &str = "default";

pub struct ClaudeAdapter;

impl Adapter for ClaudeAdapter {
    fn name(&self) -> &'static str {
        NAME
    }

    fn instance(&self, env: &EnvMap) -> String {
        env_or(env, "AH_INSTANCE", DEFAULT_INSTANCE).to_owned()
    }

    fn normalize(
        &self,
        native_event: Option<&str>,
        payload: &Map<String, Value>,
        env: &EnvMap,
    ) -> Option<Event> {
        let native = native_event_name(native_event, payload);
        let instance = self.instance(env);

        if native == "Notification" {
            return attention_event(&instance, payload, &native);
        }
        let (common, data) = shared_mapping(&native, payload)?;
        Some(make_event(common, NAME, &instance, payload, &native, data))
    }

    fn respond(&self, event: &Event, results: &[ConsumerResult]) -> String {
        if event.event == "session.start" {
            session_context_response(results)
        } else {
            String::new()
        }
    }

    fn safe_default(&self, _native_event: Option<&str>) -> String {
        String::new()
    }
}

fn attention_event(instance: &str, payload: &Map<String, Value>, native: &str) -> Option<Event> {
    let kind = attention_kind(payload)?;
    let data = json!({"attention": {"kind": kind, "message": field(payload, "message")}});
    let Value::Object(data) = data else {
        return None;
    };
    Some(make_event(
        "attention.needed",
        NAME,
        instance,
        payload,
        native,
        data,
    ))
}

/// `ATTENTION_KINDS.get(str(payload.get("notification_type") or ""))`.
fn attention_kind(payload: &Map<String, Value>) -> Option<&'static str> {
    let notification_type = payload
        .get("notification_type")
        .filter(|value| is_python_truthy(value))
        .map(python_str)
        .unwrap_or_default();
    match notification_type.as_str() {
        "permission_prompt" => Some("permission"),
        "idle_prompt" => Some("idle"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_override() {
        let work: EnvMap = [("AH_INSTANCE".to_owned(), "work".to_owned())]
            .into_iter()
            .collect();
        assert_eq!(ClaudeAdapter.instance(&work), "work");
        assert_eq!(ClaudeAdapter.instance(&EnvMap::new()), "default");
    }

    #[test]
    fn other_notification_types_are_ignored() {
        let payload =
            json!({"hook_event_name": "Notification", "notification_type": "auth_success"});
        let payload = payload.as_object().unwrap();
        assert!(ClaudeAdapter
            .normalize(None, payload, &EnvMap::new())
            .is_none());
    }

    #[test]
    fn safe_default_is_empty() {
        for native in [
            Some("Stop"),
            Some("SessionStart"),
            Some("FutureEvent"),
            None,
        ] {
            assert_eq!(ClaudeAdapter.safe_default(native), "");
        }
    }
}
