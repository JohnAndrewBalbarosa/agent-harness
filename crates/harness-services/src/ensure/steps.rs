//! The individual `ensure` steps (bootstrap.py `ensure_orb`, `ensure_hub`, and the daemon that replaced
//! `receiver.ensure` / `ensure_console_monitor`). Each returns bootstrap.py's `result` string.

use std::cell::Cell;
use std::ffi::OsString;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use super::layout::{native_hub_binary, services_binary, EnsureLayout};
use super::probes::ServiceProbes;

/// bootstrap.py `ORB_HEALTH_MAX_AGE_S`.
pub const ORB_HEALTH_MAX_AGE_SECONDS: f64 = 15.0;
const DAEMON_MODE: &str = "serve";
const HUB_MODE: &str = "serve";
const ORB_HEADLESS_FLAG: &str = "--headless";

pub const NOT_INSTALLED: &str = "not_installed";
pub const ALREADY_RUNNING: &str = "already_running";
pub const REQUESTED: &str = "requested";
pub const REQUESTED_HEADLESS: &str = "requested_headless";

/// bootstrap.py `ensure_orb`: start `orb.py` when its venv exists and `health.json` is not fresh + healthy.
pub fn ensure_orb(
    layout: &EnsureLayout,
    probes: &dyn ServiceProbes,
) -> std::io::Result<&'static str> {
    let orb = layout.root.join("tools").join("codex-status-orb");
    let pythonw = orb.join(".venv").join("Scripts").join("pythonw.exe");
    if !pythonw.exists() {
        return Ok(NOT_INSTALLED);
    }
    if is_orb_healthy(&layout.orb_health) {
        return Ok(ALREADY_RUNNING);
    }
    let script = orb.join("orb.py");
    if !layout.orb_ui {
        let command: [OsString; 3] = [pythonw.into(), script.into(), ORB_HEADLESS_FLAG.into()];
        probes.launch(&command, &orb)?;
        return Ok(REQUESTED_HEADLESS);
    }
    let command: [OsString; 2] = [pythonw.into(), script.into()];
    probes.launch(&command, &orb)?;
    Ok(REQUESTED)
}

/// bootstrap.py `ensure_hub`: start `bin/obs-hub serve` (cwd `bin/`) when installed and `/health` fails.
pub fn ensure_hub(
    layout: &EnsureLayout,
    probes: &dyn ServiceProbes,
) -> std::io::Result<&'static str> {
    let hub = native_hub_binary(&layout.root);
    if !hub.exists() {
        return Ok(NOT_INSTALLED);
    }
    let health_url = format!("{}/health", layout.obs_endpoint.trim_end_matches('/'));
    if probes.http_healthy(&health_url) {
        return Ok(ALREADY_RUNNING);
    }
    let working_directory = hub.parent().unwrap_or(&layout.root).to_path_buf();
    let command: [OsString; 2] = [hub.into(), HUB_MODE.into()];
    probes.launch(&command, &working_directory)?;
    Ok(REQUESTED)
}

/// `receiver.ensure` and `console-monitor.ensure` both mean "the `serve` daemon runs": the first step that
/// succeeds decides, the second reuses its answer instead of launching a second daemon.
pub struct DaemonStep<'a> {
    layout: &'a EnsureLayout,
    probes: &'a dyn ServiceProbes,
    settled: Cell<Option<&'static str>>,
}

impl<'a> DaemonStep<'a> {
    pub fn new(layout: &'a EnsureLayout, probes: &'a dyn ServiceProbes) -> Self {
        Self {
            layout,
            probes,
            settled: Cell::new(None),
        }
    }

    pub fn ensure(&self) -> std::io::Result<&'static str> {
        if let Some(result) = self.settled.get() {
            return Ok(result);
        }
        let result = ensure_daemon(self.layout, self.probes)?;
        self.settled.set(Some(result));
        Ok(result)
    }
}

// Mental model: installed? → mutex held? → launch `harness-services serve` detached (cwd = root).
fn ensure_daemon(
    layout: &EnsureLayout,
    probes: &dyn ServiceProbes,
) -> std::io::Result<&'static str> {
    let binary = services_binary(&layout.root);
    if !binary.is_file() {
        return Ok(NOT_INSTALLED);
    }
    if probes.daemon_running() {
        return Ok(ALREADY_RUNNING);
    }
    let command: [OsString; 2] = [binary.into(), DAEMON_MODE.into()];
    probes.launch(&command, &layout.root)?;
    Ok(REQUESTED)
}

/// bootstrap.py `_orb_healthy`: `status == "healthy"` and `time.time() - float(updated_at or 0) < 15`;
/// any read/parse/type problem means "not healthy".
pub fn is_orb_healthy(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(health) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    let Some(fields) = health.as_object() else {
        return false;
    };
    let is_status_healthy = fields.get("status").and_then(Value::as_str) == Some("healthy");
    let Some(updated_at) = python_float(fields.get("updated_at")) else {
        return false;
    };
    is_status_healthy && unix_now_seconds() - updated_at < ORB_HEALTH_MAX_AGE_SECONDS
}

/// Python `float(value)` for the JSON values `health.get("updated_at", 0)` can return; `None` = TypeError/ValueError.
#[inline]
fn python_float(value: Option<&Value>) -> Option<f64> {
    match value {
        None => Some(0.0),
        Some(Value::Number(number)) => number.as_f64(),
        Some(Value::Bool(flag)) => Some(if *flag { 1.0 } else { 0.0 }),
        Some(Value::String(text)) => text.trim().parse::<f64>().ok(),
        Some(_) => None,
    }
}

#[inline]
fn unix_now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs_f64())
        .unwrap_or_default()
}
