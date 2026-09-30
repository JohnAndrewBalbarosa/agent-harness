//! RAII wrapper that closes a kernel `HANDLE` exactly once.

use windows::Win32::Foundation::{CloseHandle, HANDLE};

/// Owns a valid (non-null, non-`INVALID_HANDLE_VALUE`) kernel handle and closes it on drop.
pub(super) struct OwnedHandle(HANDLE);

impl OwnedHandle {
    /// Takes ownership of `handle`; the caller must not close it elsewhere.
    #[inline]
    pub(super) fn new(handle: HANDLE) -> Self {
        Self(handle)
    }

    #[inline]
    pub(super) fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: `self.0` came from a successful Win32 open/create call, is owned solely by this
        // wrapper, and is closed exactly once here. A close failure leaves nothing to recover.
        let _ = unsafe { CloseHandle(self.0) };
    }
}
