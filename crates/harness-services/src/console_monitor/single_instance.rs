//! Named mutex `Local\CodexSharedConsoleWindowMonitor` shared with the Python monitor.

use windows::core::w;
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
use windows::Win32::System::Threading::CreateMutexW;

use super::owned_handle::OwnedHandle;

/// Outcome of claiming the per-session monitor slot.
pub(super) enum InstanceClaim {
    /// This process owns the slot for as long as the guard lives.
    Acquired(OwnedHandle),
    /// Another monitor (Rust or Python) already runs in this session.
    AlreadyRunning,
    /// `CreateMutexW` returned null; carries the Win32 error code.
    Unavailable(u32),
}

/// Creates (or opens) the named mutex; existence of the name is the single-instance signal.
pub(super) fn claim() -> InstanceClaim {
    // SAFETY: the name is a static NUL-terminated wide string and no security attributes are passed.
    let created =
        unsafe { CreateMutexW(None, false, w!("Local\\CodexSharedConsoleWindowMonitor")) };
    // SAFETY: GetLastError only reads the calling thread's last-error value, set by CreateMutexW above.
    let last_error = unsafe { GetLastError() };
    match created {
        Err(_) => InstanceClaim::Unavailable(last_error.0),
        Ok(handle) => {
            let guard = OwnedHandle::new(handle);
            if last_error == ERROR_ALREADY_EXISTS {
                drop(guard);
                InstanceClaim::AlreadyRunning
            } else {
                InstanceClaim::Acquired(guard)
            }
        }
    }
}
