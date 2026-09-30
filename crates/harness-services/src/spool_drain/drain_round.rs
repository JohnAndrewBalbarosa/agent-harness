//! One drain round, exactly `tools/observability-client/flush_spool.py` `main()`:
//! non-blocking `flush-spool.lock` (byte 0) → while under 120 s: stop when the queue is empty, else
//! `obs.flush()` (obs-cli `flush_oldest_batch`: oldest 50, bisect on 400/413 and unreachable hub, dead-letter
//! rules) → a flush that did not shrink the queue is a stalled round; 5 stalled rounds end the drain, with a
//! 250 ms pause between them.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Context;
use obs_cli::failure::CliFailure;
use obs_cli::settings::Settings;
use obs_cli::spool::flush::{flush_oldest_batch, OperationSink};
use obs_cli::spool::open::open_spool_at;
use rusqlite::Connection;

use crate::ensure::file_guard::try_exclusive;
use crate::service_context::ServiceContext;

const FLUSH_LOCK_FILE: &str = "flush-spool.lock";

/// The spool and the lock every flusher (flush_spool.py, obs-rs, this daemon) serialises on.
#[derive(Debug, Clone)]
pub struct SpoolPaths {
    pub spool: PathBuf,
    /// flush_spool.py: `<tools/observability-client>/var/flush-spool.lock`.
    pub flush_lock: PathBuf,
}

impl SpoolPaths {
    pub fn for_context(context: &ServiceContext) -> Self {
        Self {
            spool: context.spool_path(),
            flush_lock: context
                .harness_root
                .join("tools")
                .join("observability-client")
                .join("var")
                .join(FLUSH_LOCK_FILE),
        }
    }
}

/// flush_spool.py's bounds.
#[derive(Debug, Clone, Copy)]
pub struct DrainLimits {
    /// `MAX_SECONDS`.
    pub max_round: Duration,
    /// `stalled_attempts >= 5`.
    pub max_stalled_flushes: u32,
    /// `time.sleep(0.25)` after a stalled flush.
    pub stall_pause: Duration,
}

impl DrainLimits {
    pub const FLUSH_SPOOL_PY: DrainLimits = DrainLimits {
        max_round: Duration::from_secs(120),
        max_stalled_flushes: 5,
        stall_pause: Duration::from_millis(250),
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainStatus {
    /// Another flusher holds `flush-spool.lock` (flush_spool.py `return 0`).
    LockHeld,
    /// The queue is empty.
    Drained,
    /// Five flushes in a row did not shrink the queue (hub down or rejecting).
    Stalled,
    /// `MAX_SECONDS` elapsed with work left.
    TimedOut,
}

impl DrainStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LockHeld => "lock_held",
            Self::Drained => "drained",
            Self::Stalled => "stalled",
            Self::TimedOut => "timed_out",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrainOutcome {
    pub status: DrainStatus,
    /// Operations the hub accepted in this round.
    pub delivered: usize,
    /// Queue rows left (0 when the lock was held: not inspected).
    pub remaining: i64,
}

// Mental model: lock (skip if held) → open the spool → flush until settled.
/// One flush_spool.py run against `paths` with `sink` as the hub.
pub fn drain_spool(
    paths: &SpoolPaths,
    settings: &Settings,
    sink: &dyn OperationSink,
    limits: &DrainLimits,
) -> anyhow::Result<DrainOutcome> {
    let lock = try_exclusive(&paths.flush_lock)
        .with_context(|| format!("unable to lock {}", paths.flush_lock.display()))?;
    let Some(_flush_guard) = lock else {
        return Ok(DrainOutcome {
            status: DrainStatus::LockHeld,
            delivered: 0,
            remaining: 0,
        });
    };
    let conn = open_spool_at(&paths.spool).map_err(cli_failure_error)?;
    flush_until_settled(&conn, settings, sink, limits)
}

/// flush_spool.py's `while time.monotonic() < deadline` loop.
fn flush_until_settled(
    conn: &Connection,
    settings: &Settings,
    sink: &dyn OperationSink,
    limits: &DrainLimits,
) -> anyhow::Result<DrainOutcome> {
    let deadline = Instant::now() + limits.max_round;
    let mut delivered = 0;
    let mut stalled_flushes = 0;
    while Instant::now() < deadline {
        let before = pending_count(conn)?;
        if before == 0 {
            return Ok(outcome(DrainStatus::Drained, delivered, 0));
        }
        delivered += flush_oldest_batch(conn, settings, sink).map_err(cli_failure_error)?;
        let after = pending_count(conn)?;
        if after < before {
            stalled_flushes = 0;
            continue;
        }
        stalled_flushes += 1;
        if stalled_flushes >= limits.max_stalled_flushes {
            return Ok(outcome(DrainStatus::Stalled, delivered, after));
        }
        std::thread::sleep(limits.stall_pause);
    }
    let remaining = pending_count(conn)?;
    let status = if remaining == 0 {
        DrainStatus::Drained
    } else {
        DrainStatus::TimedOut
    };
    Ok(outcome(status, delivered, remaining))
}

#[inline]
fn outcome(status: DrainStatus, delivered: usize, remaining: i64) -> DrainOutcome {
    DrainOutcome {
        status,
        delivered,
        remaining,
    }
}

/// flush_spool.py `pending_count()`.
#[inline]
fn pending_count(conn: &Connection) -> anyhow::Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM queue", [], |row| row.get(0))
        .context("unable to count queued operations")
}

/// `CliFailure` has no `Display`; keep its message for the lifecycle log.
fn cli_failure_error(failure: CliFailure) -> anyhow::Error {
    match failure {
        CliFailure::Internal(error) => error,
        CliFailure::Refusal(message) => anyhow::anyhow!(message),
        CliFailure::Usage { message, .. } => anyhow::anyhow!(message),
        CliFailure::HelpRequested { usage } => anyhow::anyhow!("unexpected help request: {usage}"),
    }
}
