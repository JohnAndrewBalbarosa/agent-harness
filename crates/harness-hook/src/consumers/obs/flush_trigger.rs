//! Replaces hook.py `schedule_flush()` and obs.py's `enqueue(flush_now=True)` (spec deviation: the hook
//! never delivers synchronously). Spawns ONE detached `<harness>/bin/obs-rs(.exe) flush` pointed at the
//! same spool; when obs-rs is not installed, falls back to hook.py's detached `flush_spool.py` exactly.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::spool_store::SpoolLocation;
use crate::hook_context::HookContext;

const HUB_DIRECTORY_ENV: &str = "OBS_HUB_DIR";
const PYTHON_ENV: &str = "HARNESS_PYTHON";
const DEFAULT_PYTHON: &str = "python";
const PYTHON_WORKER: &str = "flush_spool.py";

/// Mental model: prefer obs-rs → else Python worker → else nothing (neither installed, e.g. tests).
pub fn schedule(context: &HookContext, location: &SpoolLocation) {
    let command = obs_rs_flush(context, location).or_else(|| python_flush(context, location));
    if let Some(mut command) = command {
        detach(&mut command);
        // The child outlives this process; dropping the handle neither waits for nor kills it.
        let _ = command.spawn();
    }
}

fn obs_rs_flush(context: &HookContext, location: &SpoolLocation) -> Option<Command> {
    let executable = obs_rs_executable(&context.harness_root);
    if !executable.is_file() {
        return None;
    }
    let mut command = Command::new(executable);
    command
        .arg("flush")
        .envs(&context.env)
        .env(HUB_DIRECTORY_ENV, location.hub_dir())
        .current_dir(&location.client_dir);
    Some(command)
}

/// `[sys.executable, flush_spool.py]` with `cwd=worker.parent`.
fn python_flush(context: &HookContext, location: &SpoolLocation) -> Option<Command> {
    let worker = location.client_dir.join(PYTHON_WORKER);
    if !worker.is_file() {
        return None;
    }
    let mut command = Command::new(context.env_value(PYTHON_ENV).unwrap_or(DEFAULT_PYTHON));
    command
        .arg(worker)
        .envs(&context.env)
        .current_dir(&location.client_dir);
    Some(command)
}

#[inline]
fn obs_rs_executable(harness_root: &Path) -> PathBuf {
    harness_root
        .join("bin")
        .join(format!("obs-rs{}", std::env::consts::EXE_SUFFIX))
}

fn detach(command: &mut Command) {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    detach_from_console(command);
}

/// `CREATE_NO_WINDOW | DETACHED_PROCESS`.
#[cfg(windows)]
fn detach_from_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    command.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
}

#[cfg(not(windows))]
fn detach_from_console(_command: &mut Command) {}
