//! `context` consumer (`core/context/consumer.py` + herdr-bridge `build_context`): on `session.start`, fetch
//! `GET {OBS_ENDPOINT}/v1/projects/<id>/context/bootstrap?profile=<instance>&source=<source>` (2 s timeouts)
//! and print bounded plain text; the agent's adapter wraps it into its SessionStart output.
//!
//! - Project id: nearest `observability.project.toml` walking up from the event cwd, else the global
//!   project `uuid5(NAMESPACE_URL, "codex-observability:global-agent-project")`.
//! - Limit: 3000 characters for `source == "compact"`, else 6000.
//! - Any failure (hub down, HTTP error, bad JSON, malformed item, unreadable marker) prints nothing.

use std::path::{Path, PathBuf};
use std::time::Duration;

use obs_cli::project_resolution::find_project_marker;
use obs_cli::python_compat::uuid5_url_text;
use serde_json::{Map, Value};

use crate::consumer::{Consumer, ConsumerResult};
use crate::events::{is_python_truthy, python_str};
use crate::hook_context::HookContext;
use crate::runtime_env::python_strip;

pub const NAME: &str = "context";
pub const TIMEOUT: Duration = Duration::from_secs(20);
pub const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:4319";
pub const GLOBAL_PROJECT_NAME: &str = "codex-observability:global-agent-project";
const HUB_TIMEOUT: Duration = Duration::from_secs(2);
const HEADER: &str =
    "Shared project context (database-derived; query older evidence only when needed):\n\n";
const CONSTRAINT_LIMIT: usize = 12;
const HANDOFF_LIMIT: usize = 5;
const HANDOFF_CONTENT_CHARS: usize = 1200;
const ERROR_LIMIT: usize = 3;
const ERROR_SUMMARY_CHARS: usize = 500;

pub struct ContextConsumer;

impl Consumer for ContextConsumer {
    fn name(&self) -> &'static str {
        NAME
    }

    fn subscribes_to(&self, event: &str) -> bool {
        event == "session.start"
    }

    fn timeout(&self) -> Duration {
        TIMEOUT
    }

    fn run(&self, context: &HookContext) -> anyhow::Result<ConsumerResult> {
        Ok(ConsumerResult::succeeded(
            NAME,
            session_context_text(context),
        ))
    }
}

// Mental model: only session.start → resolve endpoint, source, project → fetch hub JSON → render sections
// → bound to the source's limit.
/// `handle(event, env)`.
pub fn session_context_text(context: &HookContext) -> String {
    let event = &context.event;
    if event.event != "session.start" {
        return String::new();
    }

    let endpoint = context
        .env
        .get("OBS_ENDPOINT")
        .map_or(DEFAULT_ENDPOINT, String::as_str);
    let source = event
        .data
        .get("source")
        .filter(|value| is_python_truthy(value))
        .map(python_str)
        .unwrap_or_default();
    let Some(project_id) = project_id(event.cwd.as_deref(), &context.harness_root) else {
        return String::new();
    };

    let url = bootstrap_url(endpoint, &project_id, &event.instance, &source);
    fetch_json(&url)
        .and_then(|document| render_context(&document, &source))
        .unwrap_or_default()
}

/// `limit_for(source)`.
#[inline]
pub fn limit_for(source: &str) -> usize {
    if source == "compact" {
        3000
    } else {
        6000
    }
}

/// `GLOBAL_PROJECT_ID`.
pub fn global_project_id() -> String {
    uuid5_url_text(GLOBAL_PROJECT_NAME)
}

/// `_project_id(cwd)`; `None` when a marker exists but cannot be read or has no `project.id`
/// (Python raised there, and the consumer printed nothing). A relative cwd resolves against the harness
/// root, which was the Python consumer's working directory.
pub fn project_id(cwd: Option<&str>, harness_root: &Path) -> Option<String> {
    let Some(cwd) = cwd.filter(|cwd| !cwd.is_empty()) else {
        return Some(global_project_id());
    };
    let start = absolute_from(harness_root, cwd);
    match find_project_marker(&start) {
        Ok(Some(marker)) => marker.project_id_text().ok(),
        Ok(None) => Some(global_project_id()),
        Err(_) => None,
    }
}

/// `f"{endpoint}/v1/projects/{quote(project_id)}/context/bootstrap?{urlencode({profile, source})}"`.
pub fn bootstrap_url(endpoint: &str, project_id: &str, profile: &str, source: &str) -> String {
    format!(
        "{endpoint}/v1/projects/{}/context/bootstrap?profile={}&source={}",
        percent_encode(project_id, "/", false),
        percent_encode(profile, "", true),
        percent_encode(source, "", true),
    )
}

// Mental model: collect the labelled sections in Python order; nothing collected → no output; a
// malformed list item → no output (Python raised AttributeError and printed nothing).
/// herdr-bridge `build_context` after the HTTP call.
pub fn render_context(document: &Value, source: &str) -> Option<String> {
    let Value::Object(document) = document else {
        return Some(String::new());
    };

    let mut sections = Vec::new();
    if let Some(Value::Object(context)) = document.get("context") {
        sections.extend(summary_sections(context));
        sections.extend(constraints_section(context));
    }
    sections.extend(handoffs_section(document)?);
    sections.extend(errors_section(document)?);
    sections.extend(code_version_section(document));

    if sections.is_empty() {
        return Some(String::new());
    }
    let text = format!("{HEADER}{}", sections.join("\n\n"));
    Some(text.chars().take(limit_for(source)).collect())
}

fn summary_sections(context: &Map<String, Value>) -> Vec<String> {
    [
        ("Vision", "vision_summary"),
        ("Architecture", "architecture_summary"),
        ("Current goal", "current_goal_summary"),
    ]
    .into_iter()
    .filter_map(|(label, key)| {
        let text = truthy_text(context.get(key));
        let stripped = python_strip(&text);
        (!stripped.is_empty()).then(|| format!("{label}: {stripped}"))
    })
    .collect()
}

fn constraints_section(context: &Map<String, Value>) -> Option<String> {
    let constraints = non_empty_list(context.get("constraints"))?;
    let joined = constraints
        .iter()
        .take(CONSTRAINT_LIMIT)
        .map(python_str)
        .collect::<Vec<_>>()
        .join("; ");
    Some(format!("Constraints: {joined}"))
}

/// `Some(None)` = no handoffs; `None` = a handoff item is not an object.
fn handoffs_section(document: &Map<String, Value>) -> Option<Option<String>> {
    let Some(handoffs) = non_empty_list(document.get("handoffs")) else {
        return Some(None);
    };
    let lines = handoffs
        .iter()
        .take(HANDOFF_LIMIT)
        .map(|item| {
            let item = item.as_object()?;
            let kind = get_or_text(item, "handoff_kind", "progress");
            let content: String = get_or_text(item, "content", "")
                .chars()
                .take(HANDOFF_CONTENT_CHARS)
                .collect();
            Some(format!("- [{kind}] {content}"))
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Some(format!("Live handoff:\n{}", lines.join("\n"))))
}

/// `Some(None)` = no errors; `None` = an error item is not an object.
fn errors_section(document: &Map<String, Value>) -> Option<Option<String>> {
    let Some(errors) = non_empty_list(document.get("errors")) else {
        return Some(None);
    };
    let lines = errors
        .iter()
        .take(ERROR_LIMIT)
        .map(|item| {
            let item = item.as_object()?;
            let summary: String = get_or_text(item, "normalized_summary", "")
                .chars()
                .take(ERROR_SUMMARY_CHARS)
                .collect();
            Some(format!("- {summary}"))
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Some(format!("Unresolved errors:\n{}", lines.join("\n"))))
}

fn code_version_section(document: &Map<String, Value>) -> Option<String> {
    let Some(Value::Object(code_version)) = document.get("code_version") else {
        return None;
    };
    let commit = truthy_text_or(code_version.get("commit_sha"), "uncommitted");
    let branch = truthy_text_or(code_version.get("branch_name"), "no branch");
    Some(format!("Code version: {commit} ({branch})"))
}

/// `urllib.request.urlopen(url, timeout=2.0)` + `json.load`; any failure → `None`.
fn fetch_json(url: &str) -> Option<Value> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(HUB_TIMEOUT)
        .timeout_read(HUB_TIMEOUT)
        .build();
    let response = agent.get(url).call().ok()?;
    let mut body = Vec::new();
    std::io::Read::read_to_end(&mut response.into_reader(), &mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

/// `str(value or "")`.
#[inline]
fn truthy_text(value: Option<&Value>) -> String {
    truthy_text_or(value, "")
}

/// `value or default`, rendered with `str()`.
#[inline]
fn truthy_text_or(value: Option<&Value>, default: &str) -> String {
    value
        .filter(|value| is_python_truthy(value))
        .map_or_else(|| default.to_owned(), python_str)
}

/// `str(item.get(key, default))` (a present `null` is `"None"`).
#[inline]
fn get_or_text(item: &Map<String, Value>, key: &str, default: &str) -> String {
    item.get(key).map_or_else(|| default.to_owned(), python_str)
}

/// `isinstance(value, list) and value`.
#[inline]
fn non_empty_list(value: Option<&Value>) -> Option<&Vec<Value>> {
    value
        .and_then(Value::as_array)
        .filter(|items| !items.is_empty())
}

#[inline]
fn absolute_from(base: &Path, path: &str) -> PathBuf {
    let candidate = PathBuf::from(path);
    if candidate.is_absolute() {
        candidate
    } else {
        base.join(candidate)
    }
}

/// `urllib.parse.quote` (`plus = false`, `safe = "/"`) and `quote_plus` (`plus = true`, `safe = ""`):
/// letters, digits and `_.-~` stay, space becomes `+` in plus mode, every other UTF-8 byte is `%XX`.
pub fn percent_encode(text: &str, safe: &str, plus: bool) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        let character = char::from(byte);
        if byte.is_ascii_alphanumeric()
            || b"_.-~".contains(&byte)
            || (byte.is_ascii() && safe.contains(character))
        {
            encoded.push(character);
        } else if plus && byte == b' ' {
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
    fn compact_context_is_shorter() {
        assert_eq!(limit_for("compact"), 3000);
        assert_eq!(limit_for("startup"), 6000);
    }

    #[test]
    fn global_project_id_matches_python_uuid5() {
        assert_eq!(
            global_project_id(),
            uuid5_url_text("codex-observability:global-agent-project")
        );
        assert_eq!(project_id(None, Path::new(".")), Some(global_project_id()));
    }

    #[test]
    fn url_matches_python_quoting() {
        assert_eq!(
            bootstrap_url("http://h:1", "a b/c", "work profile", "résumé&x"),
            "http://h:1/v1/projects/a%20b/c/context/bootstrap?profile=work+profile&source=r%C3%A9sum%C3%A9%26x"
        );
    }

    #[test]
    fn renders_sections_in_python_order() {
        let document = json!({
            "context": {"vision_summary": " Ship agent-harness ", "constraints": ["no secrets", 3]},
            "handoffs": [{"handoff_kind": "blocker", "content": "wait"}, {"content": null}],
            "errors": [{"normalized_summary": "boom"}],
            "code_version": {"commit_sha": "", "branch_name": "main"},
        });
        let text = render_context(&document, "startup").unwrap();
        assert_eq!(
            text,
            format!(
                "{HEADER}Vision: Ship agent-harness\n\nConstraints: no secrets; 3\n\nLive handoff:\n- [blocker] wait\n- [progress] None\n\nUnresolved errors:\n- boom\n\nCode version: uncommitted (main)"
            )
        );
    }

    #[test]
    fn empty_or_malformed_documents_print_nothing() {
        assert_eq!(render_context(&json!({}), "startup"), Some(String::new()));
        assert_eq!(render_context(&json!([1]), "startup"), Some(String::new()));
        assert_eq!(render_context(&json!({"handoffs": ["x"]}), "startup"), None);
    }

    #[test]
    fn compact_source_truncates_to_three_thousand_characters() {
        let document = json!({"context": {"vision_summary": "ñ".repeat(10_000)}});
        assert_eq!(
            render_context(&document, "compact")
                .unwrap()
                .chars()
                .count(),
            3000
        );
    }

    #[test]
    fn unreachable_hub_yields_nothing() {
        assert!(fetch_json("http://127.0.0.1:9/v1/projects/x/context/bootstrap").is_none());
    }

    /// One-shot HTTP server answering any request with `body` (the Python test's `FakeHub`).
    fn fake_hub(body: &'static str) -> (String, std::sync::mpsc::Receiver<String>) {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut header = String::new();
            while reader.read_line(&mut header).unwrap() > 2 {
                header.clear();
            }
            let mut stream = stream;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
            sender.send(request_line).unwrap();
        });
        (endpoint, receiver)
    }

    fn session_start_context(endpoint: &str, cwd: &Path) -> HookContext {
        let payload = json!({"session_id": "s", "cwd": cwd.to_string_lossy(), "source": "startup"});
        let payload = payload.as_object().unwrap().clone();
        let data = json!({"source": "startup"}).as_object().unwrap().clone();
        HookContext {
            event: crate::events::make_event(
                "session.start",
                "claude",
                "default",
                &payload,
                "SessionStart",
                data,
            ),
            native_payload: Value::Object(payload),
            native_bytes: Vec::new(),
            env: [("OBS_ENDPOINT".to_owned(), endpoint.to_owned())]
                .into_iter()
                .collect(),
            harness_root: PathBuf::from("."),
        }
    }

    #[test]
    fn context_text_from_hub() {
        let (endpoint, requests) = fake_hub(
            r#"{"context": {"vision_summary": "Ship agent-harness", "constraints": ["no secrets"]}}"#,
        );
        let cwd = tempfile::tempdir().unwrap();
        let text = session_context_text(&session_start_context(&endpoint, cwd.path()));

        assert!(text.contains("Vision: Ship agent-harness"), "{text}");
        assert!(text.contains("Constraints: no secrets"));
        let request_line = requests.recv().unwrap();
        assert!(
            request_line.contains("/context/bootstrap?profile=default&source=startup"),
            "{request_line}"
        );
    }
}
