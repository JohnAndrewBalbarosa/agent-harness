//! Win32 process facts: ToolHelp32 snapshot and full image path (Python `processes`, `executable_path`).

use windows::core::PWSTR;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};

use super::owned_handle::OwnedHandle;
use super::process_ancestry::{ProcessEntry, ProcessTable};

/// Wide-character capacity for image paths (Python used 32768).
const IMAGE_PATH_CAPACITY: usize = 32_768;

/// Snapshot of every process (pid → exe name, parent pid). Empty when the snapshot fails (Python parity).
pub(super) fn snapshot_processes() -> ProcessTable {
    // SAFETY: plain FFI call with a valid flag; the returned handle is owned by `OwnedHandle` below.
    let snapshot = match unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) } {
        Ok(handle) => OwnedHandle::new(handle),
        Err(_) => return ProcessTable::new(),
    };
    collect_entries(&snapshot)
}

/// Mental model: First → (record, Next)* until Next fails (end of list or error).
fn collect_entries(snapshot: &OwnedHandle) -> ProcessTable {
    let mut table = ProcessTable::new();
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    // SAFETY: `snapshot` is a live ToolHelp32 handle and `entry` is a properly sized, writable struct.
    if unsafe { Process32FirstW(snapshot.raw(), &mut entry) }.is_err() {
        return table;
    }
    loop {
        table.insert(entry.th32ProcessID, to_process_entry(&entry));
        // SAFETY: same handle and struct as above; `dwSize` is preserved between calls.
        if unsafe { Process32NextW(snapshot.raw(), &mut entry) }.is_err() {
            return table;
        }
    }
}

#[inline]
fn to_process_entry(entry: &PROCESSENTRY32W) -> ProcessEntry {
    ProcessEntry {
        name: wide_until_nul(&entry.szExeFile),
        parent_pid: entry.th32ParentProcessID,
    }
}

/// Full Win32 image path of `pid`, or an empty string when it cannot be opened or queried.
pub(super) fn executable_path(pid: u32) -> String {
    // SAFETY: plain FFI call; the returned handle is owned by `OwnedHandle` below.
    let process = match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) } {
        Ok(handle) => OwnedHandle::new(handle),
        Err(_) => return String::new(),
    };
    let mut buffer = vec![0u16; IMAGE_PATH_CAPACITY];
    let mut length = IMAGE_PATH_CAPACITY as u32;
    // SAFETY: `buffer` holds `length` writable u16s; on success `length` is set to the characters
    // written (excluding the NUL) and never exceeds the capacity.
    let queried = unsafe {
        QueryFullProcessImageNameW(
            process.raw(),
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    };
    match queried {
        Ok(()) => {
            let written = (length as usize).min(buffer.len());
            String::from_utf16_lossy(&buffer[..written])
        }
        Err(_) => String::new(),
    }
}

/// Decodes a fixed wide-char buffer up to its first NUL.
#[inline]
pub(super) fn wide_until_nul(buffer: &[u16]) -> String {
    let end = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}
