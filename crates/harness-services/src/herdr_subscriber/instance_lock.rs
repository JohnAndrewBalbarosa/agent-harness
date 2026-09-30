//! One subscriber per Herdr socket: `<root>/var/herdr-bridge/subscriber-<sha256(socket)[:16]>.lock` held for
//! the process lifetime, plus `subscriber-<id>.pid` holding the ascii pid (subscriber.py `acquire_lock`/`main`).
//!
//! Lock semantics: `std::fs::File::try_lock` (Windows `LockFileEx` over the whole file starting at byte 0,
//! Unix `flock`). It conflicts with subscriber.py's `msvcrt.locking(LK_NBLCK, 1)` on byte 0 / `fcntl.flock`
//! and is exactly what the hook's `is_subscriber_running` probe tests.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use anyhow::Context;
use harness_hook::consumers::herdr_bridge::background_launch::subscriber_lock_path;
use sha2::{Digest, Sha256};

const IDENTITY_HEX_CHARS: usize = 16;

pub enum InstanceAcquisition {
    Acquired(SubscriberInstance),
    /// Another process holds the lock (or locking failed, which subscriber.py also treated as "running").
    AlreadyRunning,
}

/// The held lock; dropping it removes the `.pid` file and releases the lock (Python's `finally`).
#[derive(Debug)]
pub struct SubscriberInstance {
    identity: String,
    lock_path: PathBuf,
    pid_path: PathBuf,
    _lock_file: File,
}

impl SubscriberInstance {
    // Mental model: mkdir DATA_ROOT → open (create) the lock file → non-blocking exclusive lock → write pid.
    pub fn acquire(harness_root: &Path, socket_path: &str) -> anyhow::Result<InstanceAcquisition> {
        let lock_path = subscriber_lock_path(harness_root, socket_path);
        if let Some(data_root) = lock_path.parent() {
            std::fs::create_dir_all(data_root)
                .with_context(|| format!("create {}", data_root.display()))?;
        }
        let lock_file = open_lock_file(&lock_path)?;
        if !has_taken_lock(&lock_file) {
            return Ok(InstanceAcquisition::AlreadyRunning);
        }
        let pid_path = pid_path_for(&lock_path);
        std::fs::write(&pid_path, std::process::id().to_string())
            .with_context(|| format!("write {}", pid_path.display()))?;
        Ok(InstanceAcquisition::Acquired(Self {
            identity: socket_identity(socket_path),
            lock_path,
            pid_path,
            _lock_file: lock_file,
        }))
    }

    /// `sha256(socket)[:16]`, logged as `socket_hash`.
    pub fn identity(&self) -> &str {
        &self.identity
    }

    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }

    pub fn pid_path(&self) -> &Path {
        &self.pid_path
    }
}

impl Drop for SubscriberInstance {
    fn drop(&mut self) {
        // `pid_path.unlink(missing_ok=True)`; the lock is released when `_lock_file` closes.
        let _ = std::fs::remove_file(&self.pid_path);
    }
}

/// `hashlib.sha256(socket_path.encode("utf-8")).hexdigest()[:16]`.
#[inline]
pub fn socket_identity(socket_path: &str) -> String {
    let digest = hex::encode(Sha256::digest(socket_path.as_bytes()));
    digest[..IDENTITY_HEX_CHARS].to_owned()
}

/// `subscriber-<id>.lock` → `subscriber-<id>.pid`.
#[inline]
pub fn pid_path_for(lock_path: &Path) -> PathBuf {
    lock_path.with_extension("pid")
}

/// `path.open("a+b")`: create when missing, never truncate.
#[inline]
fn open_lock_file(lock_path: &Path) -> anyhow::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .with_context(|| format!("open {}", lock_path.display()))
}

/// Non-blocking exclusive lock; any failure means "someone else runs" (subscriber.py returned `None`).
#[inline]
fn has_taken_lock(lock_file: &File) -> bool {
    // Both `TryLockError::WouldBlock` and `TryLockError::Error(_)` mean "not ours".
    lock_file.try_lock().is_ok()
}
