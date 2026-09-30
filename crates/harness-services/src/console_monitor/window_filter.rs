//! Cheap, snapshot-free window filters and the console classification (Python `is_console_window`).

/// `OBJID_WINDOW`: the event concerns the window itself, not a child object.
pub const OBJID_WINDOW: i32 = 0;
/// `CHILDID_SELF`.
pub const CHILDID_SELF: i32 = 0;

/// Executables whose top-level windows count as consoles (compared lowercase).
pub const CONSOLE_PROCESSES: [&str; 5] = [
    "cmd.exe",
    "conhost.exe",
    "openconsole.exe",
    "powershell.exe",
    "pwsh.exe",
];

/// Whether a WinEvent refers to a whole window (non-null hwnd, `OBJID_WINDOW`, `CHILDID_SELF`).
#[inline]
pub fn is_window_object_event(hwnd_is_null: bool, object_id: i32, child_id: i32) -> bool {
    !hwnd_is_null && object_id == OBJID_WINDOW && child_id == CHILDID_SELF
}

/// Class-name half of the console test; needs no process snapshot.
#[inline]
pub fn is_console_class(class_name: &str) -> bool {
    class_name.to_lowercase().contains("console")
}

/// Process-name half of the console test; needs the process snapshot to know the exe name.
#[inline]
pub fn is_console_process(process_name: &str) -> bool {
    let lowered = process_name.to_lowercase();
    CONSOLE_PROCESSES.contains(&lowered.as_str())
}

/// Full console test, identical to Python: exe name in the set OR class name contains "console".
#[inline]
pub fn is_console_window(process_name: &str, class_name: &str) -> bool {
    is_console_process(process_name) || is_console_class(class_name)
}

/// First 80 characters (Unicode scalar values, like Python `str[:80]`) of a window class.
#[inline]
pub fn truncate_window_class(class_name: &str) -> String {
    class_name.chars().take(80).collect()
}
