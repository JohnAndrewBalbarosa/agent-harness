//! What every in-process consumer implements (replaces one Python subprocess per consumer).

use std::time::Duration;

use crate::hook_context::HookContext;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(25);

/// Mirrors the Python `ConsumerResult`: only `context` and `herdr-bridge` stdout is ever used by adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumerResult {
    pub name: &'static str,
    pub exit_code: i32,
    pub stdout: String,
}

impl ConsumerResult {
    pub fn succeeded(name: &'static str, stdout: impl Into<String>) -> Self {
        Self {
            name,
            exit_code: 0,
            stdout: stdout.into(),
        }
    }

    pub fn failed(name: &'static str, exit_code: i32) -> Self {
        Self {
            name,
            exit_code,
            stdout: String::new(),
        }
    }
}

/// One consumer. Implementations must never panic outward on bad input (the router also catches panics)
/// and must keep every file they write byte-compatible with the Python consumer they replace.
pub trait Consumer: Send + Sync {
    fn name(&self) -> &'static str;

    /// Common event types this consumer runs for (`core/router/consumers.py`).
    fn subscribes_to(&self, event: &str) -> bool;

    /// Extra gating beyond the event type (e.g. herdr-bridge only inside Herdr, orb only when installed).
    fn is_enabled(&self, _context: &HookContext) -> bool {
        true
    }

    fn timeout(&self) -> Duration {
        DEFAULT_TIMEOUT
    }

    /// Runs the consumer; `Err` is logged and reported as exit code 1.
    fn run(&self, context: &HookContext) -> anyhow::Result<ConsumerResult>;
}
