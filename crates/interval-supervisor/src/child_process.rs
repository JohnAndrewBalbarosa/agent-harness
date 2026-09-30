//! Starting the child like `subprocess.Popen(command, cwd=root, stdout=PIPE, stderr=PIPE,
//! stdin=DEVNULL)` (interval_supervisor.py:203-213) and reading its `returncode`.
//!
//! The child inherits the supervisor's console exactly like Python's default `creationflags`,
//! so no extra console window appears.

use std::io;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};

use crate::completion_record::StartFailure;

/// Python raises `IndexError` for `Popen([])` on POSIX.
const EMPTY_COMMAND_EXCEPTION: &str = "IndexError";
const EMPTY_COMMAND_MESSAGE: &str = "list index out of range";

pub fn spawn_child(command: &[String], cwd: &Path) -> Result<Child, StartFailure> {
    let Some((program, arguments)) = command.split_first() else {
        return Err(StartFailure {
            exception_type: EMPTY_COMMAND_EXCEPTION.to_owned(),
            message: EMPTY_COMMAND_MESSAGE.to_owned(),
        });
    };

    Command::new(program)
        .args(arguments)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| describe_start_error(&error, program))
}

/// Python `Popen.returncode`: negative signal number for a signalled POSIX child, the unsigned
/// 32-bit exit code on Windows.
pub fn python_returncode(status: ExitStatus) -> i64 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return -i64::from(signal);
        }
    }
    #[cfg(windows)]
    {
        if let Some(code) = status.code() {
            // Bit-for-bit reinterpretation of the Win32 DWORD exit code.
            return i64::from(code as u32);
        }
    }
    status.code().map(i64::from).unwrap_or(-1)
}

/// The value handed to `std::process::exit` so the OS reports the same status Python's
/// `raise SystemExit(returncode)` produced (truncated by the platform the same way).
#[inline]
pub fn process_exit_code(returncode: i64) -> i32 {
    returncode as i32
}

/// Python exception class name + `str(error)` for an `OSError` raised by `Popen`.
fn describe_start_error(error: &io::Error, program: &str) -> StartFailure {
    StartFailure {
        exception_type: python_exception_type(error).to_owned(),
        message: python_error_text(error, program),
    }
}

/// CPython maps errno / winerror to `OSError` subclasses; `io::ErrorKind::NotADirectory` is newer
/// than the workspace MSRV, so the directory cases use the raw codes.
#[inline]
fn python_exception_type(error: &io::Error) -> &'static str {
    match (error.kind(), error.raw_os_error()) {
        (io::ErrorKind::NotFound, _) => "FileNotFoundError",
        (io::ErrorKind::PermissionDenied, _) => "PermissionError",
        (_, Some(code)) if code == NOT_A_DIRECTORY_CODE => "NotADirectoryError",
        (_, Some(code)) if Some(code) == IS_A_DIRECTORY_CODE => "IsADirectoryError",
        _ => "OSError",
    }
}

/// `ENOTDIR` on POSIX, `ERROR_DIRECTORY` (267) on Windows.
#[cfg(windows)]
const NOT_A_DIRECTORY_CODE: i32 = 267;
#[cfg(not(windows))]
const NOT_A_DIRECTORY_CODE: i32 = 20;
/// `EISDIR` on POSIX; Windows has no equivalent `CreateProcess` code.
#[cfg(windows)]
const IS_A_DIRECTORY_CODE: Option<i32> = None;
#[cfg(not(windows))]
const IS_A_DIRECTORY_CODE: Option<i32> = Some(21);

/// POSIX: `[Errno N] <strerror>: '<program>'`; Windows: `[WinError N] <message>`.
fn python_error_text(error: &io::Error, program: &str) -> String {
    let Some(code) = error.raw_os_error() else {
        return error.to_string();
    };
    let message = os_message_without_suffix(error, code);
    if cfg!(windows) {
        format!("[WinError {code}] {message}")
    } else {
        format!(
            "[Errno {code}] {message}: {}",
            obs_cli::python_compat::python_repr_str(program)
        )
    }
}

/// Rust renders `<message> (os error N)`; CPython also drops the Windows trailing period.
#[inline]
fn os_message_without_suffix(error: &io::Error, code: i32) -> String {
    let rendered = error.to_string();
    let suffix = format!(" (os error {code})");
    let message = rendered.strip_suffix(&suffix).unwrap_or(&rendered);
    if cfg!(windows) {
        message.trim_end_matches(['.', ' ', '\r', '\n']).to_owned()
    } else {
        message.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_program_is_file_not_found() {
        let directory = tempfile::tempdir().unwrap();
        let failure = spawn_child(
            &["definitely-not-a-real-program-4f2c".to_owned()],
            directory.path(),
        )
        .unwrap_err();

        assert_eq!(failure.exception_type, "FileNotFoundError");
        assert!(!failure.message.contains("os error"));
    }

    #[test]
    fn empty_command_matches_python_index_error() {
        let failure = spawn_child(&[], Path::new(".")).unwrap_err();
        assert_eq!(failure.exception_type, "IndexError");
    }

    #[cfg(unix)]
    #[test]
    fn signalled_child_reports_negative_returncode() {
        let status = Command::new("sh")
            .args(["-c", "kill -9 $$"])
            .status()
            .unwrap();
        assert_eq!(python_returncode(status), -9);
    }
}
