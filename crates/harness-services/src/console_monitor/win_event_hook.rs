//! Out-of-context WinEvent hooks, the `extern "system"` callback, and the message pump.
//!
//! Mental model: the hooks are installed on the monitor thread, so Windows delivers every event to
//! that thread while it sits in `GetMessageW`. The callback reaches the pure logic through a
//! thread-local `Inspector`; its body runs inside `catch_unwind` so no panic crosses the FFI boundary.

use std::cell::RefCell;
use std::panic::{catch_unwind, AssertUnwindSafe};

use windows::Win32::Foundation::{GetLastError, HWND};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    GetMessageW, MSG, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
};

use super::dedupe_cache::{EVENT_OBJECT_SHOW, EVENT_SYSTEM_FOREGROUND};
use super::monitor_log::MonitorLog;
use super::monitor_records::inspect_failed;
use super::window_inspector::{Inspector, WinEvent};

/// Events hooked, in Python order.
pub(super) const HOOKED_EVENTS: [u32; 2] = [EVENT_OBJECT_SHOW, EVENT_SYSTEM_FOREGROUND];

thread_local! {
    static INSPECTOR: RefCell<Option<Inspector>> = const { RefCell::new(None) };
}

/// A `SetWinEventHook` that returned null.
pub(super) struct HookFailure {
    pub(super) event_id: u32,
    pub(super) error_code: u32,
}

/// Installed hooks plus the thread-local inspector; dropping it unhooks and clears the inspector.
pub(super) struct InstalledHooks {
    hooks: Vec<HWINEVENTHOOK>,
}

impl Drop for InstalledHooks {
    fn drop(&mut self) {
        for hook in self.hooks.drain(..) {
            // SAFETY: each hook came from a successful SetWinEventHook on this thread and is
            // unhooked exactly once.
            let _ = unsafe { UnhookWinEvent(hook) };
        }
        INSPECTOR.with(|slot| {
            if let Ok(mut inspector) = slot.try_borrow_mut() {
                *inspector = None;
            }
        });
    }
}

/// Installs the inspector and both hooks on the calling thread; on failure the hooks installed so far
/// are removed (by drop) and the failing event is reported.
pub(super) fn install(log: MonitorLog) -> Result<InstalledHooks, HookFailure> {
    INSPECTOR.with(|slot| *slot.borrow_mut() = Some(Inspector::new(log)));
    let mut installed = InstalledHooks { hooks: Vec::new() };
    for event in HOOKED_EVENTS {
        // SAFETY: `on_win_event` matches WINEVENTPROC, lives for the whole program, and the flags
        // request out-of-context delivery (no DLL injection, so no module handle is needed).
        let hook = unsafe {
            SetWinEventHook(
                event,
                event,
                None,
                Some(on_win_event),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            )
        };
        if hook.is_invalid() {
            // SAFETY: reads the thread's last-error value set by the failed SetWinEventHook.
            let error_code = unsafe { GetLastError() }.0;
            return Err(HookFailure {
                event_id: event,
                error_code,
            });
        }
        installed.hooks.push(hook);
    }
    Ok(installed)
}

/// Pumps messages until `WM_QUIT` (0) or an error (-1); hook callbacks run inside `GetMessageW`.
pub(super) fn pump_messages() {
    let mut message = MSG::default();
    // SAFETY: `message` is a valid writable MSG; a `None` window means all messages of this thread.
    while unsafe { GetMessageW(&mut message, None, 0, 0) }.0 > 0 {}
}

/// WinEvent callback: forwards to the thread-local inspector; never unwinds into Windows.
unsafe extern "system" fn on_win_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    object_id: i32,
    child_id: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    let win_event = WinEvent {
        event,
        hwnd,
        object_id,
        child_id,
    };
    let outcome = catch_unwind(AssertUnwindSafe(|| dispatch(win_event)));
    let failure = match outcome {
        Ok(Ok(())) => return,
        Ok(Err(error_type)) => error_type,
        Err(_) => "panic",
    };
    // Reporting must not panic either: a second panic here would abort at the FFI boundary.
    let _ = catch_unwind(AssertUnwindSafe(|| report_failure(failure)));
}

/// Runs the inspector; `Err` carries the Python-style `errorType` string.
#[inline]
fn dispatch(win_event: WinEvent) -> Result<(), &'static str> {
    INSPECTOR.with(|slot| {
        let mut guard = slot.try_borrow_mut().map_err(|_| "ReentrantCallback")?;
        if let Some(inspector) = guard.as_mut() {
            inspector.inspect(win_event);
        }
        Ok(())
    })
}

#[inline]
fn report_failure(error_type: &str) {
    INSPECTOR.with(|slot| {
        if let Ok(guard) = slot.try_borrow() {
            if let Some(inspector) = guard.as_ref() {
                // No other sink exists for a failed log write.
                let _ = inspector.log.emit(&inspect_failed(error_type));
            }
        }
    });
}
