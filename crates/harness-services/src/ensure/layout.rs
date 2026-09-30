//! Where `ensure` looks (bootstrap.py `Layout` / `default_layout`) and the shared services log path.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const DEFAULT_OBS_ENDPOINT: &str = "http://127.0.0.1:4319";
const ORB_HEALTH_DIRECTORY: &str = "CodexStatusOrb";
const ORB_HEALTH_FILE: &str = "health.json";
const SERVICES_BINARY_STEM: &str = "harness-services";
const HUB_BINARY_STEM: &str = "obs-hub";
const SERVICES_LOG_FILE: &str = "services.lifecycle.jsonl";

/// bootstrap.py `Layout`.
#[derive(Debug, Clone)]
pub struct EnsureLayout {
    pub root: PathBuf,
    pub orb_health: PathBuf,
    pub obs_endpoint: String,
    /// `ORB_UI=0`: the orb runs headless (same logic and sounds, no window or tray).
    pub orb_ui: bool,
}

impl EnsureLayout {
    /// bootstrap.py `default_layout(env)`.
    pub fn from_env(root: &Path, env: &BTreeMap<String, String>) -> Self {
        let local_app_data = non_empty(env, "LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_directory(env));
        Self {
            root: root.to_path_buf(),
            orb_health: local_app_data
                .join(ORB_HEALTH_DIRECTORY)
                .join(ORB_HEALTH_FILE),
            obs_endpoint: non_empty(env, "OBS_ENDPOINT")
                .unwrap_or(DEFAULT_OBS_ENDPOINT)
                .to_owned(),
            orb_ui: env.get("ORB_UI").map(|value| value.trim()) != Some("0"),
        }
    }

    /// `<root>/var/logs/services.lifecycle.jsonl` (bootstrap.py `Layout.log`).
    pub fn log(&self) -> PathBuf {
        services_log_path(&self.root)
    }
}

/// bootstrap.py writes `<root>/var/logs/services.lifecycle.jsonl` regardless of `AH_LOG_DIR`; the daemon and
/// the spool drain log to the same file.
pub fn services_log_path(root: &Path) -> PathBuf {
    root.join("var").join("logs").join(SERVICES_LOG_FILE)
}

/// `<root>/bin/harness-services(.exe)`: the daemon `ensure` starts.
pub fn services_binary(root: &Path) -> PathBuf {
    bin_executable(root, SERVICES_BINARY_STEM)
}

/// bootstrap.py `native_hub_binary`: `<root>/bin/obs-hub(.exe)`.
pub fn native_hub_binary(root: &Path) -> PathBuf {
    bin_executable(root, HUB_BINARY_STEM)
}

#[inline]
fn bin_executable(root: &Path, stem: &str) -> PathBuf {
    root.join("bin")
        .join(format!("{stem}{}", std::env::consts::EXE_SUFFIX))
}

#[inline]
fn non_empty<'a>(env: &'a BTreeMap<String, String>, key: &str) -> Option<&'a str> {
    env.get(key)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
}

/// Python `Path.home()`: `USERPROFILE` on Windows, `HOME` elsewhere.
#[inline]
fn home_directory(env: &BTreeMap<String, String>) -> PathBuf {
    non_empty(env, "USERPROFILE")
        .or_else(|| non_empty(env, "HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}
