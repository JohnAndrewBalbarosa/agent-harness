//! Metadata-only observations use the existing durable spool and normalized hub tables.
use std::io;
use std::path::Path;
use std::time::Instant;

use obs_cli::python_compat::{uuid5_url_text, uuid7_text};
use serde_json::{json, Value};

use super::reader::ReadResult;
use crate::consumers::usage_fallback::spool_enqueue;
use crate::hook_context::HookContext;

pub fn record_reads(context: &HookContext, root: &Path, results: &[ReadResult], started: Instant) {
    let documents = results.iter().map(|item| json!({"path": item.path, "hash": item.hash,
        "byteSize": item.byte_size, "source": "explicit_read", "coverage": item.coverage,
        "outcome": item.outcome, "startCharacter": item.start_character, "endCharacter": item.end_character,
        "suppliedCharacters": item.content.chars().count()})).collect::<Vec<_>>();
    if let Err(error) = record_batch(context, root, "explicit_selection", &documents, started) {
        record_failure(context, error);
    }
}

pub fn record_observation(
    context: &HookContext,
    root: &Path,
    path: &str,
    hash: &str,
    byte_size: usize,
    coverage: &str,
    source: &str,
) -> io::Result<()> {
    let documents = [
        json!({"path": path, "hash": hash, "byteSize": byte_size, "source": source,
        "coverage": coverage, "outcome": "observed", "suppliedCharacters": 0}),
    ];
    if let Err(error) = record_batch(
        context,
        root,
        "successful_tool_read",
        &documents,
        Instant::now(),
    ) {
        record_failure(context, error);
    }
    Ok(())
}

pub fn record_batch(
    context: &HookContext,
    root: &Path,
    reason: &str,
    documents: &[Value],
    started: Instant,
) -> io::Result<()> {
    if context.env_value("AH_MARKDOWN_TELEMETRY_ENABLED") == Some("0") {
        return Ok(());
    }
    // Do not enroll unregistered repositories or merge their documents into the global project.
    if !root.join("observability.project.toml").is_file() {
        return Ok(());
    }
    let registration =
        crate::consumers::obs::project_registration::register(root).map_err(io::Error::other)?;
    let spool = spool_enqueue::spool_path(&context.env, &context.harness_root);
    let _access = crate::shared_state::spool_access::lock_spool_access();
    let identity = context
        .native_payload
        .get("tool_use_id")
        .and_then(Value::as_str);
    let batch_id = identity
        .map(|id| {
            uuid5_url_text(&format!(
                "markdown:{}:{}:{}:{id}:{reason}",
                context.event.agent, context.event.instance, context.event.session_id
            ))
        })
        .unwrap_or_else(uuid7_text);
    let data = json!({"batchId": batch_id, "projectId": registration.project.id_text,
        "nativeSession": context.event.session_id, "nativeTurn": context.event.turn_id,
        "agent": context.event.agent, "instance": context.event.instance,
        "contextEpoch": context.env_value("AH_MARKDOWN_CONTEXT_EPOCH").unwrap_or("0"),
        "reason": reason, "durationMs": started.elapsed().as_secs_f64() * 1000.0,
        "documents": documents});
    spool_enqueue::enqueue_operation(
        &spool,
        "project.register",
        &registration.operation["data"],
        &context.env,
    )
    .and_then(|_| {
        spool_enqueue::enqueue_operation(&spool, "markdown-context.record", &data, &context.env)
    })
    .map_err(io::Error::other)?;
    spool_enqueue::schedule_flush(&context.env, &context.harness_root, &spool);
    Ok(())
}

pub fn record_failure(context: &HookContext, error: io::Error) {
    super::record(
        context,
        "markdown.telemetry.failed",
        "failed",
        Instant::now(),
        json!({"errorKind": format!("{:?}", error.kind()), "recovery": "bounded_spool_or_next_attempt"}),
    );
}
