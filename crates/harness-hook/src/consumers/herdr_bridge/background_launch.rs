//! Detached helper processes the bridge starts and never waits for: the Herdr event subscriber
//! (`hook.py` `ensure_subscriber`, still `subscriber.py` until sub-project #3; it is single-instance itself)
//! and the spool flusher.
//!
//! Python interpreter: the one `harness-hook.cmd` would use (`HARNESS_PYTHON`, else the first line of
//! `var/python.path`, else the pinned uv CPython, else `python` from `PATH`), because the Python bridge ran the
//! subscriber with its own `sys.executable`.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::json;

use super::bridge_log::BridgeLog;
use super::herdr_cli::python_os_error_name;

/// Windows `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW`.
#[cfg_attr(not(windows), allow(dead_code))]
const DETACHED_FLAGS: u32 = 0x0000_0008 | 0x0000_0200 | 0x0800_0000;
const PINNED_UV_PYTHON: [&str; 4] = [
    "uv",
    "python",
    "cpython-3.14.7-windows-x86_64-none",
    "python.exe",
];

/// `ensure_subscriber()`: only inside Herdr with a socket to subscribe to.
pub fn ensure_subscriber(harness_root: &Path, env: &BTreeMap<String, String>, log: &BridgeLog) {
    let is_inside_herdr = env.get("HERDR_ENV").map(String::as_str) == Some("1");
    let has_socket = env
        .get("HERDR_SOCKET_PATH")
        .is_some_and(|value| !value.is_empty());
    if !is_inside_herdr || !has_socket {
        return;
    }
    let script = harness_root
        .join("tools")
        .join("herdr-bridge")
        .join("subscriber.py");
    let mut child_env = env.clone();
    child_env.insert("PYTHONUTF8".to_owned(), "1".to_owned());

    match spawn_detached(
        &harness_python(harness_root, env),
        &[script.display().to_string()],
        harness_root,
        &child_env,
    ) {
        Ok(process_id) => log.event(
            "subscriber.process.started",
            "started",
            vec![
                ("processId", json!(process_id)),
                ("windowMode", json!("hidden")),
            ],
        ),
        Err(error) => log.event(
            "subscriber.launch",
            "failed",
            vec![("error", json!(python_os_error_name(&error)))],
        ),
    }
}

/// `harness-hook.cmd`'s interpreter resolution.
pub fn harness_python(harness_root: &Path, env: &BTreeMap<String, String>) -> PathBuf {
    let candidate = env
        .get("HARNESS_PYTHON")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| recorded_python(harness_root))
        .unwrap_or_else(|| pinned_uv_python(env));
    if candidate.exists() {
        candidate
    } else {
        PathBuf::from("python")
    }
}

/// `set /p HARNESS_PYTHON=<var\python.path` (first line).
#[inline]
fn recorded_python(harness_root: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(harness_root.join("var").join("python.path")).ok()?;
    let first_line = text.lines().next()?.trim_end_matches('\r');
    (!first_line.is_empty()).then(|| PathBuf::from(first_line))
}

#[inline]
fn pinned_uv_python(env: &BTreeMap<String, String>) -> PathBuf {
    let app_data = PathBuf::from(env.get("APPDATA").map(String::as_str).unwrap_or_default());
    PINNED_UV_PYTHON
        .iter()
        .fold(app_data, |path, part| path.join(part))
}

/// Starts `program args...` detached with stdio closed; returns its pid. Never waited on by design.
#[allow(clippy::zombie_processes)]
pub fn spawn_detached(
    program: &Path,
    args: &[String],
    cwd: &Path,
    env: &BTreeMap<String, String>,
) -> io::Result<u32> {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    detach(&mut command);
    command.spawn().map(|child| child.id())
}

#[cfg(windows)]
#[inline]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(DETACHED_FLAGS);
}

#[cfg(not(windows))]
#[inline]
fn detach(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorded_python_path_is_used_when_it_exists() {
        let dir = tempfile::tempdir().unwrap();
        let python = dir.path().join("py.exe");
        std::fs::write(&python, b"").unwrap();
        std::fs::create_dir_all(dir.path().join("var")).unwrap();
        std::fs::write(
            dir.path().join("var").join("python.path"),
            format!("{}\r\nextra\r\n", python.display()),
        )
        .unwrap();
        assert_eq!(harness_python(dir.path(), &BTreeMap::new()), python);
    }

    #[test]
    fn missing_interpreters_fall_back_to_path_python() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            harness_python(dir.path(), &BTreeMap::new()),
            PathBuf::from("python")
        );
    }

    #[test]
    fn subscriber_is_not_started_without_a_socket() {
        let dir = tempfile::tempdir().unwrap();
        let log = BridgeLog::under_root(dir.path());
        let env = BTreeMap::from([("HERDR_ENV".to_owned(), "1".to_owned())]);
        ensure_subscriber(dir.path(), &env, &log);
        assert!(!log.path().exists());
    }
}
