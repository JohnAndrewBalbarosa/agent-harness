//! When the otherwise endless subscriber leaves: a stop signal (Ctrl-C everywhere; Ctrl-Break / console close
//! on Windows; SIGTERM on Unix) or a lock file that no longer exists (deleted by uninstall/cleanup: a new
//! subscriber could then lock a fresh file, so this one must go to keep a single instance).

use std::path::Path;
use std::time::Duration;

const LOCK_CHECK_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    Signal,
    LockLost,
}

// Mental model: first of {signal, lock gone} wins.
pub async fn wait_for_stop(lock_path: &Path) -> StopReason {
    tokio::select! {
        () = wait_for_signal() => StopReason::Signal,
        () = wait_until_lock_lost(lock_path) => StopReason::LockLost,
    }
}

/// Polls the lock file; only a definite "not found" counts as lost.
pub async fn wait_until_lock_lost(lock_path: &Path) {
    let mut ticks = tokio::time::interval(LOCK_CHECK_INTERVAL);
    loop {
        ticks.tick().await;
        if is_lock_file_missing(lock_path) {
            return;
        }
    }
}

#[inline]
pub fn is_lock_file_missing(lock_path: &Path) -> bool {
    matches!(
        std::fs::metadata(lock_path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    )
}

async fn wait_for_signal() {
    tokio::select! {
        () = wait_for_ctrl_c() => {},
        () = wait_for_platform_stop() => {},
    }
}

/// A handler that cannot be installed never fires.
#[inline]
async fn wait_for_ctrl_c() {
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
}

#[cfg(windows)]
async fn wait_for_platform_stop() {
    use tokio::signal::windows::{ctrl_break, ctrl_close};
    let (Ok(mut on_break), Ok(mut on_close)) = (ctrl_break(), ctrl_close()) else {
        return std::future::pending().await;
    };
    tokio::select! {
        _ = on_break.recv() => {},
        _ = on_close.recv() => {},
    }
}

#[cfg(unix)]
async fn wait_for_platform_stop() {
    use tokio::signal::unix::{signal, SignalKind};
    let Ok(mut on_terminate) = signal(SignalKind::terminate()) else {
        return std::future::pending().await;
    };
    on_terminate.recv().await;
}
