//! receiver.py:57 `_service_prefixes` / `adapter_for`: resource `service.name` → adapter by prefix.
//!
//! The prefixes are embedded instead of parsed from `adapters/*/manifest.toml` at runtime (no TOML
//! dependency in this crate); `tests/otlp_adapter_manifests.rs` fails when the manifests drift.
//! Order is Python's: manifests sorted by path (`claude` before `codex`), first match wins.

use serde_json::Value;

use super::otlp_decoding::OtelAttributes;
use super::{claude_usage, codex_usage};

/// An agent whose adapter has an `otel.map`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageAdapter {
    Claude,
    Codex,
}

/// `(otel_service_names prefix, manifest agent)` pairs.
pub const OTEL_SERVICE_PREFIXES: [(&str, UsageAdapter); 2] = [
    ("claude-code", UsageAdapter::Claude),
    ("codex", UsageAdapter::Codex),
];

impl UsageAdapter {
    /// The manifest's `agent` value.
    pub fn agent_name(self) -> &'static str {
        match self {
            UsageAdapter::Claude => "claude",
            UsageAdapter::Codex => "codex",
        }
    }

    /// `adapters.<agent>.otel.map(attrs)`.
    #[inline]
    pub fn map_attributes(self, attributes: &OtelAttributes) -> Option<Value> {
        match self {
            UsageAdapter::Claude => claude_usage::map_api_request(attributes),
            UsageAdapter::Codex => codex_usage::map_response_completed(attributes),
        }
    }
}

/// `adapter_for(service_name)`.
pub fn adapter_for(service_name: &str) -> Option<UsageAdapter> {
    OTEL_SERVICE_PREFIXES
        .iter()
        .find(|(prefix, _)| service_name.starts_with(*prefix))
        .map(|(_, adapter)| *adapter)
}
