//! One writer at a time for the observability spool inside this process. Consumers run on parallel
//! threads, and SQLite returns `SQLITE_BUSY` immediately (without the busy timeout) for a fresh
//! database's WAL switch or a read-to-write upgrade; serialising in-process access removes that race.
//! Other processes (Python services, `obs-rs flush`) are still coordinated by SQLite's busy timeout.

use std::sync::{Mutex, MutexGuard};

static SPOOL_ACCESS: Mutex<()> = Mutex::new(());

/// Held for the duration of one consumer's spool work; a panic in another consumer never blocks it.
pub fn lock_spool_access() -> MutexGuard<'static, ()> {
    SPOOL_ACCESS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
