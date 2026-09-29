//! `build_context(project_id, profile, source)` (`hook.py`): the hub's project bootstrap rendered as bounded
//! plain text for `SessionStart` (the Python context consumer reuses this same function).
//!
//! `GET {OBS_ENDPOINT}/v1/projects/{quote(project_id)}/context/bootstrap?profile=..&source=..` with a 2 s
//! timeout; any transport, HTTP-status, or JSON failure yields `""`.

use std::time::Duration;

use serde_json::{Map, Value};

use super::identity::is_python_space;
use crate::events::{is_python_truthy, python_str};

pub const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:4319";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const HEADER: &str =
    "Shared project context (database-derived; query older evidence only when needed):\n\n";
const FULL_LIMIT: usize = 6000;
const COMPACT_LIMIT: usize = 3000;
const SUMMARY_FIELDS: [(&str, &str); 3] = [
    ("Vision", "vision_summary"),
    ("Architecture", "architecture_summary"),
    ("Current goal", "current_goal_summary"),
];
const MAX_CONSTRAINTS: usize = 12;
const MAX_HANDOFFS: usize = 5;
const HANDOFF_CONTENT_LIMIT: usize = 1200;
const MAX_ERRORS: usize = 3;
const ERROR_SUMMARY_LIMIT: usize = 500;

/// Fetches the bootstrap document and renders it; `""` when the hub is unreachable or has nothing.
pub fn build_context(endpoint: &str, project_id: &str, profile: &str, source: &str) -> String {
    fetch_bootstrap(&bootstrap_url(endpoint, project_id, profile, source))
        .map(|document| render_context(&document, source))
        .unwrap_or_default()
}

/// The request URL, quoted like `urllib.parse.quote` / `urlencode`.
pub fn bootstrap_url(endpoint: &str, project_id: &str, profile: &str, source: &str) -> String {
    format!(
        "{endpoint}/v1/projects/{}/context/bootstrap?profile={}&source={}",
        percent_encode(project_id, "/", false),
        percent_encode(profile, "", true),
        percent_encode(source, "", true),
    )
}

fn fetch_bootstrap(url: &str) -> Option<Map<String, Value>> {
    let agent = ureq::AgentBuilder::new().timeout(REQUEST_TIMEOUT).build();
    let response = agent.get(url).call().ok()?;
    let document: Value = serde_json::from_reader(response.into_reader()).ok()?;
    document.as_object().cloned()
}

// Mental model: collect sections (summaries, constraints, handoffs, errors, code version) → header + join →
// cut at 6000 chars (3000 for `compact`).
pub fn render_context(document: &Map<String, Value>, source: &str) -> String {
    let mut sections = Vec::new();
    if let Some(context) = document.get("context").and_then(Value::as_object) {
        push_summaries(&mut sections, context);
        push_constraints(&mut sections, context);
    }
    push_handoffs(&mut sections, document);
    push_errors(&mut sections, document);
    push_code_version(&mut sections, document);

    if sections.is_empty() {
        return String::new();
    }
    let limit = if source == "compact" {
        COMPACT_LIMIT
    } else {
        FULL_LIMIT
    };
    format!("{HEADER}{}", sections.join("\n\n"))
        .chars()
        .take(limit)
        .collect()
}

fn push_summaries(sections: &mut Vec<String>, context: &Map<String, Value>) {
    for (label, key) in SUMMARY_FIELDS {
        let text = truthy_text(context.get(key)).unwrap_or_default();
        let text = text.trim_matches(is_python_space);
        if !text.is_empty() {
            sections.push(format!("{label}: {text}"));
        }
    }
}

fn push_constraints(sections: &mut Vec<String>, context: &Map<String, Value>) {
    let Some(constraints) = non_empty_list(context.get("constraints")) else {
        return;
    };
    let items: Vec<String> = constraints
        .iter()
        .take(MAX_CONSTRAINTS)
        .map(python_str)
        .collect();
    sections.push(format!("Constraints: {}", items.join("; ")));
}

fn push_handoffs(sections: &mut Vec<String>, document: &Map<String, Value>) {
    let Some(handoffs) = non_empty_list(document.get("handoffs")) else {
        return;
    };
    let lines: Vec<String> = handoffs
        .iter()
        .take(MAX_HANDOFFS)
        .map(|item| {
            let kind = field_text(item, "handoff_kind", "progress");
            let content: String = field_text(item, "content", "")
                .chars()
                .take(HANDOFF_CONTENT_LIMIT)
                .collect();
            format!("- [{kind}] {content}")
        })
        .collect();
    sections.push(format!("Live handoff:\n{}", lines.join("\n")));
}

fn push_errors(sections: &mut Vec<String>, document: &Map<String, Value>) {
    let Some(errors) = non_empty_list(document.get("errors")) else {
        return;
    };
    let lines: Vec<String> = errors
        .iter()
        .take(MAX_ERRORS)
        .map(|item| {
            let summary: String = field_text(item, "normalized_summary", "")
                .chars()
                .take(ERROR_SUMMARY_LIMIT)
                .collect();
            format!("- {summary}")
        })
        .collect();
    sections.push(format!("Unresolved errors:\n{}", lines.join("\n")));
}

fn push_code_version(sections: &mut Vec<String>, document: &Map<String, Value>) {
    let Some(code_version) = document.get("code_version").and_then(Value::as_object) else {
        return;
    };
    let commit =
        truthy_text(code_version.get("commit_sha")).unwrap_or_else(|| "uncommitted".to_owned());
    let branch =
        truthy_text(code_version.get("branch_name")).unwrap_or_else(|| "no branch".to_owned());
    sections.push(format!("Code version: {commit} ({branch})"));
}

/// `str(item.get(key, default))` (a present `null` renders as `None`, like Python).
#[inline]
fn field_text(item: &Value, key: &str, default: &str) -> String {
    item.get(key).map_or_else(|| default.to_owned(), python_str)
}

/// `str(value)` when truthy.
#[inline]
fn truthy_text(value: Option<&Value>) -> Option<String> {
    value
        .filter(|value| is_python_truthy(value))
        .map(python_str)
}

#[inline]
fn non_empty_list(value: Option<&Value>) -> Option<&Vec<Value>> {
    value
        .and_then(Value::as_array)
        .filter(|items| !items.is_empty())
}

/// `urllib.parse.quote(text, safe)` / `quote_plus` (letters, digits and `_.-~` are never encoded).
pub fn percent_encode(text: &str, safe: &str, space_as_plus: bool) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        let character = char::from(byte);
        if byte.is_ascii_alphanumeric()
            || b"_.-~".contains(&byte)
            || (byte.is_ascii() && safe.contains(character))
        {
            encoded.push(character);
        } else if byte == b' ' && space_as_plus {
            encoded.push('+');
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn url_quotes_like_urllib() {
        assert_eq!(
            bootstrap_url("http://h", "a b/c", "cy", "start up&x"),
            "http://h/v1/projects/a%20b/c/context/bootstrap?profile=cy&source=start+up%26x"
        );
    }

    #[test]
    fn renders_every_section_in_python_order() {
        let document = json!({
            "context": {"vision_summary": " V ", "architecture_summary": "", "current_goal_summary": "G",
                        "constraints": ["c1", 2]},
            "handoffs": [{"handoff_kind": "note", "content": "h1"}, {"content": null}],
            "errors": [{"normalized_summary": "e1"}],
            "code_version": {"commit_sha": "", "branch_name": "main"},
        });
        let text = render_context(document.as_object().unwrap(), "startup");
        assert_eq!(
            text,
            format!(
                "{HEADER}Vision: V\n\nCurrent goal: G\n\nConstraints: c1; 2\n\nLive handoff:\n- [note] h1\n- [progress] None\n\nUnresolved errors:\n- e1\n\nCode version: uncommitted (main)"
            )
        );
    }

    #[test]
    fn empty_bootstrap_renders_nothing_and_compact_is_shorter() {
        assert_eq!(render_context(&Map::new(), "startup"), "");
        let long = json!({"context": {"vision_summary": "x".repeat(10_000)}});
        assert_eq!(
            render_context(long.as_object().unwrap(), "compact")
                .chars()
                .count(),
            COMPACT_LIMIT
        );
    }

    #[test]
    fn unreachable_hub_yields_empty_context() {
        assert_eq!(
            build_context("http://127.0.0.1:9", "p", "personal", "startup"),
            ""
        );
    }
}
