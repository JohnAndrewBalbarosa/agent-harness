//! Codex CLI adapter (`adapters/codex/adapter.py`). Payload field names follow
//! codex-rs/hooks/schema/generated/*.command.input.schema.json.
//!
//! - Instance: `AH_INSTANCE`, else the `CODEX_HOME` leaf (`.codex` → personal, `.codex-cy` → cy,
//!   `.codex-feu` → feu, `.codex-<x>` → x, anything else verbatim), else `personal`.
//! - `PermissionRequest` becomes `attention.needed` (kind `permission`).
//! - `Stop` must always answer `{"continue": true}`, even on failure.

use serde_json::{json, Map, Value};

use super::{
    additional_context_response, field, native_event_name, session_context_response,
    shared_mapping, Adapter,
};
use crate::consumer::ConsumerResult;
use crate::events::{make_event, python_str, Event};
use crate::runtime_env::EnvMap;

pub const NAME: &str = "codex";
/// `json.dumps({"continue": True})`.
pub const CONTINUE: &str = r#"{"continue": true}"#;
const DEFAULT_INSTANCE: &str = "personal";
const HOME_PREFIX: &str = ".codex-";

pub struct CodexAdapter;

impl Adapter for CodexAdapter {
    fn name(&self) -> &'static str {
        NAME
    }

    fn instance(&self, env: &EnvMap) -> String {
        if let Some(instance) = non_empty(env, "AH_INSTANCE") {
            return instance.to_owned();
        }
        match non_empty(env, "CODEX_HOME") {
            Some(home) => instance_for_home(home),
            None => DEFAULT_INSTANCE.to_owned(),
        }
    }

    fn normalize(
        &self,
        native_event: Option<&str>,
        payload: &Map<String, Value>,
        env: &EnvMap,
    ) -> Option<Event> {
        let native = native_event_name(native_event, payload);
        let (common, data) = codex_mapping(&native, payload)?;
        Some(make_event(
            common,
            NAME,
            &self.instance(env),
            payload,
            &native,
            data,
        ))
    }

    fn respond(&self, event: &Event, results: &[ConsumerResult]) -> String {
        match event.event.as_str() {
            "session.start" => session_context_response(results),
            "prompt.submit" | "tool.requested" => additional_context_response(event, results),
            "turn.stop" => CONTINUE.to_owned(),
            _ => String::new(),
        }
    }

    fn safe_default(&self, native_event: Option<&str>) -> String {
        if native_event == Some("Stop") {
            CONTINUE.to_owned()
        } else {
            String::new()
        }
    }
}

/// `MAPPING = {**SHARED_MAPPING, "PermissionRequest": ...}`.
fn codex_mapping(
    native: &str,
    payload: &Map<String, Value>,
) -> Option<(&'static str, Map<String, Value>)> {
    if native != "PermissionRequest" {
        return shared_mapping(native, payload);
    }
    let tool_name = field(payload, "tool_name");
    let message = format!("Permission requested: {}", python_str(&tool_name));
    let data = json!({"attention": {"kind": "permission", "tool": tool_name, "message": message}});
    match data {
        Value::Object(map) => Some(("attention.needed", map)),
        _ => None,
    }
}

/// `HOME_INSTANCES.get(leaf, leaf.removeprefix(".codex-") or "personal")`.
pub fn instance_for_home(home: &str) -> String {
    let leaf = windows_path_name(home);
    match leaf {
        ".codex" => "personal".to_owned(),
        ".codex-cy" => "cy".to_owned(),
        ".codex-feu" => "feu".to_owned(),
        other => {
            let stripped = other.strip_prefix(HOME_PREFIX).unwrap_or(other);
            if stripped.is_empty() {
                DEFAULT_INSTANCE.to_owned()
            } else {
                stripped.to_owned()
            }
        }
    }
}

// Mental model: drop the anchor (drive `C:` or UNC `\\server\share`), then take the last component that is
// not empty or `.` — what `PureWindowsPath(home).name` yields.
/// `PureWindowsPath(path).name`.
pub fn windows_path_name(path: &str) -> &str {
    let relative = without_windows_anchor(path);
    relative
        .split(is_windows_separator)
        .rfind(|component| !component.is_empty() && *component != ".")
        .unwrap_or("")
}

fn without_windows_anchor(path: &str) -> &str {
    if is_unc(path) {
        return skip_components(path.trim_start_matches(is_windows_separator), 2);
    }
    if has_drive(path) {
        return &path[2..];
    }
    path
}

/// Skips `count` separator-delimited components (the UNC server and share).
fn skip_components(path: &str, count: usize) -> &str {
    let mut rest = path;
    for _ in 0..count {
        rest = match rest.find(is_windows_separator) {
            Some(index) => &rest[index + 1..],
            None => "",
        };
    }
    rest
}

#[inline]
fn is_windows_separator(character: char) -> bool {
    character == '\\' || character == '/'
}

#[inline]
fn is_unc(path: &str) -> bool {
    let mut characters = path.chars();
    let starts_with_two_separators = matches!(
        (characters.next(), characters.next()),
        (Some(first), Some(second)) if is_windows_separator(first) && is_windows_separator(second)
    );
    starts_with_two_separators && !path[2..].starts_with(is_windows_separator)
}

#[inline]
fn has_drive(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

#[inline]
fn non_empty<'a>(env: &'a EnvMap, key: &str) -> Option<&'a str> {
    env.get(key)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> EnvMap {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn instance_from_codex_home() {
        let cases = [
            (r"C:\Users\x\.codex", "personal"),
            (r"C:\Users\x\.codex-cy", "cy"),
            (r"C:\Users\x\.codex-feu", "feu"),
            (r"C:\Users\x\.codex-work", "work"),
            ("C:/Users/x/.codex-cy/", "cy"),
            (r"C:\Users\x\.codex-", "personal"),
            (r"C:\", "personal"),
            (r"\\server\share", "personal"),
            (r"D:\homes\lab", "lab"),
        ];
        for (home, instance) in cases {
            assert_eq!(
                CodexAdapter.instance(&env(&[("CODEX_HOME", home)])),
                instance,
                "{home}"
            );
        }
        assert_eq!(CodexAdapter.instance(&EnvMap::new()), "personal");
    }

    #[test]
    fn explicit_instance_override() {
        let env = env(&[("AH_INSTANCE", "lab"), ("CODEX_HOME", r"C:\x\.codex")]);
        assert_eq!(CodexAdapter.instance(&env), "lab");
    }

    #[test]
    fn safe_defaults() {
        assert_eq!(CodexAdapter.safe_default(Some("Stop")), CONTINUE);
        assert_eq!(CodexAdapter.safe_default(Some("PostToolUse")), "");
        assert_eq!(CodexAdapter.safe_default(Some("FutureEvent")), "");
        assert_eq!(CodexAdapter.safe_default(None), "");
    }

    #[test]
    fn permission_message_uses_python_str_of_missing_tool() {
        let payload = json!({"hook_event_name": "PermissionRequest", "session_id": "s"});
        let event = CodexAdapter
            .normalize(None, payload.as_object().unwrap(), &EnvMap::new())
            .unwrap();
        assert_eq!(
            event.data["attention"]["message"],
            "Permission requested: None"
        );
    }
}
