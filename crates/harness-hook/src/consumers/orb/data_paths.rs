//! Where the orb keeps its state (`status_common.py:18-24`):
//! `DATA_ROOT = $CODEX_STATUS_ORB_DATA` else `$LOCALAPPDATA/CodexStatusOrb` else `~/CodexStatusOrb`.

use std::collections::BTreeMap;
use std::path::PathBuf;

const DATA_ENV: &str = "CODEX_STATUS_ORB_DATA";
const DATA_DIR_NAME: &str = "CodexStatusOrb";
const LOG_FILE_NAME: &str = "status-orb.log";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrbDataPaths {
    data_root: PathBuf,
}

impl OrbDataPaths {
    pub fn new(data_root: PathBuf) -> Self {
        Self { data_root }
    }

    /// Python `os.environ.get(...)` semantics: a variable that is set (even empty) wins.
    pub fn from_env(env: &BTreeMap<String, String>) -> Self {
        let data_root = env
            .get(DATA_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| local_app_data(env).join(DATA_DIR_NAME));
        Self { data_root }
    }

    pub fn data_root(&self) -> &PathBuf {
        &self.data_root
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.data_root.join("sessions")
    }

    pub fn log_path(&self) -> PathBuf {
        self.data_root.join(LOG_FILE_NAME)
    }
}

/// `Path(os.environ.get("LOCALAPPDATA", Path.home()))`.
#[inline]
fn local_app_data(env: &BTreeMap<String, String>) -> PathBuf {
    env.get("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| user_home(env))
}

/// `Path.home()`: `USERPROFILE` on Windows, `HOME` elsewhere.
#[inline]
fn user_home(env: &BTreeMap<String, String>) -> PathBuf {
    let preferred = if cfg!(windows) {
        ["USERPROFILE", "HOME"]
    } else {
        ["HOME", "USERPROFILE"]
    };
    preferred
        .iter()
        .find_map(|key| env.get(*key).filter(|value| !value.is_empty()))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_data_root_wins_over_local_app_data() {
        let env = BTreeMap::from([
            (DATA_ENV.to_owned(), "/data/orb".to_owned()),
            ("LOCALAPPDATA".to_owned(), "/local".to_owned()),
        ]);
        let paths = OrbDataPaths::from_env(&env);
        assert_eq!(
            paths.sessions_dir(),
            PathBuf::from("/data/orb").join("sessions")
        );
    }

    #[test]
    fn local_app_data_hosts_the_default_root() {
        let env = BTreeMap::from([("LOCALAPPDATA".to_owned(), "/local".to_owned())]);
        assert_eq!(
            OrbDataPaths::from_env(&env).log_path(),
            PathBuf::from("/local")
                .join("CodexStatusOrb")
                .join("status-orb.log")
        );
    }
}
