//! `services` consumer (`core/services/consumer.py`): on `session.start`, detach the service bootstrap
//! (cwd = harness root) and return at once:
//! - `<root>/bin/harness-services.exe ensure` when the Rust services binary is installed;
//! - else `python -m core.services.bootstrap` (the Python fallback until the cutover).
//!
//! Python interpreter, in `harness-hook.cmd` order: `HARNESS_PYTHON`, else the first line of
//! `<root>/var/python.path`, else the pinned uv CPython under `%APPDATA%`; a path that does not exist falls
//! back to `python` on PATH.
//! Detachment: `CREATE_NO_WINDOW | DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP` on Windows, null stdio.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::consumer::{Consumer, ConsumerResult};
use crate::hook_context::HookContext;
use crate::runtime_env::with_python_child_conventions;

pub const NAME: &str = "services";
pub const TIMEOUT: Duration = Duration::from_secs(10);
pub const BOOTSTRAP_MODULE: &str = "core.services.bootstrap";
const SERVICES_BINARY_STEM: &str = "harness-services";
const ENSURE_MODE: &str = "ensure";
const PINNED_UV_PYTHON: &str = r"uv\python\cpython-3.14.7-windows-x86_64-none\python.exe";
const PATH_PYTHON: &str = "python";

#[cfg(windows)]
const DETACHED_FLAGS: u32 = CREATE_NO_WINDOW | DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const DETACHED_PROCESS: u32 = 0x0000_0008;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

pub struct ServicesConsumer;

impl Consumer for ServicesConsumer {
    fn name(&self) -> &'static str {
        NAME
    }

    fn subscribes_to(&self, event: &str) -> bool {
        event == "session.start"
    }

    fn timeout(&self) -> Duration {
        TIMEOUT
    }

    /// Python returns 1 when `Popen` raises `OSError`, else 0.
    fn run(&self, context: &HookContext) -> anyhow::Result<ConsumerResult> {
        match launch_bootstrap(context) {
            Ok(()) => Ok(ConsumerResult::succeeded(NAME, "")),
            Err(_) => Ok(ConsumerResult::failed(NAME, 1)),
        }
    }
}

// Mental model: native `harness-services ensure` when installed → else the Python bootstrap module.
/// Detaches whichever bootstrap is installed; never waits for it.
pub fn launch_bootstrap(context: &HookContext) -> std::io::Result<()> {
    let native = native_services_binary(&context.harness_root);
    if native.is_file() {
        return launch_native_bootstrap(context, &native);
    }
    launch_python_bootstrap(context)
}

/// `<root>/bin/harness-services(.exe)`, installed by `install.ps1` next to `harness-hook`.
#[inline]
pub fn native_services_binary(harness_root: &Path) -> PathBuf {
    harness_root.join("bin").join(format!(
        "{SERVICES_BINARY_STEM}{}",
        std::env::consts::EXE_SUFFIX
    ))
}

/// `harness-services ensure` with the hook's merged runtime environment.
fn launch_native_bootstrap(context: &HookContext, binary: &Path) -> std::io::Result<()> {
    let mut command = Command::new(binary);
    command
        .arg(ENSURE_MODE)
        .current_dir(&context.harness_root)
        .env_clear()
        .envs(&context.env)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    detach(&mut command);

    let bootstrap = command.spawn()?;
    leave_running(bootstrap);
    Ok(())
}

/// `launch_detached([sys.executable, "-m", "core.services.bootstrap"], ROOT)`.
fn launch_python_bootstrap(context: &HookContext) -> std::io::Result<()> {
    let python = harness_python(context);
    let mut command = Command::new(python);
    command
        .args(["-m", BOOTSTRAP_MODULE])
        .current_dir(&context.harness_root)
        .env_clear()
        .envs(with_python_child_conventions(
            &context.env,
            &context.harness_root,
        ))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    detach(&mut command);

    let bootstrap = command.spawn()?;
    leave_running(bootstrap);
    Ok(())
}

/// The bootstrap outlives this hook on purpose (Python dropped its `Popen` the same way); never waited.
#[inline]
fn leave_running(_bootstrap: std::process::Child) {}

// Mental model: first configured candidate wins; the cmd's `if not exist → python` applies to it.
/// The interpreter `harness-hook.cmd` would have run the consumer with (`sys.executable`).
pub fn harness_python(context: &HookContext) -> PathBuf {
    let configured = context
        .env_value("HARNESS_PYTHON")
        .map(PathBuf::from)
        .or_else(|| recorded_python(&context.harness_root))
        .or_else(|| {
            context
                .env_value("APPDATA")
                .map(|appdata| Path::new(appdata).join(PINNED_UV_PYTHON))
        });

    match configured {
        Some(path) if path.exists() => path,
        _ => PathBuf::from(PATH_PYTHON),
    }
}

/// `set /p HARNESS_PYTHON=<"<root>\var\python.path"`: the first line, without its line break.
fn recorded_python(harness_root: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(harness_root.join("var").join("python.path")).ok()?;
    let first_line = text.lines().next()?.trim_end_matches('\r');
    (!first_line.is_empty()).then(|| PathBuf::from(first_line))
}

#[cfg(windows)]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(DETACHED_FLAGS);
}

#[cfg(not(windows))]
fn detach(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::make_event;
    use serde_json::{Map, Value};

    fn context(root: &Path, env: &[(&str, &str)]) -> HookContext {
        HookContext {
            event: make_event(
                "session.start",
                "claude",
                "default",
                &Map::new(),
                "SessionStart",
                Map::new(),
            ),
            native_payload: Value::Object(Map::new()),
            native_bytes: Vec::new(),
            env: env
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect(),
            harness_root: root.to_path_buf(),
        }
    }

    #[test]
    fn runs_on_session_start_only() {
        assert!(ServicesConsumer.subscribes_to("session.start"));
        assert!(!ServicesConsumer.subscribes_to("turn.stop"));
    }

    #[test]
    fn python_path_file_is_used_when_it_exists() {
        let dir = tempfile::tempdir().unwrap();
        let python = dir.path().join("python-real");
        std::fs::write(&python, b"").unwrap();
        std::fs::create_dir_all(dir.path().join("var")).unwrap();
        std::fs::write(
            dir.path().join("var").join("python.path"),
            format!("{}\r\n", python.display()),
        )
        .unwrap();
        assert_eq!(harness_python(&context(dir.path(), &[])), python);
    }

    #[test]
    fn native_binary_lives_in_the_root_bin_directory() {
        let root = Path::new("/h");
        assert_eq!(
            native_services_binary(root),
            root.join("bin")
                .join(format!("harness-services{}", std::env::consts::EXE_SUFFIX))
        );
    }

    #[test]
    fn missing_interpreters_fall_back_to_path_python() {
        let dir = tempfile::tempdir().unwrap();
        let env = [("HARNESS_PYTHON", "/definitely/missing/python.exe")];
        assert_eq!(
            harness_python(&context(dir.path(), &env)),
            PathBuf::from("python")
        );
    }
}
