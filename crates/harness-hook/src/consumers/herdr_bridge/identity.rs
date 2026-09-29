//! Who and where this hook is (`tools/herdr-bridge/common.py` + `hook.py` helpers): identifier sanitizing,
//! payload lookups, the Codex profile name, and the observability project id of the working directory.

use std::collections::BTreeMap;
use std::path::PathBuf;

use obs_cli::project_marker::{parse_project_table, MarkerValue, MARKER_FILE_NAME};
use obs_cli::python_compat::uuid5_url_text;
use serde_json::{Map, Value};

use crate::events::{is_python_truthy, python_str};

const SAFE_ID_LIMIT: usize = 240;
const GLOBAL_PROJECT_NAME: &str = "codex-observability:global-agent-project";
const KNOWN_PROFILES: [&str; 3] = ["personal", "cy", "feu"];

/// `safe_id(value, fallback)`: `str(value or fallback).strip()`, disallowed chars → `_`, first 240 chars.
pub fn safe_id(value: Option<&Value>, fallback: &str) -> String {
    let raw = match value {
        Some(value) if is_python_truthy(value) => python_str(value),
        _ => fallback.to_owned(),
    };
    let cleaned: String = raw
        .trim_matches(is_python_space)
        .chars()
        .map(|character| {
            if is_safe_id_char(character) {
                character
            } else {
                '_'
            }
        })
        .take(SAFE_ID_LIMIT)
        .collect();
    if cleaned.is_empty() {
        fallback.to_owned()
    } else {
        cleaned
    }
}

#[inline]
fn is_safe_id_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':')
}

/// Python `str.isspace()` (Rust whitespace plus the ASCII separators U+001C..U+001F).
#[inline]
pub fn is_python_space(character: char) -> bool {
    character.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&character)
}

/// `payload_value(payload, *names)`: the first value that is neither `None` nor `""`.
pub fn payload_value<'a>(payload: &'a Map<String, Value>, names: &[&str]) -> Option<&'a Value> {
    names
        .iter()
        .filter_map(|name| payload.get(*name))
        .find(|value| !matches!(value, Value::Null) && value.as_str() != Some(""))
}

/// `profile_name()`: explicit `CODEX_SHARED_PROFILE`, else derived from `CODEX_HOME`, else `personal`.
pub fn profile_name(env: &BTreeMap<String, String>) -> String {
    let explicit = env
        .get("CODEX_SHARED_PROFILE")
        .map(|value| value.trim().to_lowercase())
        .unwrap_or_default();
    if KNOWN_PROFILES.contains(&explicit.as_str()) {
        return explicit;
    }
    let home = env
        .get("CODEX_HOME")
        .map(|value| value.to_lowercase().replace('/', "\\"))
        .unwrap_or_default();
    if home.ends_with("\\.codex-cy") {
        return "cy".to_owned();
    }
    if home.ends_with("\\.codex-feu") {
        return "feu".to_owned();
    }
    "personal".to_owned()
}

/// `project_record(cwd)["id"]`, else the synthetic global agent project (uuid5 over the URL namespace).
pub fn project_id_for(cwd: &str) -> String {
    project_marker_id(cwd).unwrap_or_else(global_project_id)
}

#[inline]
pub fn global_project_id() -> String {
    uuid5_url_text(GLOBAL_PROJECT_NAME)
}

// Mental model: nearest `observability.project.toml` upward from the resolved cwd decides; an unreadable or
// id-less nearest marker means "no project" (Python returns None instead of searching further up).
fn project_marker_id(cwd: &str) -> Option<String> {
    let start = resolved_directory(cwd);
    let marker = start
        .ancestors()
        .map(|directory| directory.join(MARKER_FILE_NAME))
        .find(|path| path.is_file())?;
    let text = std::fs::read_to_string(marker).ok()?;
    let table = parse_project_table(&text).ok()?;
    let id = table.get("id").filter(|value| is_marker_truthy(value))?;
    Some(id.to_python_str())
}

#[inline]
fn is_marker_truthy(value: &MarkerValue) -> bool {
    match value {
        MarkerValue::Text(text) => !text.is_empty(),
        MarkerValue::Integer(number) => *number != 0,
        MarkerValue::Float(number) => *number != 0.0,
        MarkerValue::Boolean(flag) => *flag,
    }
}

/// `Path(cwd or os.getcwd()).resolve()` (non-strict: a missing path is made absolute as-is).
fn resolved_directory(cwd: &str) -> PathBuf {
    let current = std::env::current_dir().unwrap_or_default();
    let requested = if cwd.is_empty() {
        current.clone()
    } else {
        PathBuf::from(cwd)
    };
    let absolute = if requested.is_absolute() {
        requested
    } else {
        current.join(requested)
    };
    std::fs::canonicalize(&absolute).unwrap_or(absolute)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn safe_id_replaces_disallowed_characters() {
        assert_eq!(safe_id(Some(&json!(" a/b c:1.2 ")), "unknown"), "a_b_c:1.2");
        assert_eq!(safe_id(None, "unknown"), "unknown");
        assert_eq!(safe_id(Some(&json!("")), ""), "");
        assert_eq!(safe_id(Some(&json!(0)), "unknown"), "unknown");
    }

    #[test]
    fn payload_value_skips_null_and_empty_only() {
        let payload = json!({"a": null, "b": "", "c": 0, "d": "x"});
        let payload = payload.as_object().unwrap();
        assert_eq!(payload_value(payload, &["a", "b", "c"]), Some(&json!(0)));
        assert_eq!(payload_value(payload, &["a", "b"]), None);
    }

    #[test]
    fn profile_comes_from_explicit_name_or_codex_home() {
        let env = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect()
        };
        assert_eq!(
            profile_name(&env(&[("CODEX_SHARED_PROFILE", " FEU ")])),
            "feu"
        );
        assert_eq!(
            profile_name(&env(&[("CODEX_HOME", "C:/Users/x/.codex-cy")])),
            "cy"
        );
        assert_eq!(
            profile_name(&env(&[("CODEX_SHARED_PROFILE", "lab")])),
            "personal"
        );
    }

    #[test]
    fn nearest_marker_supplies_the_project_id() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(MARKER_FILE_NAME),
            "[project]\nid = \"p-1\"\n",
        )
        .unwrap();
        let nested = dir.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(project_id_for(&nested.display().to_string()), "p-1");
    }

    #[test]
    fn global_project_is_the_python_uuid5() {
        // uuid.uuid5(uuid.NAMESPACE_URL, "codex-observability:global-agent-project")
        assert_eq!(global_project_id(), "5345ced0-b7ec-5941-86b3-5993256c692a");
    }
}
