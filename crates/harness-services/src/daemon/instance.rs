//! The `serve` daemon's single-instance guard.
//!
//! - Windows: named mutex `Local\AgentHarnessServices`, held for the daemon's lifetime (the kernel drops it
//!   when the process dies, so a crash never leaves a stale guard).
//! - Elsewhere: an exclusive lock on `var/services-daemon.lock`.

use crate::service_context::ServiceContext;

pub const DAEMON_MUTEX_NAME: &str = r"Local\AgentHarnessServices";
pub const DAEMON_LOCK_FILE: &str = "services-daemon.lock";

pub use platform::{acquire, is_daemon_running, DaemonInstance};

/// `<root>/var/services-daemon.lock` (the guard off Windows).
pub fn daemon_lock_path(context: &ServiceContext) -> std::path::PathBuf {
    context.var_dir().join(DAEMON_LOCK_FILE)
}

#[cfg(windows)]
mod platform {
    use std::io;

    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, HANDLE,
    };
    use windows::Win32::System::Threading::{
        CreateMutexW, OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE,
    };

    use crate::service_context::ServiceContext;

    const MUTEX_NAME: PCWSTR = w!("Local\\AgentHarnessServices");

    /// Owns the mutex handle; the mutex object lives while any handle is open.
    pub struct DaemonInstance {
        handle: HANDLE,
    }

    impl Drop for DaemonInstance {
        fn drop(&mut self) {
            close_handle(self.handle);
        }
    }

    // Mental model: create-or-open the named mutex → `ERROR_ALREADY_EXISTS` means another daemon owns it.
    /// `Ok(None)` when another daemon already runs.
    pub fn acquire(_context: &ServiceContext) -> io::Result<Option<DaemonInstance>> {
        // SAFETY: `MUTEX_NAME` is a static NUL-terminated wide string; no security attributes are passed.
        let handle = unsafe { CreateMutexW(None, false, MUTEX_NAME) }
            .map_err(|error| io::Error::other(error.to_string()))?;
        // SAFETY: reads the calling thread's last-error value set by `CreateMutexW` just above.
        let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        if already_exists {
            close_handle(handle);
            return Ok(None);
        }
        Ok(Some(DaemonInstance { handle }))
    }

    /// Opening (not creating) the mutex succeeds only while a daemon holds it; access-denied also means it exists.
    pub fn is_daemon_running(_context: &ServiceContext) -> bool {
        // SAFETY: static wide-string name; the returned handle is closed right away.
        match unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, MUTEX_NAME) } {
            Ok(handle) => {
                close_handle(handle);
                true
            }
            Err(error) => error.code() == ERROR_ACCESS_DENIED.to_hresult(),
        }
    }

    #[inline]
    fn close_handle(handle: HANDLE) {
        // SAFETY: `handle` came from CreateMutexW/OpenMutexW and is closed exactly once.
        if let Err(error) = unsafe { CloseHandle(handle) } {
            eprintln!("harness-services: CloseHandle failed ({error})");
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use std::io;

    use super::daemon_lock_path;
    use crate::ensure::file_guard::{is_held_elsewhere, try_exclusive, ExclusiveFileGuard};
    use crate::service_context::ServiceContext;

    /// Holds `var/services-daemon.lock` for the daemon's lifetime.
    pub struct DaemonInstance {
        _guard: ExclusiveFileGuard,
    }

    /// `Ok(None)` when another daemon already runs.
    pub fn acquire(context: &ServiceContext) -> io::Result<Option<DaemonInstance>> {
        Ok(
            try_exclusive(&daemon_lock_path(context))?
                .map(|guard| DaemonInstance { _guard: guard }),
        )
    }

    pub fn is_daemon_running(context: &ServiceContext) -> bool {
        is_held_elsewhere(&daemon_lock_path(context))
    }
}
