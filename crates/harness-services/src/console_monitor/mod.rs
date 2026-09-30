//! Console window monitor: records visible console windows whose process has a `codex.exe` ancestor.
//!
//! Port of `tools/observability-client/console_window_monitor.py` (record shape, mutex name, filters,
//! dedupe and log rotation are kept byte-compatible). Never captures window titles or command lines.
//!
//! Mental model:
//! `spawn` → dedicated thread → single-instance mutex → WinEvent hooks (SHOW, FOREGROUND) → message pump.
//! Each WinEvent → cheap window filters → console test (class name first, process snapshot only when
//! needed) → dedupe → parent walk for `codex.exe` → one `console.window.visible` record.
//!
//! Pure logic (profile mapping, console classification, dedupe, ancestry walk, records, log file)
//! is platform-independent and unit-tested on Linux; all Win32 code lives under `cfg(windows)`.

pub mod dedupe_cache;
pub mod monitor_log;
pub mod monitor_records;
pub mod process_ancestry;
pub mod profile_mapping;
pub mod window_filter;

#[cfg(windows)]
mod monitor_thread;
#[cfg(windows)]
mod owned_handle;
#[cfg(windows)]
mod single_instance;
#[cfg(windows)]
mod win32_process;
#[cfg(windows)]
mod win_event_hook;
#[cfg(windows)]
mod window_inspector;

use crate::service_context::ServiceContext;

/// Starts the monitor on its own thread (it needs a Win32 message pump) and returns immediately.
///
/// On non-Windows targets this is a documented no-op returning `Ok(())`.
/// Fails only when the home directory cannot be resolved or the thread cannot be spawned.
#[cfg(windows)]
pub fn spawn(context: ServiceContext) -> anyhow::Result<()> {
    // Mental model: resolve the log file up front (fail loudly), then hand everything to the thread.
    let log = monitor_log::MonitorLog::from_env(|key| context.env_value(key).map(str::to_owned))
        .ok_or_else(|| anyhow::anyhow!("console monitor: neither USERPROFILE nor HOME is set"))?;
    std::thread::Builder::new()
        .name("console-window-monitor".to_owned())
        .spawn(move || monitor_thread::run(log))
        .map_err(|error| anyhow::anyhow!("console monitor: thread spawn failed: {error}"))?;
    Ok(())
}

/// No-op on non-Windows targets: there are no console windows or WinEvent hooks to observe.
#[cfg(not(windows))]
pub fn spawn(context: ServiceContext) -> anyhow::Result<()> {
    let _ = context;
    Ok(())
}
