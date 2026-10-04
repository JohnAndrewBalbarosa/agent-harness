//! Packaged Rust discovery/read command, separate from fail-open native hook dispatch.
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use serde_json::{json, Value};

use super::{
    cache_for, enabled,
    index::MarkdownIndex,
    policy,
    reader::{read_selected, ReadRequest},
    record, telemetry,
};
use crate::hook_context::HookContext;

#[derive(Default)]
struct Arguments {
    command: String,
    values: BTreeMap<String, String>,
    files: Vec<PathBuf>,
    refresh: bool,
    force: bool,
}

pub fn run(arguments: &[String]) -> ExitCode {
    let result = execute(arguments);
    let (code, output) = match result {
        Ok(output) => (ExitCode::SUCCESS, output),
        Err(error) => (
            ExitCode::from(2),
            json!({"error": error.to_string(), "status": "failed"}),
        ),
    };
    let mut stdout = io::stdout().lock();
    if serde_json::to_writer(&mut stdout, &output).is_err() || stdout.write_all(b"\n").is_err() {
        return ExitCode::from(2);
    }
    code
}

// Mental model: strict arguments → configured allowlist → cached relative paths → explicit read.
pub fn execute(arguments: &[String]) -> io::Result<Value> {
    let args = Arguments::parse(arguments)?;
    let context = args.context()?;
    let started = Instant::now();
    let result = execute_with(&context, &args, started);
    if let Err(error) = &result {
        record(
            &context,
            "markdown.command.failed",
            "failed",
            started,
            json!({"command": args.command, "errorKind": format!("{:?}", error.kind())}),
        );
    }
    result
}

fn execute_with(context: &HookContext, args: &Arguments, started: Instant) -> io::Result<Value> {
    if !enabled(context) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Markdown discovery is disabled",
        ));
    }
    let cwd = PathBuf::from(context.event.cwd.as_deref().unwrap_or_default());
    let root = policy::allowed_repository(context, &cwd)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Select an allowlisted repository below Projects",
        )
    })?;
    let cache = cache_for(context, &root);
    let (index, rebuilt) = MarkdownIndex::load(&cache, &root, args.refresh)?;
    if args.command == "index" {
        let prefix = args
            .values
            .get("--prefix")
            .cloned()
            .unwrap_or_default()
            .replace('\\', "/");
        let paths = index
            .paths()
            .iter()
            .map(|path| policy::normalized_path(path))
            .filter(|path| {
                prefix.is_empty() || path == &prefix || path.starts_with(&format!("{prefix}/"))
            })
            .collect::<Vec<_>>();
        let offset = args.number("--offset", 0)?;
        let count = args.number("--limit", 80)?.clamp(1, 200);
        let mut characters = 0;
        let page = paths
            .iter()
            .skip(offset)
            .take(count)
            .take_while(|path| {
                characters += path.chars().count() + 4;
                characters <= 12_000
            })
            .cloned()
            .collect::<Vec<_>>();
        let next = offset.saturating_add(page.len());
        let candidates = page
            .iter()
            .filter(|path| {
                PathBuf::from(path).file_name().is_some_and(|name| {
                    name.eq_ignore_ascii_case("AGENTS.md") || name.eq_ignore_ascii_case("CLAUDE.md")
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        let output = json!({"paths": page, "total": paths.len(), "nextOffset": (next < paths.len()).then_some(next),
            "rebuilt": rebuilt, "nativeInstructionCandidates": candidates, "nativeLoading": "unverified"});
        record(
            context,
            "markdown.index",
            "succeeded",
            started,
            json!({"rebuilt": rebuilt, "pathCount": paths.len(), "outputCharacters": characters}),
        );
        if let Err(error) = telemetry::record_batch(context, &root, "path_inventory", &[], started)
        {
            telemetry::record_failure(context, error);
        }
        return Ok(output);
    }
    let request = ReadRequest {
        files: args.files.clone(),
        offset: args.number("--offset", 0)?,
        max_characters: args.number("--max-chars", 12_000)?.clamp(1, 100_000),
        force: args.force,
    };
    let results = read_selected(context, &cache, &index, &request)?;
    // Telemetry failures remain isolated from the document result.
    telemetry::record_reads(context, &root, &results, started);
    record(
        context,
        "markdown.read",
        "succeeded",
        started,
        json!({"selectedFiles": results.len(), "rebuilt": rebuilt,
            "outputCharacters": results.iter().map(|item| item.content.chars().count()).sum::<usize>(),
            "dedupeSkips": results.iter().filter(|item| item.outcome == "already_observed").count()}),
    );
    Ok(
        json!({"documents": results, "delivery": "emitted; walang model-consumption acknowledgment"}),
    )
}

impl Arguments {
    fn parse(arguments: &[String]) -> io::Result<Self> {
        let Some(command) = arguments
            .first()
            .filter(|command| matches!(command.as_str(), "index" | "read"))
        else {
            return Err(io::Error::new(io::ErrorKind::InvalidInput,
                "markdown index|read [--cwd PATH] [--session ID] [--agent AGENT] [--instance NAME] [--epoch ID] [--file PATH ...] [--offset N] [--limit N] [--max-chars N] [--prefix PATH] [--refresh] [--force]"));
        };
        let mut args = Self {
            command: command.clone(),
            ..Self::default()
        };
        let mut position = 1;
        while position < arguments.len() {
            match arguments[position].as_str() {
                "--refresh" => args.refresh = true,
                "--force" => args.force = true,
                key @ ("--cwd" | "--session" | "--agent" | "--instance" | "--epoch" | "--file"
                | "--offset" | "--limit" | "--max-chars" | "--prefix") => {
                    position += 1;
                    let value = arguments
                        .get(position)
                        .filter(|value| !value.starts_with("--"))
                        .ok_or_else(|| {
                            io::Error::new(io::ErrorKind::InvalidInput, "Missing option value")
                        })?;
                    if key == "--file" {
                        args.files.push(PathBuf::from(value.replace('\\', "/")));
                    } else {
                        args.values.insert(key.into(), value.clone());
                    }
                }
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Unknown Markdown option",
                    ))
                }
            }
            position += 1;
        }
        if args.command == "read" && (args.files.is_empty() || args.files.len() > 32) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Select 1–32 files with --file",
            ));
        }
        Ok(args)
    }

    fn number(&self, key: &str, default: usize) -> io::Result<usize> {
        self.values.get(key).map_or(Ok(default), |value| {
            value
                .parse()
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
        })
    }

    fn context(&self) -> io::Result<HookContext> {
        let harness_root = crate::router::resolve_harness_root();
        let mut env = crate::runtime_env::runtime_env(
            &harness_root.join(".env"),
            &crate::runtime_env::process_environment(),
        );
        let session = self
            .values
            .get("--session")
            .cloned()
            .or_else(|| env.get("AH_SESSION_ID").cloned())
            .or_else(|| env.get("CODEX_THREAD_ID").cloned());
        if self.command == "read"
            && session
                .as_ref()
                .is_none_or(|value| value.is_empty() || value == "unknown")
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Supply --session ID for versioned read tracking",
            ));
        }
        if let Some(epoch) = self.values.get("--epoch") {
            env.insert("AH_MARKDOWN_CONTEXT_EPOCH".into(), epoch.clone());
        }
        let cwd = self
            .values
            .get("--cwd")
            .map(PathBuf::from)
            .map_or_else(std::env::current_dir, Ok)?;
        let payload =
            json!({"session_id": session.unwrap_or_else(|| "index-only".into()), "cwd": cwd});
        let agent = self
            .values
            .get("--agent")
            .map(String::as_str)
            .or_else(|| env.get("AH_AGENT").map(String::as_str))
            .unwrap_or("codex");
        let instance = self
            .values
            .get("--instance")
            .map(String::as_str)
            .or_else(|| env.get("AH_INSTANCE").map(String::as_str))
            .unwrap_or("personal");
        let event = crate::events::make_event(
            "tool.requested",
            agent,
            instance,
            payload.as_object().unwrap(),
            "MarkdownCommand",
            Default::default(),
        );
        Ok(HookContext {
            event,
            native_payload: payload,
            native_bytes: Vec::new(),
            env,
            harness_root,
        })
    }
}
