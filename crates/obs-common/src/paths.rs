//! Portable default locations. Every default can be overridden by an environment variable,
//! so the same binaries work on Windows, Linux, and macOS.

use std::env;
use std::path::PathBuf;

const HARNESS_HOME_ENV: &str = "AGENT_HARNESS_HOME";
const DATABASE_PATH_ENV: &str = "OBS_DB_PATH";
const SPOOL_PATH_ENV: &str = "AH_OBS_SPOOL";

/// Hub database: `$OBS_DB_PATH`, else `<harness home>/var/obs/hub.sqlite3`.
pub fn default_database_path() -> PathBuf {
    path_from_env(DATABASE_PATH_ENV)
        .unwrap_or_else(|| harness_home().join("var").join("obs").join("hub.sqlite3"))
}

/// Client spool shared with `obs.py`: `$AH_OBS_SPOOL`, else the harness observability client spool.
pub fn default_spool_path() -> PathBuf {
    path_from_env(SPOOL_PATH_ENV).unwrap_or_else(|| {
        harness_home()
            .join("tools")
            .join("observability-client")
            .join("var")
            .join("spool.sqlite3")
    })
}

/// `$AGENT_HARNESS_HOME`, else `~/.agent-harness`.
pub fn harness_home() -> PathBuf {
    path_from_env(HARNESS_HOME_ENV).unwrap_or_else(|| user_home().join(".agent-harness"))
}

fn user_home() -> PathBuf {
    path_from_env("HOME")
        .or_else(|| path_from_env("USERPROFILE"))
        .unwrap_or_else(|| PathBuf::from("."))
}

#[inline]
fn path_from_env(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}
