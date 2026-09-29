//! Hub configuration from the same environment variables the Node hub read.

use std::env;
use std::path::PathBuf;

use anyhow::{Context, Result};

const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 4319;
const DEFAULT_LOG_LEVEL: &str = "info";

#[derive(Debug, Clone)]
pub struct HubConfig {
    pub host: String,
    pub port: u16,
    pub database_path: PathBuf,
    pub log_level: String,
}

impl HubConfig {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            host: env_or("OBS_HOST", DEFAULT_HOST),
            port: port_from_env()?,
            database_path: obs_common::default_database_path(),
            log_level: env_or("OBS_LOG_LEVEL", DEFAULT_LOG_LEVEL),
        })
    }
}

fn port_from_env() -> Result<u16> {
    match env::var("OBS_PORT") {
        Ok(raw) if !raw.trim().is_empty() => raw
            .trim()
            .parse()
            .with_context(|| format!("OBS_PORT must be a TCP port, got `{raw}`")),
        _ => Ok(DEFAULT_PORT),
    }
}

#[inline]
fn env_or(name: &str, fallback: &str) -> String {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| fallback.to_owned())
}
