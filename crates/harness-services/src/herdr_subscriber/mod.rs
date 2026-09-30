//! `harness-services herdr-subscribe`: the Herdr event subscriber (`tools/herdr-bridge/subscriber.py` +
//! `socket-stream.mjs`), one process per Herdr socket, launched detached by the hook's herdr-bridge consumer.
//!
//! Mental model:
//! `HERDR_SOCKET_PATH` (empty → exit 2) → single-instance lock `subscriber-<sha256[:16]>.lock` (held elsewhere →
//! exit 0) → `subscriber.started` → forever: connect → `events.subscribe` → NDJSON events → log `herdr.event` +
//! enqueue `herdr-event.record` for bound panes → on failure `subscriber.reconnect` + backoff 1 s → 30 s.
//! Leaves only on a stop signal or when the lock file disappears; then removes the `.pid` file and logs
//! `subscriber.stopped`.
//!
//! Deliberate deviations from Python (spec "Deliberate deviations"): no Node helper (direct named pipe /
//! Unix socket), buffered reads, and `herdr-event.record` goes into the real spool (Python's `obs.py` import
//! always failed, so every bound event only produced `herdr.event.persist` failures).

pub mod event_payload;
pub mod event_recorder;
pub mod framing;
pub mod instance_lock;
pub mod pane_binding;
pub mod reconnect;
pub mod sanitize;
pub mod stop_signal;
pub mod subscription;
pub mod transport;

use anyhow::Context;
use harness_hook::consumers::herdr_bridge::bridge_log::BridgeLog;
use serde_json::json;

use crate::service_context::ServiceContext;
use event_recorder::EventRecorder;
use instance_lock::{InstanceAcquisition, SubscriberInstance};
use reconnect::SubscriberSession;
use stop_signal::StopReason;

const SOCKET_ENV: &str = "HERDR_SOCKET_PATH";

/// Entry point of `harness-services herdr-subscribe`. `Err` (process exit 2) only when `HERDR_SOCKET_PATH` is
/// empty or the lock/runtime cannot be set up; a subscriber that already runs means `Ok` (exit 0).
pub fn run(context: &ServiceContext) -> anyhow::Result<()> {
    // Mental model: socket → instance lock → started → subscribe until stopped → release → stopped.
    let socket_path = required_socket_path(context)?;
    let instance = match SubscriberInstance::acquire(&context.harness_root, &socket_path)? {
        InstanceAcquisition::Acquired(instance) => instance,
        InstanceAcquisition::AlreadyRunning => return Ok(()),
    };
    let log = BridgeLog::under_root(&context.harness_root);
    let process_id = std::process::id();
    log.event(
        "subscriber.started",
        "succeeded",
        vec![
            ("socket_hash", json!(instance.identity())),
            ("pid", json!(process_id)),
        ],
    );

    let outcome = subscribe_until_stopped(context, &socket_path, &instance, &log);
    if let Ok(StopReason::LockLost) = outcome {
        log.event(
            "subscriber.lock.lost",
            "failed",
            vec![("socket_hash", json!(instance.identity()))],
        );
    }
    drop(instance);
    log.event(
        "subscriber.stopped",
        "succeeded",
        vec![("pid", json!(process_id))],
    );
    outcome.map(|_| ())
}

/// `os.environ.get("HERDR_SOCKET_PATH", "").strip()`; empty → error (exit 2, like `main()` returning 2).
#[inline]
fn required_socket_path(context: &ServiceContext) -> anyhow::Result<String> {
    let socket_path = context.env_value(SOCKET_ENV).unwrap_or_default().trim();
    if socket_path.is_empty() {
        anyhow::bail!("{SOCKET_ENV} is empty");
    }
    Ok(socket_path.to_owned())
}

// Mental model: one current-thread runtime; the reconnect loop never ends by itself, so the first of
// {stop signal, lock lost} decides when the process leaves.
fn subscribe_until_stopped(
    context: &ServiceContext,
    socket_path: &str,
    instance: &SubscriberInstance,
    log: &BridgeLog,
) -> anyhow::Result<StopReason> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("herdr subscriber runtime")?;
    let recorder = EventRecorder::new(context, log);
    let mut session = SubscriberSession::new(socket_path, instance.identity(), log, recorder);
    let reason = runtime.block_on(async {
        tokio::select! {
            reason = stop_signal::wait_for_stop(instance.lock_path()) => reason,
            never = session.reconnect_forever() => match never {},
        }
    });
    Ok(reason)
}
