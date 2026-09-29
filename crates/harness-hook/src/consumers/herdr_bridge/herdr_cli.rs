//! The external `herdr` CLI (`hook.py` `run_herdr`, `common.py` `herdr_binary`).
//!
//! Each call: hidden window, stdin closed, stderr merged into stdout, decoded as UTF-8 with replacement and
//! universal newlines; on timeout the process is killed and exit code 124 is reported. Only the last
//! `output_limit` characters are kept. Every call is logged to the bridge lifecycle log.

use std::collections::BTreeMap;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::bridge_log::BridgeLog;
use super::identity::safe_id;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(4);
pub const DEFAULT_OUTPUT_LIMIT: usize = 1000;
pub const TIMEOUT_EXIT_CODE: i32 = 124;
const LAUNCH_ERROR_LIMIT: usize = 1000;
const POLL_INTERVAL: Duration = Duration::from_millis(10);
/// Windows `CREATE_NO_WINDOW`.
#[cfg_attr(not(windows), allow(dead_code))]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrOutput {
    pub exit_code: i32,
    pub output: String,
}

/// Seam for tests: production uses [`ProcessHerdrRunner`].
pub trait HerdrRunner {
    fn run(&self, args: &[String], timeout: Duration, output_limit: usize) -> HerdrOutput;
}

/// `herdr_binary()`: `HERDR_BIN_PATH` when it is a file, else `%LOCALAPPDATA%/Programs/Herdr/bin/herdr.exe`
/// when present, else `herdr` from `PATH`.
pub fn herdr_binary(env: &BTreeMap<String, String>) -> PathBuf {
    let inherited = env
        .get("HERDR_BIN_PATH")
        .map(|value| value.trim())
        .unwrap_or_default();
    if !inherited.is_empty() && PathBuf::from(inherited).is_file() {
        return PathBuf::from(inherited);
    }
    let local_app_data = env
        .get("LOCALAPPDATA")
        .map(String::as_str)
        .unwrap_or_default();
    let local = PathBuf::from(local_app_data)
        .join("Programs")
        .join("Herdr")
        .join("bin")
        .join("herdr.exe");
    if local.is_file() {
        local
    } else {
        PathBuf::from("herdr")
    }
}

pub struct ProcessHerdrRunner<'a> {
    binary: PathBuf,
    log: &'a BridgeLog,
}

impl<'a> ProcessHerdrRunner<'a> {
    pub fn new(binary: PathBuf, log: &'a BridgeLog) -> Self {
        Self { binary, log }
    }
}

impl HerdrRunner for ProcessHerdrRunner<'_> {
    // Mental model: spawn (log started) → wait up to `timeout` (kill → 124) → log exited → tail of output.
    fn run(&self, args: &[String], timeout: Duration, output_limit: usize) -> HerdrOutput {
        let started = Instant::now();
        let operation = safe_id(
            args.first()
                .map(|first| Value::from(first.as_str()))
                .as_ref(),
            "unknown",
        );

        let (mut child, reader) = match spawn_merged(&self.binary, args) {
            Ok(spawned) => spawned,
            Err(error) => return self.launch_failed(&operation, &error),
        };
        let process_id = child.id();
        self.log.event(
            "helper.process.started",
            "started",
            vec![
                ("operation", json!(operation)),
                ("processId", json!(process_id)),
                ("windowMode", json!("hidden")),
            ],
        );

        let status = wait_with_timeout(&mut child, timeout);
        let output = tail_chars(&universal_newlines(&collect_output(reader)), output_limit);
        let duration_ms = round_millis(started.elapsed());

        let Some(status) = status else {
            self.log.event(
                "helper.process.failed",
                "failed",
                vec![
                    ("operation", json!(operation)),
                    ("processId", json!(process_id)),
                    ("reason", json!("timeout")),
                    ("durationMs", json!(duration_ms)),
                    ("windowMode", json!("hidden")),
                ],
            );
            return HerdrOutput {
                exit_code: TIMEOUT_EXIT_CODE,
                output,
            };
        };

        let exit_code = status.code().unwrap_or(1);
        self.log.event(
            "helper.process.exited",
            if exit_code == 0 {
                "succeeded"
            } else {
                "failed"
            },
            vec![
                ("operation", json!(operation)),
                ("processId", json!(process_id)),
                ("exitCode", json!(exit_code)),
                ("durationMs", json!(duration_ms)),
                ("windowMode", json!("hidden")),
            ],
        );
        HerdrOutput { exit_code, output }
    }
}

impl ProcessHerdrRunner<'_> {
    /// Python's `except OSError`: exit code 1 and the error text.
    fn launch_failed(&self, operation: &str, error: &io::Error) -> HerdrOutput {
        self.log.event(
            "helper.process.failed",
            "failed",
            vec![
                ("operation", json!(operation)),
                ("reason", json!("launch_failed")),
                ("errorType", json!(python_os_error_name(error))),
                ("windowMode", json!("hidden")),
            ],
        );
        HerdrOutput {
            exit_code: 1,
            output: error.to_string().chars().take(LAUNCH_ERROR_LIMIT).collect(),
        }
    }
}

/// Spawns with stdout and stderr sharing one pipe (`stderr=subprocess.STDOUT`); the reader thread drains it.
fn spawn_merged(
    binary: &Path,
    args: &[String],
) -> io::Result<(Child, thread::JoinHandle<Vec<u8>>)> {
    let (mut pipe_reader, pipe_writer) = io::pipe()?;
    let stderr_writer = pipe_writer.try_clone()?;
    let mut command = Command::new(binary);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(pipe_writer)
        .stderr(stderr_writer);
    hide_window(&mut command);
    let child = command.spawn()?;
    // The command holds the parent's copies of the write end; dropping it lets the reader see EOF.
    drop(command);
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = pipe_reader.read_to_end(&mut bytes);
        bytes
    });
    Ok((child, reader))
}

/// `None` on timeout (after killing and reaping the child).
fn wait_with_timeout(child: &mut Child, timeout: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(POLL_INTERVAL),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

#[inline]
fn collect_output(reader: thread::JoinHandle<Vec<u8>>) -> String {
    let bytes = reader.join().unwrap_or_default();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Text-mode decoding: `\r\n` and lone `\r` become `\n`.
#[inline]
pub fn universal_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// Python `text[-limit:]` (code points).
#[inline]
pub fn tail_chars(text: &str, limit: usize) -> String {
    let count = text.chars().count();
    text.chars().skip(count.saturating_sub(limit)).collect()
}

/// `round(seconds * 1000, 3)`.
#[inline]
fn round_millis(elapsed: Duration) -> f64 {
    (elapsed.as_secs_f64() * 1_000_000.0).round() / 1000.0
}

/// `type(exc).__name__` for the `OSError` subclasses a failed launch raises.
#[inline]
pub(super) fn python_os_error_name(error: &io::Error) -> &'static str {
    match error.kind() {
        io::ErrorKind::NotFound => "FileNotFoundError",
        io::ErrorKind::PermissionDenied => "PermissionError",
        _ => "OSError",
    }
}

#[cfg(windows)]
#[inline]
fn hide_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
#[inline]
fn hide_window(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_keeps_the_last_code_points() {
        assert_eq!(tail_chars("héllo", 3), "llo");
        assert_eq!(tail_chars("hi", 10), "hi");
    }

    #[test]
    fn newlines_are_normalized_like_text_mode() {
        assert_eq!(universal_newlines("a\r\nb\rc\n"), "a\nb\nc\n");
    }

    #[test]
    fn missing_binary_falls_back_to_path_lookup() {
        let env = BTreeMap::from([(
            "HERDR_BIN_PATH".to_owned(),
            "/definitely/missing/herdr".to_owned(),
        )]);
        assert_eq!(herdr_binary(&env), PathBuf::from("herdr"));
    }
}
