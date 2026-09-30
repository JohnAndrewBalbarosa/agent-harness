//! Cross-process exclusive lock on a sidecar `<path>.lock` file (`core/locking.py`).
//! Python locks byte 0 (length 1) with `msvcrt.locking`; `File::lock` takes an exclusive `LockFileEx`
//! over the whole file on Windows (and `flock` elsewhere), which overlaps byte 0, so both sides exclude
//! each other.

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Python's `msvcrt.locking(LK_LOCK)` gives up after about 10 s; a hook must never wait longer.
pub const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(10);
const RETRY_INTERVAL: Duration = Duration::from_millis(10);

/// Holds the lock until dropped.
pub struct SidecarLock {
    file: File,
}

impl Drop for SidecarLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// Waits up to `DEFAULT_LOCK_TIMEOUT` for the sidecar lock of `path`.
pub fn lock_sidecar(path: &Path) -> io::Result<SidecarLock> {
    lock_sidecar_within(path, DEFAULT_LOCK_TIMEOUT)
}

// Mental model: open the sidecar → retry a non-blocking lock every 10 ms → give up at the deadline
// (a wedged holder in another process must not hang the hook).
pub fn lock_sidecar_within(path: &Path, timeout: Duration) -> io::Result<SidecarLock> {
    let file = open_sidecar(&sidecar_path(path))?;
    let deadline = Instant::now() + timeout;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(SidecarLock { file }),
            Err(TryLockError::Error(error)) => return Err(error),
            Err(TryLockError::WouldBlock) if Instant::now() >= deadline => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "sidecar lock timed out",
                ));
            }
            Err(TryLockError::WouldBlock) => std::thread::sleep(RETRY_INTERVAL),
        }
    }
}

/// Runs `work` while holding the sidecar lock of `path` (bounded wait).
pub fn with_sidecar_lock<T>(path: &Path, work: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    let _guard = lock_sidecar(path)?;
    work()
}

/// Same as `with_sidecar_lock` with a caller-chosen bound.
pub fn with_sidecar_lock_within<T>(
    path: &Path,
    timeout: Duration,
    work: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    let _guard = lock_sidecar_within(path, timeout)?;
    work()
}

/// `<path>.lock` next to `path`.
pub fn sidecar_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(".lock");
    path.with_file_name(name)
}

fn open_sidecar(lock_path: &Path) -> io::Result<File> {
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_is_named_after_the_guarded_file() {
        assert_eq!(
            sidecar_path(Path::new("/tmp/x/state.json")),
            PathBuf::from("/tmp/x/state.json.lock")
        );
    }

    #[test]
    fn a_held_lock_times_out_instead_of_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("held.json");
        let _holder = lock_sidecar(&target).unwrap();
        let started = Instant::now();
        let waited = lock_sidecar_within(&target, Duration::from_millis(50));
        assert_eq!(
            waited.err().map(|error| error.kind()),
            Some(io::ErrorKind::TimedOut)
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn lock_can_be_taken_again_after_release() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("a.json");
        assert_eq!(with_sidecar_lock(&target, || Ok(1)).unwrap(), 1);
        assert_eq!(with_sidecar_lock(&target, || Ok(2)).unwrap(), 2);
        assert!(sidecar_path(&target).exists());
    }
}
