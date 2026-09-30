//! Ctrl+C handling: the supervisor survives the interrupt long enough to terminate the child and
//! still write its bounded logs, final state and completion record.
//!
//! Deviation from Python (which died with `KeyboardInterrupt` and left the child running on
//! Windows): Ctrl+C / Ctrl+Break (Windows) and SIGINT / SIGTERM (POSIX) set a flag that the
//! supervision loop turns into "terminate the child".
//!
//! No extra crate: one `SetConsoleCtrlHandler` / `signal` declaration each; the handlers only
//! store an atomic flag (async-signal-safe).

use std::sync::atomic::{AtomicBool, Ordering};

static INTERRUPT_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Installs the handler once; failure to install only means Ctrl+C keeps its default meaning.
pub fn install_interrupt_handler() {
    platform::install();
}

#[inline]
pub fn interrupt_requested() -> bool {
    INTERRUPT_REQUESTED.load(Ordering::SeqCst)
}

#[inline]
fn record_interrupt() {
    INTERRUPT_REQUESTED.store(true, Ordering::SeqCst);
}

#[cfg(windows)]
mod platform {
    const CTRL_C_EVENT: u32 = 0;
    const CTRL_BREAK_EVENT: u32 = 1;
    const HANDLED: i32 = 1;
    const NOT_HANDLED: i32 = 0;
    const ADD_HANDLER: i32 = 1;

    type ConsoleControlHandler = unsafe extern "system" fn(u32) -> i32;

    #[link(name = "kernel32")]
    extern "system" {
        fn SetConsoleCtrlHandler(handler: Option<ConsoleControlHandler>, add: i32) -> i32;
    }

    /// Ctrl+C / Ctrl+Break are handled (the child gets them too via the shared console);
    /// close / logoff / shutdown keep the default behaviour.
    unsafe extern "system" fn on_console_control(event: u32) -> i32 {
        if event == CTRL_C_EVENT || event == CTRL_BREAK_EVENT {
            super::record_interrupt();
            HANDLED
        } else {
            NOT_HANDLED
        }
    }

    pub(super) fn install() {
        // SAFETY: registers a process-wide handler with the documented signature; the handler
        // only stores an atomic flag and never unwinds.
        let _ = unsafe { SetConsoleCtrlHandler(Some(on_console_control), ADD_HANDLER) };
    }
}

#[cfg(unix)]
mod platform {
    use std::os::raw::c_int;

    const SIGINT: c_int = 2;
    const SIGTERM: c_int = 15;

    extern "C" {
        fn signal(signum: c_int, handler: extern "C" fn(c_int)) -> usize;
    }

    extern "C" fn on_signal(_signum: c_int) {
        super::record_interrupt();
    }

    pub(super) fn install() {
        for signum in [SIGINT, SIGTERM] {
            // SAFETY: libc `signal` with a handler that only performs an atomic store
            // (async-signal-safe); the previous handler value is intentionally discarded.
            let _ = unsafe { signal(signum, on_signal) };
        }
    }
}

#[cfg(not(any(windows, unix)))]
mod platform {
    pub(super) fn install() {}
}
