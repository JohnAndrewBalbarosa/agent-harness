//! `_windows_notification_state()`: Focus Assist / fullscreen state recorded with every decision, to
//! explain toasts Windows suppressed. Python ignores the HRESULT and reports the zero-initialized value,
//! so a failed query is `unknown(0)`; non-Windows hosts report `n/a`.

#[cfg(windows)]
pub fn windows_notification_state() -> String {
    use windows::Win32::UI::Shell::SHQueryUserNotificationState;

    // SAFETY: the wrapper owns the out-parameter; no caller memory is passed.
    #[allow(unused_unsafe)]
    let queried = unsafe { SHQueryUserNotificationState() };
    match queried {
        Ok(state) => state_label(state.0),
        Err(_) => state_label(0),
    }
}

#[cfg(not(windows))]
pub fn windows_notification_state() -> String {
    "n/a".to_owned()
}

/// `QUERY_USER_NOTIFICATION_STATE` names used by the Python consumer.
pub fn state_label(value: i32) -> String {
    let label = match value {
        1 => "not-present",
        2 => "busy-fullscreen",
        3 => "d3d-fullscreen",
        4 => "presentation-mode",
        5 => "accepts-notifications",
        6 => "quiet-time",
        7 => "app-fullscreen",
        other => return format!("unknown({other})"),
    };
    label.to_owned()
}
