//! What every mode reads: the harness root, the merged runtime environment, and derived paths.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Same resolution as `harness-hook`: `AGENT_HARNESS_HOME`, else the parent of the directory holding
/// this exe (installed at `<root>/bin/harness-services.exe`).
#[derive(Debug, Clone)]
pub struct ServiceContext {
    pub harness_root: PathBuf,
    /// Process environment merged with the `.env` runtime keys (process wins), as the hook computes it.
    pub env: BTreeMap<String, String>,
}

impl ServiceContext {
    pub fn from_process() -> Self {
        let harness_root = harness_hook::router::resolve_harness_root();
        let process = harness_hook::runtime_env::process_environment();
        let env = harness_hook::runtime_env::runtime_env(&harness_root.join(".env"), &process);
        Self { harness_root, env }
    }

    pub fn env_value(&self, key: &str) -> Option<&str> {
        self.env
            .get(key)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
    }

    pub fn var_dir(&self) -> PathBuf {
        self.harness_root.join("var")
    }

    pub fn log_dir(&self) -> PathBuf {
        self.env_value("AH_LOG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| self.var_dir().join("logs"))
    }

    pub fn bin_dir(&self) -> PathBuf {
        self.harness_root.join("bin")
    }

    /// `AH_OBS_SPOOL`, else `<root>/tools/observability-client/var/spool.sqlite3`.
    pub fn spool_path(&self) -> PathBuf {
        self.env_value("AH_OBS_SPOOL")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                self.harness_root
                    .join("tools")
                    .join("observability-client")
                    .join("var")
                    .join("spool.sqlite3")
            })
    }
}
