//! `harness-hook <agent> [<native-event>]` dispatch (`core/router/harness_hook.py`).
//!
//! Reads the native payload, normalizes it through the agent's adapter, fans the common event out to the
//! subscribed in-process consumers (parallel threads, per-consumer timeout) and returns the adapter's native
//! response. It never fails its agent: every error path returns the adapter's safe default.
//!
//! Harness root (`HARNESS_HOME`, where `.env`, `adapters/`, `var/` live): `AGENT_HARNESS_HOME` when set,
//! else the parent of the directory holding the running exe (installed as `<root>/bin/harness-hook.exe`).

mod fan_out;
mod harness_root;
mod lifecycle_records;
mod payload;
mod version_check;

use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use serde_json::{Map, Value};

pub use harness_root::{resolve_harness_root, HARNESS_HOME_ENV};
pub use payload::parse_payload;
pub use version_check::tested_versions;

use crate::adapters::{load_adapter, native_event_name, Adapter};
use crate::consumer::Consumer;
use crate::consumers::all_consumers;
use crate::hook_context::HookContext;
use crate::runtime_env::{consumer_env, process_environment, runtime_env, EnvMap, ENV_FILE_NAME};

pub const LIFECYCLE_LOG_NAME: &str = "hook-router.lifecycle.jsonl";

/// Everything the router needs besides the hook itself; tests inject fake consumers and a temp log dir.
pub struct RouterSetup {
    pub harness_root: PathBuf,
    /// The runtime env (`runtime_env(.env, process env)`), before the per-event `AH_*` identity is added.
    pub env: EnvMap,
    /// Candidate consumers; the router keeps those that subscribe to the event and are enabled.
    pub registry: Vec<Box<dyn Consumer>>,
}

// Mental model: silence panic output (Python ran with `2>nul`) → build the real setup (root, .env merge,
// full registry) → dispatch; any panic that escapes still yields the adapter's safe default.
/// Process entry point used by `main.rs`.
pub fn run_hook(agent: &str, native_event: Option<&str>, stdin: &[u8]) -> String {
    panic::set_hook(Box::new(|_| {}));

    let outcome = panic::catch_unwind(|| {
        let harness_root = resolve_harness_root();
        let env = runtime_env(&harness_root.join(ENV_FILE_NAME), &process_environment());
        let setup = RouterSetup {
            harness_root,
            env,
            registry: all_consumers(),
        };
        run_hook_with(setup, agent, native_event, stdin)
    });

    outcome.unwrap_or_else(|_| last_resort_default(agent, native_event))
}

// Mental model: unknown agent → "" | bad payload → safe default | normalize → version check (session.start)
// → consumers → hook.dispatch record → adapter.respond. Only the dispatch phase can panic into a
// `hook.dispatch failed` record.
/// `run(agent, native_event, stdin, env, registry)` without the process-level concerns.
pub fn run_hook_with(
    setup: RouterSetup,
    agent: &str,
    native_event: Option<&str>,
    stdin: &[u8],
) -> String {
    let started = Instant::now();
    let log = lifecycle_log_path(&setup.env, &setup.harness_root);

    let Some(adapter) = load_adapter(agent) else {
        lifecycle_records::agent_unknown(&log, agent);
        return String::new();
    };
    let Some(payload) = parse_payload(stdin) else {
        lifecycle_records::payload_invalid(&log, agent, native_event);
        return adapter.safe_default(native_event);
    };

    let native = resolved_native_event(native_event, &payload);
    let request = DispatchRequest {
        agent,
        adapter,
        native: native.as_deref(),
        payload,
        stdin,
        started,
        log: &log,
    };
    let dispatched = panic::catch_unwind(AssertUnwindSafe(|| dispatch(setup, &request)));

    dispatched.unwrap_or_else(|_| {
        lifecycle_records::dispatch_failed(&log, agent, native.as_deref(), "Panic");
        adapter.safe_default(native.as_deref())
    })
}

/// `Path(env.get("AH_LOG_DIR") or ROOT / "var" / "logs") / "hook-router.lifecycle.jsonl"`.
pub fn lifecycle_log_path(env: &EnvMap, harness_root: &Path) -> PathBuf {
    let directory = env
        .get("AH_LOG_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| harness_root.join("var").join("logs"));
    directory.join(LIFECYCLE_LOG_NAME)
}

struct DispatchRequest<'a> {
    agent: &'a str,
    adapter: &'static dyn Adapter,
    native: Option<&'a str>,
    payload: Map<String, Value>,
    stdin: &'a [u8],
    started: Instant,
    log: &'a Path,
}

// Mental model: normalize (None → safe default) → version check on session.start → build the shared
// HookContext → pick subscribed + enabled consumers → run them → record → respond.
fn dispatch(setup: RouterSetup, request: &DispatchRequest<'_>) -> String {
    let adapter = request.adapter;
    let Some(event) = adapter.normalize(request.native, &request.payload, &setup.env) else {
        return adapter.safe_default(request.native);
    };

    if event.event == "session.start" {
        version_check::warn_when_untested(
            &setup.harness_root,
            request.agent,
            &request.payload,
            request.log,
        );
    }

    let context = Arc::new(HookContext {
        env: consumer_env(&setup.env, &event, &setup.harness_root),
        event,
        native_payload: Value::Object(request.payload.clone()),
        native_bytes: request.stdin.to_vec(),
        harness_root: setup.harness_root,
    });
    let subscribed = subscribed_consumers(setup.registry, &context);
    let results = fan_out::run_consumers(subscribed, &context, request.log);

    lifecycle_records::dispatched(
        request.log,
        request.agent,
        &context.event,
        request.native,
        &results,
        request.started,
    );
    adapter.respond(&context.event, &results)
}

/// `[consumer for consumer in registry(event, env) if event.event in consumer.events]` plus each
/// consumer's own gating (herdr-bridge inside Herdr only, orb only when installed, ...).
fn subscribed_consumers(
    registry: Vec<Box<dyn Consumer>>,
    context: &HookContext,
) -> Vec<Box<dyn Consumer>> {
    registry
        .into_iter()
        .filter(|consumer| {
            consumer.subscribes_to(&context.event.event) && consumer.is_enabled(context)
        })
        .collect()
}

/// `native_event or str(payload.get("hook_event_name") or "") or None`.
fn resolved_native_event(
    native_event: Option<&str>,
    payload: &Map<String, Value>,
) -> Option<String> {
    Some(native_event_name(native_event, payload)).filter(|native| !native.is_empty())
}

/// Used only when even setup panicked: the adapter's safe default without touching any file.
fn last_resort_default(agent: &str, native_event: Option<&str>) -> String {
    load_adapter(agent)
        .map(|adapter| adapter.safe_default(native_event))
        .unwrap_or_default()
}
