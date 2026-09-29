//! Notification delivery backends (`core/notify/deliver.py`). Never fails; returns a result string for
//! the notify log.
//!
//! Inside Herdr: Herdr's notification API shows its in-app toast and plays the configured done/request
//! bell, and the Windows pop-up (kept in Notification Center) is silent. Outside Herdr, or when Herdr
//! does not show it: a Windows pop-up with sound, through the unchanged `core/notify/toast.ps1`.

use std::collections::BTreeMap;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use super::decide::Sound;
use crate::events::{is_python_truthy, python_str};

pub const TIMEOUT: Duration = Duration::from_secs(15);
const WAIT_POLL: Duration = Duration::from_millis(10);
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// What a finished command produced (`subprocess.CompletedProcess`, stdout only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
}

/// A launch/wait failure named like the Python exception `type(error).__name__` it replaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaunchFailure {
    pub python_name: &'static str,
}

impl LaunchFailure {
    pub const OS_ERROR: Self = Self {
        python_name: "OSError",
    };
    pub const TIMEOUT_EXPIRED: Self = Self {
        python_name: "TimeoutExpired",
    };

    fn from_io(error: &io::Error) -> Self {
        let python_name = match error.kind() {
            io::ErrorKind::NotFound => "FileNotFoundError",
            io::ErrorKind::PermissionDenied => "PermissionError",
            _ => "OSError",
        };
        Self { python_name }
    }
}

/// `subprocess.run(args, capture_output=True, timeout=..., creationflags=CREATE_NO_WINDOW)`.
pub trait CommandRunner: Send + Sync {
    fn run(&self, args: &[String], timeout: Duration) -> Result<CommandOutput, LaunchFailure>;
}

/// One toast to show.
#[derive(Debug, Clone, Copy)]
pub struct ToastRequest<'a> {
    pub title: &'a str,
    pub message: &'a str,
    pub sound: Sound,
}

/// `deliver(decision, title, herdr_bin)`.
pub fn deliver(
    request: &ToastRequest<'_>,
    herdr_bin: Option<&Path>,
    runner: &dyn CommandRunner,
    toast_script: &Path,
) -> String {
    // Mental model: no Herdr → one sounded Windows toast; Herdr → Herdr bell first, then a Windows toast
    // that is silent only when Herdr actually played the bell.
    let Some(herdr_bin) = herdr_bin else {
        return windows_toast(request, false, runner, toast_script);
    };
    let (herdr_result, bell_played) = herdr_notification(herdr_bin, request, runner);
    let windows_result = windows_toast(request, bell_played, runner, toast_script);
    format!("{herdr_result} + {windows_result}")
}

/// `core/notify/toast.ps1` beside the harness root (`deliver.TOAST_SCRIPT`).
pub fn toast_script_path(harness_root: &Path) -> PathBuf {
    harness_root.join("core").join("notify").join("toast.ps1")
}

/// `consumer._herdr_bin(env)`: the Herdr CLI when this agent runs inside a Herdr pane and it exists.
pub fn herdr_binary(env: &BTreeMap<String, String>) -> Option<PathBuf> {
    let in_herdr = env.get("HERDR_ENV").map(String::as_str) == Some("1")
        && env
            .get("HERDR_PANE_ID")
            .is_some_and(|pane| !pane.is_empty());
    if !in_herdr {
        return None;
    }
    let candidate = match env.get("HERDR_BIN_PATH").filter(|path| !path.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(
            env.get("LOCALAPPDATA")
                .map(String::as_str)
                .unwrap_or_default(),
        )
        .join("Programs")
        .join("Herdr")
        .join("bin")
        .join("herdr.exe"),
    };
    candidate.exists().then_some(candidate)
}

/// `_windows(title, message, silent, run, script)`.
fn windows_toast(
    request: &ToastRequest<'_>,
    silent: bool,
    runner: &dyn CommandRunner,
    toast_script: &Path,
) -> String {
    let mut args: Vec<String> = [
        "powershell",
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
    ]
    .iter()
    .map(|arg| (*arg).to_owned())
    .collect();
    args.push(toast_script.display().to_string());
    args.extend([
        "-Title".to_owned(),
        request.title.to_owned(),
        "-Message".to_owned(),
        request.message.to_owned(),
    ]);
    if silent {
        args.push("-Silent".to_owned());
    }

    match runner.run(&args, TIMEOUT) {
        Err(failure) => format!("windows:error({})", failure.python_name),
        Ok(done) if done.exit_code != 0 => format!("windows:error(exit {})", done.exit_code),
        Ok(_) if silent => "windows:handed(silent)".to_owned(),
        Ok(_) => "windows:handed".to_owned(),
    }
}

/// `_herdr(...)`: `(result text, bell played)`; `result.shown` decides whether Herdr handled the sound.
fn herdr_notification(
    herdr_bin: &Path,
    request: &ToastRequest<'_>,
    runner: &dyn CommandRunner,
) -> (String, bool) {
    let args = vec![
        herdr_bin.display().to_string(),
        "notification".to_owned(),
        "show".to_owned(),
        request.title.to_owned(),
        "--body".to_owned(),
        request.message.to_owned(),
        "--sound".to_owned(),
        request.sound.as_str().to_owned(),
    ];
    let result = runner
        .run(&args, TIMEOUT)
        .and_then(|done| herdr_result_object(&done.stdout));
    let result = match result {
        Ok(result) => result,
        Err(failure) => return (format!("herdr:error({})", failure.python_name), false),
    };
    if result.get("shown").is_some_and(is_python_truthy) {
        return (format!("herdr:{}", reason_or(&result, "shown")), true);
    }
    (
        format!("herdr:not-shown({})", reason_or(&result, "unknown")),
        false,
    )
}

/// `json.loads(stdout or b"{}").get("result") or {}`.
#[inline]
fn herdr_result_object(stdout: &[u8]) -> Result<Map<String, Value>, LaunchFailure> {
    let text: &[u8] = if stdout.is_empty() { b"{}" } else { stdout };
    let document: Value = serde_json::from_slice(text).map_err(|_| LaunchFailure {
        python_name: "JSONDecodeError",
    })?;
    Ok(document
        .get("result")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default())
}

/// `result.get('reason', default)`.
#[inline]
fn reason_or(result: &Map<String, Value>, default: &str) -> String {
    result
        .get("reason")
        .map_or_else(|| default.to_owned(), python_str)
}

/// Runs real processes: hidden console, stdout captured on a reader thread, killed at the timeout.
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&self, args: &[String], timeout: Duration) -> Result<CommandOutput, LaunchFailure> {
        let (program, arguments) = args.split_first().ok_or(LaunchFailure::OS_ERROR)?;
        let mut command = Command::new(program);
        command
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        hide_console_window(&mut command);

        let mut child = command
            .spawn()
            .map_err(|error| LaunchFailure::from_io(&error))?;
        let stdout_reader = child.stdout.take().map(|mut pipe| {
            thread::spawn(move || {
                let mut captured = Vec::new();
                let _ = pipe.read_to_end(&mut captured);
                captured
            })
        });

        let status = wait_until(&mut child, Instant::now() + timeout)?;
        let stdout = stdout_reader
            .and_then(|reader| reader.join().ok())
            .unwrap_or_default();
        Ok(CommandOutput {
            exit_code: status.code().unwrap_or(-1),
            stdout,
        })
    }
}

/// Polls the child until it exits; past `deadline` it is killed (`subprocess.TimeoutExpired`).
fn wait_until(child: &mut Child, deadline: Instant) -> Result<ExitStatus, LaunchFailure> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(error) => return Err(LaunchFailure::from_io(&error)),
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(LaunchFailure::TIMEOUT_EXPIRED);
        }
        thread::sleep(WAIT_POLL);
    }
}

#[cfg(windows)]
#[inline]
fn hide_console_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
#[inline]
fn hide_console_window(_command: &mut Command) {}
