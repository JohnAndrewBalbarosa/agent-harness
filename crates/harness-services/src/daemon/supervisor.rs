//! Task isolation for the daemon: every service runs as its own tokio task; an error or panic is logged to
//! `services.lifecycle.jsonl` (`daemon.task.failed`) and the task restarts with backoff (1 s doubling to
//! 60 s). One failing task never stops the others or the daemon.

use std::any::Any;
use std::future::Future;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use harness_hook::shared_state::lifecycle_log::{details, emit};
use obs_cli::python_compat::truncate_chars;
use serde_json::json;

use crate::ensure::COMPONENT;

const MAX_ERROR_CHARS: usize = 300;

/// Restart delays: `initial`, doubling, capped at `max`. A task that stayed up for `max` restarts from `initial`.
#[derive(Debug, Clone, Copy)]
pub struct Backoff {
    pub initial: Duration,
    pub max: Duration,
}

impl Backoff {
    pub const DEFAULT: Backoff = Backoff {
        initial: Duration::from_secs(1),
        max: Duration::from_secs(60),
    };

    #[inline]
    pub fn after(&self, current: Duration) -> Duration {
        current.saturating_mul(2).min(self.max)
    }
}

/// What a finished `Ok(())` means for the task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    /// Should run forever; returning (e.g. the receiver's port is taken) restarts it after the backoff.
    LongRunning,
    /// Done once it succeeds (e.g. the console monitor starting its own thread).
    OneShot,
}

/// How one attempt ended.
#[derive(Debug)]
enum TaskEnding {
    Completed,
    Failed { kind: &'static str, error: String },
}

// Mental model: start attempt as its own task → classify its ending → one-shot success ends supervision;
// anything else is logged (failures always, quiet exits once) → wait the backoff → start again.
/// Supervises one named task until it is done (one-shot) or forever (long-running).
pub async fn supervise<F, Fut>(
    task: &'static str,
    kind: TaskKind,
    log: PathBuf,
    backoff: Backoff,
    mut start: F,
) where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
{
    let mut delay = backoff.initial;
    let mut has_logged_quiet_exit = false;
    loop {
        let started = Instant::now();
        let ending = run_attempt(start()).await;
        if started.elapsed() >= backoff.max {
            delay = backoff.initial;
        }
        match ending {
            TaskEnding::Completed if kind == TaskKind::OneShot => return,
            TaskEnding::Completed => {
                if !has_logged_quiet_exit {
                    log_quiet_exit(&log, task, delay);
                    has_logged_quiet_exit = true;
                }
            }
            TaskEnding::Failed {
                kind: failure_kind,
                error,
            } => log_failure(&log, task, failure_kind, &error, delay),
        }
        tokio::time::sleep(delay).await;
        delay = backoff.after(delay);
    }
}

/// Runs the attempt on its own tokio task so a panic is caught as a `JoinError` instead of unwinding here.
async fn run_attempt<Fut>(attempt: Fut) -> TaskEnding
where
    Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
{
    match tokio::spawn(attempt).await {
        Ok(Ok(())) => TaskEnding::Completed,
        Ok(Err(error)) => TaskEnding::Failed {
            kind: "error",
            error: bounded(&format!("{error:#}")),
        },
        Err(join) if join.is_panic() => TaskEnding::Failed {
            kind: "panic",
            error: bounded(&panic_text(join.into_panic())),
        },
        Err(join) => TaskEnding::Failed {
            kind: "cancelled",
            error: bounded(&join.to_string()),
        },
    }
}

fn log_failure(log: &std::path::Path, task: &str, kind: &str, error: &str, retry_in: Duration) {
    emit(
        log,
        COMPONENT,
        "daemon.task.failed",
        "failed",
        details([
            ("task", json!(task)),
            ("kind", json!(kind)),
            ("error", json!(error)),
            ("retryInMs", json!(millis(retry_in))),
        ]),
    );
}

fn log_quiet_exit(log: &std::path::Path, task: &str, retry_in: Duration) {
    emit(
        log,
        COMPONENT,
        "daemon.task.exited",
        "restarting",
        details([
            ("task", json!(task)),
            ("retryInMs", json!(millis(retry_in))),
        ]),
    );
}

/// The panic payload as text (`panic!("...")` carries `&str` or `String`).
#[inline]
pub fn panic_text(payload: Box<dyn Any + Send>) -> String {
    match payload.downcast::<String>() {
        Ok(text) => *text,
        Err(payload) => payload
            .downcast_ref::<&str>()
            .map(|text| (*text).to_owned())
            .unwrap_or_else(|| "non-text panic payload".to_owned()),
    }
}

#[inline]
fn bounded(text: &str) -> String {
    truncate_chars(text, MAX_ERROR_CHARS)
}

#[inline]
fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
