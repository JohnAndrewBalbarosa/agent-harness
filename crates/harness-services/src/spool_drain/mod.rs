//! The daemon's spool drain: replaces the `flush_spool.py` process the Python receiver spawned per POST.
//!
//! Mental model:
//! enqueuers call `DrainHandle::request_flush()` (a stored permit, never blocks) → the drain loop wakes on a
//! request or every 60 s → one round on the blocking pool: take `flush-spool.lock` (skip when a Python/obs-rs
//! flusher holds it) → `flush_oldest_batch` until the queue is empty, 120 s pass, or 5 rounds stall.
//! Requests that arrive during a round leave one permit, so exactly one more round follows (coalescing).

pub mod drain_round;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use harness_hook::shared_state::lifecycle_log::{details, emit};
use obs_cli::settings::Settings;
use obs_cli::spool::flush::HubSink;
use serde_json::{json, Value};
use tokio::sync::Notify;

use crate::ensure::layout::services_log_path;
use crate::ensure::COMPONENT;
use crate::service_context::ServiceContext;
use drain_round::{drain_spool, DrainLimits, DrainOutcome, DrainStatus, SpoolPaths};

/// Safety net: drain even when nobody asked (e.g. the hub came back after an outage).
pub const SAFETY_INTERVAL: Duration = Duration::from_secs(60);
const DRAIN_OPERATION: &str = "spool.drain";

/// Cheap, cloneable request side; held by the OTLP receiver.
#[derive(Clone)]
pub struct DrainHandle {
    signal: Arc<Notify>,
}

impl DrainHandle {
    /// Non-blocking and coalescing: any number of calls before the drain wakes count as one request.
    #[inline]
    pub fn request_flush(&self) {
        self.signal.notify_one();
    }
}

/// Wait side, consumed by `run`; cloneable so the supervisor can restart the drain loop.
#[derive(Clone)]
pub struct DrainReceiver {
    signal: Arc<Notify>,
}

/// Why a round starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainTrigger {
    Requested,
    SafetyNet,
}

impl DrainReceiver {
    /// Waits for a request, or `safety_interval` without one.
    pub async fn next_trigger(&self, safety_interval: Duration) -> DrainTrigger {
        match tokio::time::timeout(safety_interval, self.signal.notified()).await {
            Ok(()) => DrainTrigger::Requested,
            Err(_elapsed) => DrainTrigger::SafetyNet,
        }
    }
}

pub fn channel() -> (DrainHandle, DrainReceiver) {
    let signal = Arc::new(Notify::new());
    (
        DrainHandle {
            signal: Arc::clone(&signal),
        },
        DrainReceiver { signal },
    )
}

/// Everything one round needs, owned so it can move onto the blocking pool.
#[derive(Debug, Clone)]
pub struct DrainPlan {
    pub paths: SpoolPaths,
    pub settings: Settings,
    pub limits: DrainLimits,
    pub log: PathBuf,
}

impl DrainPlan {
    /// obs.py's settings from the process environment, with the merged `.env` `OBS_ENDPOINT` on top (the
    /// Python bootstrap exported it to its children the same way).
    pub fn for_context(context: &ServiceContext) -> Self {
        let mut settings = Settings::from_env();
        if let Some(endpoint) = context.env_value("OBS_ENDPOINT") {
            settings.endpoint = endpoint.to_owned();
        }
        Self {
            paths: SpoolPaths::for_context(context),
            settings,
            limits: DrainLimits::FLUSH_SPOOL_PY,
            log: services_log_path(&context.harness_root),
        }
    }
}

// Mental model: drain once at start (leftovers from before the daemon) → then one round per trigger.
/// Never returns; each round's failure is logged and the loop keeps waiting for the next trigger.
pub async fn run(context: ServiceContext, receiver: DrainReceiver) {
    let plan = DrainPlan::for_context(&context);
    let mut trigger = DrainTrigger::SafetyNet;
    loop {
        run_round(&plan, trigger).await;
        trigger = receiver.next_trigger(SAFETY_INTERVAL).await;
    }
}

async fn run_round(plan: &DrainPlan, trigger: DrainTrigger) {
    let round_plan = plan.clone();
    let joined = tokio::task::spawn_blocking(move || {
        let sink = HubSink {
            settings: &round_plan.settings,
        };
        drain_spool(
            &round_plan.paths,
            &round_plan.settings,
            &sink,
            &round_plan.limits,
        )
    })
    .await;
    match joined {
        Ok(Ok(outcome)) => log_outcome(&plan.log, trigger, &outcome),
        Ok(Err(error)) => log_failure(&plan.log, trigger, &format!("{error:#}")),
        Err(join) if join.is_panic() => std::panic::resume_unwind(join.into_panic()),
        Err(join) => log_failure(&plan.log, trigger, &join.to_string()),
    }
}

/// Idle rounds (empty spool, lock held by another flusher) are not logged; everything else is.
fn log_outcome(log: &std::path::Path, trigger: DrainTrigger, outcome: &DrainOutcome) {
    let is_noteworthy = outcome.delivered > 0
        || matches!(outcome.status, DrainStatus::Stalled | DrainStatus::TimedOut);
    if !is_noteworthy {
        return;
    }
    emit(
        log,
        COMPONENT,
        DRAIN_OPERATION,
        "succeeded",
        details([
            ("trigger", trigger_name(trigger)),
            ("status", json!(outcome.status.as_str())),
            ("delivered", json!(outcome.delivered)),
            ("remaining", json!(outcome.remaining)),
        ]),
    );
}

fn log_failure(log: &std::path::Path, trigger: DrainTrigger, error: &str) {
    emit(
        log,
        COMPONENT,
        DRAIN_OPERATION,
        "failed",
        details([
            ("trigger", trigger_name(trigger)),
            (
                "error",
                json!(obs_cli::python_compat::truncate_chars(error, 300)),
            ),
        ]),
    );
}

#[inline]
fn trigger_name(trigger: DrainTrigger) -> Value {
    json!(match trigger {
        DrainTrigger::Requested => "requested",
        DrainTrigger::SafetyNet => "interval",
    })
}
