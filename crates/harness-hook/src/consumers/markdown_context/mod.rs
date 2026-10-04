//! Paths-only discovery; document bodies require explicit selections through the Rust CLI.
pub mod cli;
pub mod index;
pub mod observations;
pub mod policy;
pub mod reader;
pub mod state;
pub mod telemetry;
pub mod tree;

use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::json;

use self::index::MarkdownIndex;
use self::state::{sha256_bytes, CachePaths};
use crate::consumer::{Consumer, ConsumerResult};
use crate::hook_context::HookContext;
use crate::shared_state::lifecycle_log::{details, emit};

pub const NAME: &str = "markdown-context";

pub struct MarkdownContextConsumer;

impl Consumer for MarkdownContextConsumer {
    fn name(&self) -> &'static str {
        NAME
    }

    fn subscribes_to(&self, event: &str) -> bool {
        matches!(
            event,
            "session.start" | "prompt.submit" | "tool.completed" | "session.end"
        )
    }

    fn is_enabled(&self, context: &HookContext) -> bool {
        enabled(context)
    }

    fn timeout(&self) -> Duration {
        Duration::from_secs(20)
    }

    fn run(&self, context: &HookContext) -> anyhow::Result<ConsumerResult> {
        let started = Instant::now();
        let output = match handle_event(context) {
            Ok(output) => output,
            Err(error) => {
                record(
                    context,
                    "markdown.context.failed",
                    "failed",
                    started,
                    json!({"errorKind": format!("{:?}", error.kind())}),
                );
                String::new()
            }
        };
        Ok(ConsumerResult::succeeded(NAME, output))
    }
}

pub fn enabled(context: &HookContext) -> bool {
    !context
        .env_value("AH_MARKDOWN_CONTEXT_ENABLED")
        .is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "0" | "false" | "no"))
}

// Mental model: allowlisted repository → paths on prompt → observed reads on completion.
// Pre-tool hooks never send bodies or drain deferred selections.
fn handle_event(context: &HookContext) -> io::Result<String> {
    let Some(cwd) = context.event.cwd.as_deref() else {
        return Ok(String::new());
    };
    let Some(root) = policy::allowed_repository(context, Path::new(cwd))? else {
        return Ok(String::new());
    };
    let cache = cache_for(context, &root);
    match context.event.event.as_str() {
        "session.end" => {
            cache.remove_session();
            Ok(String::new())
        }
        "tool.completed" => {
            observations::observe_tool_read(context, &cache, &root)?;
            Ok(String::new())
        }
        "session.start" => {
            MarkdownIndex::load(&cache, &root, false)?;
            Ok(String::new())
        }
        "prompt.submit" => render_path_inventory(context, &cache, &root),
        _ => Ok(String::new()),
    }
}

pub fn cache_for(context: &HookContext, root: &Path) -> CachePaths {
    let epoch = context
        .env_value("AH_MARKDOWN_CONTEXT_EPOCH")
        .unwrap_or("0");
    let identity = format!(
        "{}:{}:{}:{epoch}",
        context.event.agent, context.event.instance, context.event.session_id
    );
    CachePaths::new(&context.harness_root, root, &identity)
}

fn render_path_inventory(
    context: &HookContext,
    cache: &CachePaths,
    root: &Path,
) -> io::Result<String> {
    let started = Instant::now();
    let (index, rebuilt) = MarkdownIndex::load(cache, root, false)?;
    let paths = index.paths();
    let serialized = serde_json::to_vec(&paths).map_err(io::Error::other)?;
    let hash = sha256_bytes(&serialized);
    let output = cache.update_session(|state| {
        if state.index_hash.as_ref() == Some(&hash) { return Ok(String::new()); }
        let limit = configured_limit(context, "AH_MARKDOWN_INDEX_CHARS", 6_000);
        let command = policy::normalized_path(&context.harness_root.join("core/harness.cmd"));
        let mut output = format!("Markdown paths (project-relative; walang file contents):\n\
            Piliin ang kailangan. Para sa index: \"{command}\" markdown index --cwd \"{}\".\n\
            Para basahin: \"{command}\" markdown read --cwd \"{}\" --session \"{}\" --agent {} --instance {} --file <relative-path>\n",
            policy::normalized_path(root), policy::normalized_path(root), context.event.session_id,
            context.event.agent, context.event.instance);
        for (position, path) in paths.iter().enumerate() {
            let line = format!("{}\n", policy::normalized_path(path));
            if output.chars().count() + line.chars().count() + 160 > limit {
                output.push_str(&format!("May {} pang paths; gamitin ang index --offset {position}. Walang automatic next batch.\n", paths.len() - position));
                break;
            }
            output.push_str(&line);
        }
        state.index_hash = Some(hash.clone());
        Ok(output)
    })?;
    record(
        context,
        "markdown.index",
        "succeeded",
        started,
        json!({"rebuilt": rebuilt, "pathCount": paths.len(), "indexHash": hash,
            "outputCharacters": output.chars().count(), "unchangedIndex": output.is_empty()}),
    );
    Ok(output)
}

pub fn configured_limit(context: &HookContext, key: &str, default: usize) -> usize {
    context
        .env_value(key)
        .and_then(|value| value.parse().ok())
        .filter(|value| (256..=100_000).contains(value))
        .unwrap_or(default)
}

pub fn record(
    context: &HookContext,
    operation: &str,
    outcome: &str,
    started: Instant,
    attributes: serde_json::Value,
) {
    emit(
        &context.log_dir().join("markdown-context.lifecycle.jsonl"),
        NAME,
        operation,
        outcome,
        details([
            ("event", json!(context.event.event)),
            ("session", json!(context.event.session_id)),
            ("turn", json!(context.event.turn_id)),
            (
                "durationMs",
                json!(started.elapsed().as_secs_f64() * 1000.0),
            ),
            ("result", attributes),
        ]),
    );
}
