//! Turns one WinEvent into at most one `console.window.visible` record (Python `on_window` body).

use std::time::Instant;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetClassNameW, GetWindowThreadProcessId, IsWindowVisible, GA_ROOT,
};

use super::dedupe_cache::DedupeCache;
use super::monitor_log::MonitorLog;
use super::monitor_records::{window_visible, VisibleWindow};
use super::process_ancestry::{parent_pid, process_name, walk_ancestry, ProcessTable};
use super::win32_process::{executable_path, snapshot_processes, wide_until_nul};
use super::window_filter::{is_console_class, is_console_process, is_window_object_event};

/// Python `create_unicode_buffer(128)` for class names.
const CLASS_NAME_CAPACITY: usize = 128;

/// State the WinEvent callback needs between events (lives on the monitor thread only).
pub(super) struct Inspector {
    pub(super) log: MonitorLog,
    pub(super) dedupe: DedupeCache,
}

/// Raw WinEvent fields the inspector uses.
#[derive(Clone, Copy)]
pub(super) struct WinEvent {
    pub(super) event: u32,
    pub(super) hwnd: HWND,
    pub(super) object_id: i32,
    pub(super) child_id: i32,
}

/// A top-level visible window with its owner pid and class name (no title is ever read).
struct Candidate {
    pid: u32,
    class_name: String,
}

impl Inspector {
    pub(super) fn new(log: MonitorLog) -> Self {
        Self {
            log,
            dedupe: DedupeCache::new(),
        }
    }

    /// Mental model: cheap filters → console test (snapshot only if the class name is not enough)
    /// → dedupe → snapshot (if not yet taken) → ancestry → emit when a `codex.exe` ancestor exists.
    pub(super) fn inspect(&mut self, win_event: WinEvent) {
        let Some(candidate) = top_level_candidate(win_event) else {
            return;
        };
        let mut snapshot: Option<ProcessTable> = None;
        if !is_console_class(&candidate.class_name) {
            let table = snapshot.insert(snapshot_processes());
            if !is_console_process(process_name(table, candidate.pid)) {
                return;
            }
        }
        let key = (win_event.hwnd.0 as usize, candidate.pid);
        if !self.dedupe.admit(key, win_event.event, Instant::now()) {
            return;
        }
        let table = snapshot.get_or_insert_with(snapshot_processes);
        self.report_if_codex_descendant(win_event.event, &candidate, table);
    }

    fn report_if_codex_descendant(&self, event: u32, candidate: &Candidate, table: &ProcessTable) {
        let ancestry = walk_ancestry(candidate.pid, table, executable_path);
        if ancestry.codex_pid.is_none() {
            return;
        }
        let window = VisibleWindow {
            event_id: event,
            pid: candidate.pid,
            parent_pid: parent_pid(table, candidate.pid),
            process: process_name(table, candidate.pid),
            window_class: &candidate.class_name,
        };
        // A log write failure has no other sink; the monitor keeps running.
        let _ = self.log.emit(&window_visible(&window, &ancestry));
    }
}

/// Snapshot-free filters: whole-window event, visible, root window; then pid and class name.
fn top_level_candidate(win_event: WinEvent) -> Option<Candidate> {
    let hwnd = win_event.hwnd;
    if !is_window_object_event(hwnd.is_invalid(), win_event.object_id, win_event.child_id) {
        return None;
    }
    if !is_visible(hwnd) || !is_root_window(hwnd) {
        return None;
    }
    Some(Candidate {
        pid: owner_pid(hwnd),
        class_name: class_name(hwnd),
    })
}

#[inline]
fn is_visible(hwnd: HWND) -> bool {
    // SAFETY: IsWindowVisible accepts any HWND value, including stale ones (returns FALSE).
    unsafe { IsWindowVisible(hwnd) }.as_bool()
}

#[inline]
fn is_root_window(hwnd: HWND) -> bool {
    // SAFETY: GetAncestor accepts any HWND value and returns null for invalid windows.
    let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
    root == hwnd
}

#[inline]
fn owner_pid(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY: `pid` is a valid writable u32 for the duration of the call; stale HWNDs leave it 0.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

#[inline]
fn class_name(hwnd: HWND) -> String {
    let mut buffer = [0u16; CLASS_NAME_CAPACITY];
    // SAFETY: the slice is writable and its length is passed as the capacity by the wrapper.
    let copied = unsafe { GetClassNameW(hwnd, &mut buffer) };
    let copied = usize::try_from(copied).unwrap_or(0).min(buffer.len());
    wide_until_nul(&buffer[..copied])
}
