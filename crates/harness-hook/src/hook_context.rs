//! Everything a consumer may read: the common event, the agent's raw payload, the merged runtime
//! environment, and the harness root (`HARNESS_HOME`, where `var/`, `tools/` and `policy/` live).

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::Value;

use crate::events::Event;

#[derive(Debug, Clone)]
pub struct HookContext {
    pub event: Event,
    /// The native payload exactly as parsed from stdin (never truncated, unlike `event.native_payload`).
    pub native_payload: Value,
    /// Raw stdin bytes (consumers that hash or re-emit the payload use these).
    pub native_bytes: Vec<u8>,
    /// Process environment merged with the `.env` runtime keys (process wins), see `runtime_env`.
    pub env: BTreeMap<String, String>,
    pub harness_root: PathBuf,
}

impl HookContext {
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
}
