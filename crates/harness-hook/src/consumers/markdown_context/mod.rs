//! Workspace Markdown metadata index and selective model-context loader.

mod selection;
mod state;
mod tree;

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use self::selection::relevant_directories;
use self::state::{sha256_bytes, CachePaths};
use self::tree::{resolve_workspace_root, WorkspaceMarkdownTree};
use crate::consumer::{Consumer, ConsumerResult};
use crate::events::python_str;
use crate::hook_context::HookContext;
use crate::shared_state::lifecycle_log::{details, emit};

pub const NAME: &str = "markdown-context";
const TIMEOUT: Duration = Duration::from_secs(20);
const DEFAULT_EVENT_CHARACTERS: usize = 8_000;
const DEFAULT_FILE_CHARACTERS: usize = 4_000;

pub struct MarkdownContextConsumer;

impl Consumer for MarkdownContextConsumer {
    fn name(&self) -> &'static str {
        NAME
    }

    fn subscribes_to(&self, event: &str) -> bool {
        matches!(
            event,
            "session.start" | "prompt.submit" | "tool.requested" | "tool.completed" | "session.end"
        )
    }

    fn is_enabled(&self, context: &HookContext) -> bool {
        context
            .env_value("AH_MARKDOWN_CONTEXT_ENABLED")
            .is_none_or(|value| {
                !matches!(value.to_ascii_lowercase().as_str(), "0" | "false" | "no")
            })
    }

    fn timeout(&self) -> Duration {
        TIMEOUT
    }

    fn run(&self, context: &HookContext) -> anyhow::Result<ConsumerResult> {
        let started = Instant::now();
        let output = match handle_event(context) {
            Ok(output) => {
                record(
                    context,
                    "markdown.context",
                    "succeeded",
                    started,
                    output.chars().count(),
                );
                output
            }
            Err(error) => {
                record_failure(context, &error, started);
                String::new()
            }
        };
        Ok(ConsumerResult::succeeded(NAME, output))
    }
}

// Mental model: resolve one canonical workspace → keep its metadata tree fresh → only read matched
// documents for prompt/pre-tool events → remember content hashes per session.
fn handle_event(context: &HookContext) -> io::Result<String> {
    let Some(cwd) = context.event.cwd.as_deref().map(PathBuf::from) else {
        return Ok(String::new());
    };
    if !cwd.is_dir() {
        return Ok(String::new());
    }
    let workspace_root = resolve_workspace_root(&cwd);
    let cache = CachePaths::new(
        &context.harness_root,
        &workspace_root,
        &context.event.session_id,
    );

    match context.event.event.as_str() {
        "session.start" | "tool.completed" => {
            let tree = WorkspaceMarkdownTree::build(&workspace_root).map_err(io::Error::other)?;
            cache.save_tree(&tree)?;
            Ok(String::new())
        }
        "session.end" => {
            cache.remove_session();
            Ok(String::new())
        }
        "prompt.submit" | "tool.requested" => {
            let tree = load_or_build_tree(&cache, &workspace_root)?;
            let query = relevance_query(context);
            let directories = relevant_directories(&tree, &query);
            render_selected_context(context, &cache, &tree, &directories)
        }
        _ => Ok(String::new()),
    }
}

fn load_or_build_tree(cache: &CachePaths, root: &Path) -> io::Result<WorkspaceMarkdownTree> {
    match cache.load_tree() {
        Ok(tree) if tree.schema_version == 1 && tree.canonical_root == root => Ok(tree),
        _ => {
            let tree = WorkspaceMarkdownTree::build(root).map_err(io::Error::other)?;
            cache.save_tree(&tree)?;
            Ok(tree)
        }
    }
}

fn relevance_query(context: &HookContext) -> String {
    match context.event.event.as_str() {
        "prompt.submit" => context
            .event
            .data
            .get("prompt")
            .map(python_str)
            .unwrap_or_default(),
        "tool.requested" => context
            .event
            .data
            .get("tool")
            .and_then(Value::as_object)
            .and_then(|tool| tool.get("input"))
            .map(|input| serde_json::to_string(input).unwrap_or_default())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn render_selected_context(
    context: &HookContext,
    cache: &CachePaths,
    tree: &WorkspaceMarkdownTree,
    directories: &[PathBuf],
) -> io::Result<String> {
    let event_limit = configured_limit(
        context,
        "AH_MARKDOWN_CONTEXT_EVENT_CHARS",
        DEFAULT_EVENT_CHARACTERS,
    );
    let file_limit = configured_limit(
        context,
        "AH_MARKDOWN_CONTEXT_FILE_CHARS",
        DEFAULT_FILE_CHARACTERS,
    );
    let candidates = candidate_files(tree, directories);

    cache.update_session(|state| {
        let mut output = String::new();
        let mut deferred = Vec::new();
        for relative in &candidates {
            let absolute = tree.canonical_root.join(relative);
            let Ok(canonical) = absolute.canonicalize() else {
                continue;
            };
            if !canonical.starts_with(&tree.canonical_root) {
                continue;
            }
            let bytes = match std::fs::read(&canonical) {
                Ok(bytes) => bytes,
                Err(_) => continue,
            };
            let hash = sha256_bytes(&bytes);
            let key = relative.to_string_lossy().replace('\\', "/");
            if state.loaded_content_hashes.get(&key) == Some(&hash) {
                continue;
            }
            let content = String::from_utf8_lossy(&bytes);
            let bounded = bounded_head_tail(&content, file_limit);
            let section = format!("Markdown context `{key}`:\n{bounded}\n\n");
            if output.chars().count() + section.chars().count() > event_limit {
                deferred.push(key);
                continue;
            }
            output.push_str(&section);
            state.loaded_content_hashes.insert(key, hash);
        }
        if !deferred.is_empty() {
            let notice = format!("Deferred relevant Markdown: {}", deferred.join(", "));
            let remaining = event_limit.saturating_sub(output.chars().count());
            output.push_str(&notice.chars().take(remaining).collect::<String>());
        }
        Ok(output.trim_end().to_owned())
    })
}

fn candidate_files(tree: &WorkspaceMarkdownTree, directories: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    for directory in directories {
        for file in tree.markdown_files_in(directory) {
            if seen.insert(file.clone()) {
                files.push(file);
            }
        }
    }
    files
}

fn configured_limit(context: &HookContext, key: &str, default: usize) -> usize {
    context
        .env_value(key)
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| (256..=100_000).contains(value))
        .unwrap_or(default)
}

fn bounded_head_tail(text: &str, limit: usize) -> String {
    let count = text.chars().count();
    if count <= limit {
        return text.to_owned();
    }
    let head = limit * 3 / 4;
    let tail = limit.saturating_sub(head);
    let beginning = text.chars().take(head).collect::<String>();
    let ending = text
        .chars()
        .rev()
        .take(tail)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("{beginning}\n… truncated {count} characters …\n{ending}")
}

fn record(context: &HookContext, operation: &str, outcome: &str, started: Instant, chars: usize) {
    emit(
        &context.log_dir().join("markdown-context.lifecycle.jsonl"),
        NAME,
        operation,
        outcome,
        details([
            ("event", json!(context.event.event)),
            (
                "durationMs",
                json!(started.elapsed().as_secs_f64() * 1000.0),
            ),
            ("outputCharacters", json!(chars)),
        ]),
    );
}

fn record_failure(context: &HookContext, error: &io::Error, started: Instant) {
    emit(
        &context.log_dir().join("markdown-context.lifecycle.jsonl"),
        NAME,
        "markdown.context",
        "failed",
        details([
            ("event", json!(context.event.event)),
            (
                "durationMs",
                json!(started.elapsed().as_secs_f64() * 1000.0),
            ),
            ("errorKind", json!(format!("{:?}", error.kind()))),
        ]),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Map};

    #[test]
    fn bounds_large_files_with_head_and_tail() {
        let bounded = bounded_head_tail(&"x".repeat(100), 20);
        assert!(bounded.contains("truncated 100 characters"));
        assert!(bounded.chars().count() < 100);
    }

    #[test]
    fn session_deduplicates_unchanged_content_and_reloads_changes() {
        let workspace = tempfile::tempdir().unwrap();
        let harness = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("README.md"), "first").unwrap();
        let tree = WorkspaceMarkdownTree::build(workspace.path()).unwrap();
        let cache = CachePaths::new(harness.path(), workspace.path(), "session-1");
        let payload = json!({"session_id": "session-1", "cwd": workspace.path()});
        let payload = payload.as_object().unwrap().clone();
        let context = HookContext {
            event: crate::events::make_event(
                "prompt.submit",
                "codex",
                "personal",
                &payload,
                "UserPromptSubmit",
                Map::new(),
            ),
            native_payload: Value::Object(payload),
            native_bytes: Vec::new(),
            env: Default::default(),
            harness_root: harness.path().to_path_buf(),
        };
        let directories = [PathBuf::new()];

        let first = render_selected_context(&context, &cache, &tree, &directories).unwrap();
        let second = render_selected_context(&context, &cache, &tree, &directories).unwrap();
        std::fs::write(workspace.path().join("README.md"), "second").unwrap();
        let changed = render_selected_context(&context, &cache, &tree, &directories).unwrap();

        assert!(first.contains("first"));
        assert!(second.is_empty());
        assert!(changed.contains("second"));
    }
}
