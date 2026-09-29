//! `process_context()` (`status_common.py`): which `codex.exe` owns this hook, and which shell / terminal
//! window hosts it, so `orb.py` can validate the session process and focus its terminal.
//!
//! Mental model: ancestry `[self, parent, grandparent, ...]` (ToolHelp32 snapshot, pid-reuse guarded by
//! creation time like `psutil.Process.parent`) → first `codex.exe` → above it the first shell, then the first
//! terminal host (fallback: the shell as a `console`). Non-Windows builds report no ancestry.

use serde_json::{Map, Value};

use super::orb_log::OrbLog;

const CODEX_PROCESS: &str = "codex.exe";
const SHELL_PROCESSES: [&str; 3] = ["cmd.exe", "powershell.exe", "pwsh.exe"];
const TERMINAL_PROCESSES: [&str; 3] = ["windowsterminal.exe", "openconsole.exe", "conhost.exe"];

/// One ancestor: lowercased executable name and `psutil` creation time (epoch seconds) when readable.
#[derive(Debug, Clone, PartialEq)]
pub struct AncestorProcess {
    pub pid: u32,
    pub name: String,
    pub create_time: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessContext {
    pub codex_pid: u32,
    pub codex_create_time: f64,
    pub shell_pid: u32,
    pub terminal_pid: u32,
    pub terminal_kind: String,
    pub wt_session: String,
}

impl ProcessContext {
    pub fn unknown(wt_session: &str) -> Self {
        Self {
            codex_pid: 0,
            codex_create_time: 0.0,
            shell_pid: 0,
            terminal_pid: 0,
            terminal_kind: "unknown".to_owned(),
            wt_session: wt_session.to_owned(),
        }
    }

    /// `current.update(process_context())`, in Python key order.
    pub fn append_to(&self, record: &mut Map<String, Value>) {
        record.insert("codex_pid".into(), Value::from(self.codex_pid));
        record.insert(
            "codex_create_time".into(),
            Value::from(self.codex_create_time),
        );
        record.insert("shell_pid".into(), Value::from(self.shell_pid));
        record.insert("terminal_pid".into(), Value::from(self.terminal_pid));
        record.insert(
            "terminal_kind".into(),
            Value::from(self.terminal_kind.as_str()),
        );
        record.insert("wt_session".into(), Value::from(self.wt_session.as_str()));
    }
}

/// Reads this process's ancestry and resolves it; failures are logged like `process-context-error ...`.
pub fn current_process_context(wt_session: &str, log: &OrbLog) -> ProcessContext {
    match ancestry::current_ancestry() {
        Ok(ancestors) => resolve_process_context(&ancestors, wt_session),
        Err(error) => {
            log.info(&format!("process-context-error {error}"));
            ProcessContext::unknown(wt_session)
        }
    }
}

/// The pure part of `process_context()`: `ancestors[0]` is the current process.
pub fn resolve_process_context(ancestors: &[AncestorProcess], wt_session: &str) -> ProcessContext {
    let mut context = ProcessContext::unknown(wt_session);
    let Some(codex_index) = ancestors
        .iter()
        .position(|process| process.name == CODEX_PROCESS)
    else {
        return context;
    };
    let codex = &ancestors[codex_index];
    context.codex_pid = codex.pid;
    context.codex_create_time = codex.create_time.unwrap_or(0.0);

    for parent in &ancestors[codex_index + 1..] {
        if context.shell_pid == 0 && SHELL_PROCESSES.contains(&parent.name.as_str()) {
            context.shell_pid = parent.pid;
        }
        if TERMINAL_PROCESSES.contains(&parent.name.as_str()) {
            context.terminal_pid = parent.pid;
            context.terminal_kind = parent.name.trim_end_matches(".exe").to_owned();
            break;
        }
    }
    if context.terminal_pid == 0 {
        context.terminal_pid = context.shell_pid;
        context.terminal_kind = "console".to_owned();
    }
    context
}

#[cfg(windows)]
mod ancestry {
    //! ToolHelp32 process snapshot + `GetProcessTimes`.

    use std::collections::{HashMap, HashSet};

    use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    use super::AncestorProcess;

    /// FILETIME ticks (100 ns since 1601) at the Unix epoch.
    const UNIX_EPOCH_TICKS: u64 = 116_444_736_000_000_000;
    const TICKS_PER_SECOND: f64 = 10_000_000.0;

    struct SnapshotEntry {
        parent_pid: u32,
        name: String,
    }

    // Mental model: snapshot pid → (ppid, name) once, then walk ppid links while the parent is not newer
    // than its child (a newer "parent" is a reused pid, where psutil stops too).
    pub fn current_ancestry() -> Result<Vec<AncestorProcess>, String> {
        let snapshot = process_snapshot()?;
        let mut ancestors = Vec::new();
        let mut visited = HashSet::new();
        let mut pid = std::process::id();

        while let Some(entry) = snapshot.get(&pid) {
            if !visited.insert(pid) {
                break;
            }
            let create_time = process_create_time(pid);
            if let Some(child) = ancestors.last() {
                if is_newer_than_child(create_time, child) {
                    break;
                }
            }
            ancestors.push(AncestorProcess {
                pid,
                name: entry.name.to_lowercase(),
                create_time,
            });
            if pid == 0 || entry.parent_pid == 0 {
                break;
            }
            pid = entry.parent_pid;
        }
        Ok(ancestors)
    }

    #[inline]
    fn is_newer_than_child(parent_time: Option<f64>, child: &AncestorProcess) -> bool {
        matches!((parent_time, child.create_time), (Some(parent), Some(child)) if parent > child)
    }

    fn process_snapshot() -> Result<HashMap<u32, SnapshotEntry>, String> {
        // SAFETY: plain Win32 calls; the entry is sized before use and the handle is closed below.
        unsafe {
            let handle = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
                .map_err(|error| error.to_string())?;
            let entries = read_snapshot(handle);
            let _ = CloseHandle(handle);
            Ok(entries)
        }
    }

    unsafe fn read_snapshot(handle: HANDLE) -> HashMap<u32, SnapshotEntry> {
        let mut entries = HashMap::new();
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut has_entry = Process32FirstW(handle, &mut entry).is_ok();
        while has_entry {
            entries.insert(
                entry.th32ProcessID,
                SnapshotEntry {
                    parent_pid: entry.th32ParentProcessID,
                    name: wide_name(&entry.szExeFile),
                },
            );
            has_entry = Process32NextW(handle, &mut entry).is_ok();
        }
        entries
    }

    #[inline]
    fn wide_name(raw: &[u16]) -> String {
        let length = raw.iter().position(|unit| *unit == 0).unwrap_or(raw.len());
        String::from_utf16_lossy(&raw[..length])
    }

    /// `psutil.Process(pid).create_time()`; `None` when the process cannot be queried.
    fn process_create_time(pid: u32) -> Option<f64> {
        // SAFETY: the handle is only used for GetProcessTimes and closed before returning.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut creation = FILETIME::default();
            let mut exit = FILETIME::default();
            let mut kernel = FILETIME::default();
            let mut user = FILETIME::default();
            let queried = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user);
            let _ = CloseHandle(handle);
            queried.ok()?;
            Some(filetime_to_epoch_seconds(&creation))
        }
    }

    #[inline]
    fn filetime_to_epoch_seconds(time: &FILETIME) -> f64 {
        let ticks = (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
        ticks.saturating_sub(UNIX_EPOCH_TICKS) as f64 / TICKS_PER_SECOND
    }
}

#[cfg(not(windows))]
mod ancestry {
    use super::AncestorProcess;

    /// No `codex.exe` ancestry outside Windows (Python finds none either).
    pub fn current_ancestry() -> Result<Vec<AncestorProcess>, String> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, name: &str) -> AncestorProcess {
        AncestorProcess {
            pid,
            name: name.to_owned(),
            create_time: Some(f64::from(pid)),
        }
    }

    #[test]
    fn finds_codex_shell_and_windows_terminal() {
        let ancestors = [
            process(1, "harness-hook.exe"),
            process(2, "cmd.exe"),
            process(3, "codex.exe"),
            process(4, "pwsh.exe"),
            process(5, "cmd.exe"),
            process(6, "windowsterminal.exe"),
            process(7, "explorer.exe"),
        ];
        let context = resolve_process_context(&ancestors, "wt-1");
        assert_eq!(context.codex_pid, 3);
        assert_eq!(context.codex_create_time, 3.0);
        assert_eq!(context.shell_pid, 4);
        assert_eq!(context.terminal_pid, 6);
        assert_eq!(context.terminal_kind, "windowsterminal");
        assert_eq!(context.wt_session, "wt-1");
    }

    #[test]
    fn shell_becomes_the_console_without_a_terminal_host() {
        let ancestors = [
            process(1, "hook.exe"),
            process(3, "codex.exe"),
            process(4, "powershell.exe"),
        ];
        let context = resolve_process_context(&ancestors, "");
        assert_eq!(
            (context.terminal_pid, context.terminal_kind.as_str()),
            (4, "console")
        );
    }

    #[test]
    fn without_codex_everything_stays_unknown() {
        let context = resolve_process_context(&[process(1, "claude.exe")], "");
        assert_eq!(context, ProcessContext::unknown(""));
    }

    #[test]
    fn key_order_matches_python() {
        let mut record = Map::new();
        ProcessContext::unknown("w").append_to(&mut record);
        let keys: Vec<&str> = record.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "codex_pid",
                "codex_create_time",
                "shell_pid",
                "terminal_pid",
                "terminal_kind",
                "wt_session"
            ]
        );
        assert_eq!(record["codex_create_time"], Value::from(0.0));
    }
}
