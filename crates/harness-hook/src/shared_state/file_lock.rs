//! Cross-process exclusive lock on a sidecar `<path>.lock` file (`core/locking.py`).
//! Python locks byte 0 (length 1) with `msvcrt.locking`; `File::lock` takes an exclusive `LockFileEx`
//! over the whole file on Windows (and `flock` elsewhere), which overlaps byte 0, so both sides exclude
//! each other.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// Holds the lock until dropped.
pub struct SidecarLock {
    file: File,
}

impl Drop for SidecarLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// Blocks until the sidecar lock of `path` is held.
pub fn lock_sidecar(path: &Path) -> io::Result<SidecarLock> {
    let file = open_sidecar(&sidecar_path(path))?;
    file.lock()?;
    Ok(SidecarLock { file })
}

/// Runs `work` while holding the sidecar lock of `path`.
pub fn with_sidecar_lock<T>(path: &Path, work: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    let _guard = lock_sidecar(path)?;
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
    fn lock_can_be_taken_again_after_release() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("a.json");
        assert_eq!(with_sidecar_lock(&target, || Ok(1)).unwrap(), 1);
        assert_eq!(with_sidecar_lock(&target, || Ok(2)).unwrap(), 2);
        assert!(sidecar_path(&target).exists());
    }
}
