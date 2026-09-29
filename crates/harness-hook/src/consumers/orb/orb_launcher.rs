//! `launch_orb()` (`status_common.py`): start `orb.py` fully detached on every `SessionStart`
//! (the orb itself is single-instance; `ORB_UI=0` only makes it headless).
//!
//! Interpreter: `.venv/Scripts/pythonw.exe`, else the venv `python.exe` the Python hook itself ran under
//! (`Path(sys.executable).with_name("pythonw.exe")` resolves to the same venv).

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::orb_log::OrbLog;
use super::{orb_python, orb_tool_root};

/// Windows `CREATE_NO_WINDOW | DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`.
pub const DETACHED_FLAGS: u32 = 0x0800_0000 | 0x0000_0008 | 0x0000_0200;

pub fn launch_orb(harness_root: &Path, env: &BTreeMap<String, String>, log: &OrbLog) {
    let tool_root = orb_tool_root(harness_root);
    let interpreter = orb_interpreter(harness_root);
    if let Err(error) = spawn_detached(&interpreter, &tool_root.join("orb.py"), &tool_root, env) {
        log.info(&format!("orb-launch-error {error}"));
    }
}

/// `pythonw.exe` when present, else the venv `python.exe`.
pub fn orb_interpreter(harness_root: &Path) -> PathBuf {
    let windowless = orb_tool_root(harness_root)
        .join(".venv")
        .join("Scripts")
        .join("pythonw.exe");
    if windowless.exists() {
        windowless
    } else {
        orb_python(harness_root)
    }
}

// The orb outlives the hook by design: it is never waited on.
#[allow(clippy::zombie_processes)]
fn spawn_detached(
    interpreter: &Path,
    script: &Path,
    cwd: &Path,
    env: &BTreeMap<String, String>,
) -> io::Result<()> {
    let mut command = Command::new(interpreter);
    command
        .arg(script)
        .current_dir(cwd)
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    detach(&mut command);
    command.spawn().map(|_child| ())
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
