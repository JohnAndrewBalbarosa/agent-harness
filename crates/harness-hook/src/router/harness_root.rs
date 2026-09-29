//! Where the harness lives. Python derived it from its own file (`Path(__file__).parents[2]`); the Rust
//! binary is installed as `<root>/bin/harness-hook.exe`, so the root is the exe directory's parent.
//! `AGENT_HARNESS_HOME` overrides both (tests, relocated installs).

use std::path::PathBuf;

pub const HARNESS_HOME_ENV: &str = "AGENT_HARNESS_HOME";

pub fn resolve_harness_root() -> PathBuf {
    explicit_root()
        .or_else(root_from_exe_location)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn explicit_root() -> Option<PathBuf> {
    std::env::var_os(HARNESS_HOME_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// `<root>/bin/harness-hook.exe` → `<root>`.
fn root_from_exe_location() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let bin_directory = exe.parent()?;
    bin_directory.parent().map(PathBuf::from)
}
