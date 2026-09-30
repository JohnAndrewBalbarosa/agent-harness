//! Non-blocking exclusive file locks shared with Python processes that lock byte 0 with `msvcrt.locking`
//! (`var/services.lock`, `flush-spool.lock`, the non-Windows daemon lock).
//!
//! `File::try_lock` takes an exclusive `LockFileEx` over the whole file on Windows (`flock` elsewhere), which
//! overlaps byte 0, so Rust and Python holders exclude each other.

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::Path;

/// Holds the lock until dropped. Closing the handle releases the lock even when the explicit unlock fails.
pub struct ExclusiveFileGuard {
    file: File,
}

impl Drop for ExclusiveFileGuard {
    fn drop(&mut self) {
        if let Err(error) = self.file.unlock() {
            // Nowhere left to report from a destructor; the handle closes right after, which releases the lock.
            eprintln!("harness-services: unlock failed ({error}); closing the handle releases it");
        }
    }
}

// Mental model: create the lock file (and its directory) → try once → `None` when another process holds it.
/// Takes the lock without waiting; `Ok(None)` means another holder has it.
pub fn try_exclusive(path: &Path) -> io::Result<Option<ExclusiveFileGuard>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(ExclusiveFileGuard { file })),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

/// Probe without creating the file: `true` only when the file exists and someone else holds its lock.
pub fn is_held_elsewhere(path: &Path) -> bool {
    let Ok(file) = OpenOptions::new().read(true).write(true).open(path) else {
        return false;
    };
    match file.try_lock() {
        Ok(()) => {
            if let Err(error) = file.unlock() {
                eprintln!(
                    "harness-services: probe unlock failed ({error}); the handle closes next"
                );
            }
            false
        }
        Err(TryLockError::WouldBlock) => true,
        Err(TryLockError::Error(_)) => false,
    }
}
