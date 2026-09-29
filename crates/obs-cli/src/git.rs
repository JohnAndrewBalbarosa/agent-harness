//! obs.py:336 `git_value`: one git query in a repository root; any failure yields "".

use std::path::Path;
use std::process::{Command, Stdio};

/// Windows `CREATE_NO_WINDOW`, so hooks and consoles never flash a git window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn git_value(root: &Path, arguments: &[&str]) -> String {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .args(arguments)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    hide_console_window(&mut command);

    match command.output() {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        }
        _ => String::new(),
    }
}

#[cfg(windows)]
fn hide_console_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console_window(_command: &mut Command) {}
