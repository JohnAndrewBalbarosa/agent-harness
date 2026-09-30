//! `harness-services ensure`: the session-start bootstrap (`core/services/bootstrap.py`).
//!
//! Mental model:
//! `var/services.lock` free? → no: log `bootstrap skipped reason=already_running` → yes: run four isolated
//! steps, each logged to `var/logs/services.lifecycle.jsonl` as `{operation, succeeded, result}` or
//! `{operation, failed, error=<type>}`:
//! `receiver.ensure` (daemon) → `orb.ensure` → `hub.ensure` → `console-monitor.ensure` (daemon, reused answer).

pub mod file_guard;
pub mod layout;
pub mod probes;
pub mod steps;

use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use anyhow::Context;
use harness_hook::shared_state::lifecycle_log::{details, emit};
use serde_json::json;

use crate::service_context::ServiceContext;
use file_guard::{try_exclusive, ExclusiveFileGuard};
use layout::EnsureLayout;
use probes::{ServiceProbes, SystemProbes};
use steps::{ensure_hub, ensure_orb, DaemonStep};

pub const COMPONENT: &str = "services";
const BOOTSTRAP_OPERATION: &str = "bootstrap";
const BOOTSTRAP_LOCK_FILE: &str = "services.lock";
/// bootstrap.py: `result=result or "ensured"`.
const DEFAULT_RESULT: &str = "ensured";

/// `python -m core.services.bootstrap` `main()`.
pub fn run(context: &ServiceContext) -> anyhow::Result<()> {
    let layout = EnsureLayout::from_env(&context.harness_root, &context.env);
    let Some(_bootstrap_guard) = acquire_bootstrap_lock(&layout)? else {
        emit(
            &layout.log(),
            COMPONENT,
            BOOTSTRAP_OPERATION,
            "skipped",
            details([("reason", json!("already_running"))]),
        );
        return Ok(());
    };
    let probes = SystemProbes::new(context);
    run_steps(&layout, &probes);
    Ok(())
}

/// bootstrap.py `single_instance(root)`: `Ok(None)` while another bootstrap holds `var/services.lock`.
pub fn single_instance(root: &Path) -> io::Result<Option<ExclusiveFileGuard>> {
    try_exclusive(&bootstrap_lock_path(root))
}

#[inline]
pub fn bootstrap_lock_path(root: &Path) -> PathBuf {
    root.join("var").join(BOOTSTRAP_LOCK_FILE)
}

/// Opening the lock file can fail (permissions, disk); Python would crash with a traceback, so this logs
/// the failure and returns it for `main` to report.
fn acquire_bootstrap_lock(layout: &EnsureLayout) -> anyhow::Result<Option<ExclusiveFileGuard>> {
    single_instance(&layout.root)
        .inspect_err(|error| {
            emit(
                &layout.log(),
                COMPONENT,
                BOOTSTRAP_OPERATION,
                "failed",
                details([("error", json!(python_error_name(error)))]),
            );
        })
        .with_context(|| {
            format!(
                "unable to lock {}",
                bootstrap_lock_path(&layout.root).display()
            )
        })
}

// Mental model: every step runs even when an earlier one failed or panicked (bootstrap.py `run`).
/// bootstrap.py `run(layout, launch, healthy, ensure_receiver)` with the probes injected.
pub fn run_steps(layout: &EnsureLayout, probes: &dyn ServiceProbes) {
    let daemon = DaemonStep::new(layout, probes);
    let steps: [(&str, &dyn Fn() -> io::Result<&'static str>); 4] = [
        ("receiver.ensure", &|| daemon.ensure()),
        ("orb.ensure", &|| ensure_orb(layout, probes)),
        ("hub.ensure", &|| ensure_hub(layout, probes)),
        ("console-monitor.ensure", &|| daemon.ensure()),
    ];
    for (operation, step) in steps {
        let outcome = catch_unwind(AssertUnwindSafe(step));
        record_step(&layout.log(), operation, outcome);
    }
}

fn record_step(
    log: &Path,
    operation: &str,
    outcome: std::thread::Result<io::Result<&'static str>>,
) {
    match outcome {
        Ok(Ok(result)) => {
            let result = if result.is_empty() {
                DEFAULT_RESULT
            } else {
                result
            };
            emit(
                log,
                COMPONENT,
                operation,
                "succeeded",
                details([("result", json!(result))]),
            );
        }
        Ok(Err(error)) => emit(
            log,
            COMPONENT,
            operation,
            "failed",
            details([("error", json!(python_error_name(&error)))]),
        ),
        Err(_panic) => emit(
            log,
            COMPONENT,
            operation,
            "failed",
            details([("error", json!("panic"))]),
        ),
    }
}

/// Python `type(error).__name__` for the `OSError` family `Popen`/`open` raise.
#[inline]
pub fn python_error_name(error: &io::Error) -> &'static str {
    match error.kind() {
        io::ErrorKind::NotFound => "FileNotFoundError",
        io::ErrorKind::PermissionDenied => "PermissionError",
        io::ErrorKind::TimedOut => "TimeoutError",
        _ => "OSError",
    }
}
