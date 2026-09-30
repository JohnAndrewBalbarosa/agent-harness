//! `harness-services serve`: the single-instance background daemon.
//!
//! Mental model:
//! acquire the instance guard (exit 0 when another daemon has it) → log `daemon.start` → current-thread tokio
//! runtime with a small blocking pool → three supervised tasks (OTLP receiver, spool drain, console monitor),
//! each restarted on failure → runs until Ctrl-C/shutdown → log `daemon.stop`.

pub mod instance;
pub mod supervisor;

use std::path::{Path, PathBuf};

use anyhow::Context;
use harness_hook::shared_state::lifecycle_log::{details, emit};
use serde_json::json;

use crate::ensure::layout::services_log_path;
use crate::ensure::COMPONENT;
use crate::service_context::ServiceContext;
use crate::{console_monitor, otlp_receiver, spool_drain};
use supervisor::{supervise, Backoff, TaskKind};

/// SQLite drains and the console-monitor start are the only blocking work.
const MAX_BLOCKING_THREADS: usize = 4;
const RUNTIME_THREAD_NAME: &str = "harness-services";

pub fn run(context: &ServiceContext) -> anyhow::Result<()> {
    let log = services_log_path(&context.harness_root);
    let Some(_instance) = acquire_instance(context, &log)? else {
        emit(
            &log,
            COMPONENT,
            "daemon.start",
            "skipped",
            details([("reason", json!("already_running"))]),
        );
        return Ok(());
    };
    emit(
        &log,
        COMPONENT,
        "daemon.start",
        "started",
        details([("processId", json!(std::process::id()))]),
    );

    let runtime = build_runtime().inspect_err(|error| {
        emit(
            &log,
            COMPONENT,
            "daemon.start",
            "failed",
            details([("error", json!(format!("{error:#}")))]),
        );
    })?;
    let reason = runtime.block_on(run_until_shutdown(context.clone(), log.clone()));
    // A drain round can run for up to 120 s on the blocking pool; do not hold the mutex that long.
    runtime.shutdown_timeout(std::time::Duration::from_secs(3));

    emit(
        &log,
        COMPONENT,
        "daemon.stop",
        "stopped",
        details([("reason", json!(reason))]),
    );
    Ok(())
}

fn acquire_instance(
    context: &ServiceContext,
    log: &Path,
) -> anyhow::Result<Option<instance::DaemonInstance>> {
    instance::acquire(context)
        .inspect_err(|error| {
            emit(
                log,
                COMPONENT,
                "daemon.start",
                "failed",
                details([("error", json!(error.to_string()))]),
            );
        })
        .context("unable to take the daemon single-instance guard")
}

#[inline]
fn build_runtime() -> anyhow::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(MAX_BLOCKING_THREADS)
        .thread_name(RUNTIME_THREAD_NAME)
        .build()
        .context("unable to build the tokio runtime")
}

// Mental model: the supervised tasks never all finish; the daemon stops only on Ctrl-C (console close).
async fn run_until_shutdown(context: ServiceContext, log: PathBuf) -> &'static str {
    tokio::select! {
        () = run_supervised_tasks(context, log.clone()) => "tasks_ended",
        () = shutdown_signal(log) => "ctrl_c",
    }
}

/// Resolves on Ctrl-C; when the handler cannot be installed it logs that once and never resolves, so the
/// daemon keeps serving instead of stopping.
async fn shutdown_signal(log: PathBuf) {
    if let Err(error) = tokio::signal::ctrl_c().await {
        emit(
            &log,
            COMPONENT,
            "daemon.signal",
            "failed",
            details([("error", json!(error.to_string()))]),
        );
        std::future::pending::<()>().await;
    }
}

async fn run_supervised_tasks(context: ServiceContext, log: PathBuf) {
    let (drain_handle, drain_receiver) = spool_drain::channel();

    let receiver_context = context.clone();
    let receiver = supervise(
        "otlp-receiver",
        TaskKind::LongRunning,
        log.clone(),
        Backoff::DEFAULT,
        move || otlp_receiver::serve(receiver_context.clone(), drain_handle.clone()),
    );

    let drain_context = context.clone();
    let drain = supervise(
        "spool-drain",
        TaskKind::LongRunning,
        log.clone(),
        Backoff::DEFAULT,
        move || {
            let context = drain_context.clone();
            let receiver = drain_receiver.clone();
            async move {
                spool_drain::run(context, receiver).await;
                Ok(())
            }
        },
    );

    let monitor_context = context;
    let monitor = supervise(
        "console-monitor",
        TaskKind::OneShot,
        log,
        Backoff::DEFAULT,
        move || start_console_monitor(monitor_context.clone()),
    );

    tokio::join!(receiver, drain, monitor);
}

/// `console_monitor::spawn` starts its own thread and returns; it runs on the blocking pool so a slow start
/// never stalls the runtime. A panic inside is re-raised so the supervisor records it as a panic.
async fn start_console_monitor(context: ServiceContext) -> anyhow::Result<()> {
    match tokio::task::spawn_blocking(move || console_monitor::spawn(context)).await {
        Ok(started) => started,
        Err(join) if join.is_panic() => std::panic::resume_unwind(join.into_panic()),
        Err(join) => Err(anyhow::anyhow!(
            "console monitor start was cancelled: {join}"
        )),
    }
}
