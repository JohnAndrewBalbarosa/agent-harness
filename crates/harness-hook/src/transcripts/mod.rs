//! Agent transcript readers shared by the notify, usage-fallback and context-guard consumers
//! (`adapters/{claude,codex}/transcript.py`, `adapters/claude/background.py`, `core/jsonl.py`).
//!
//! Transcripts grow to hundreds of MiB, so every reader streams the file line by line and keeps
//! only running totals (never the whole entry list the Python readers materialize).
//!
//! - `jsonl_stream`       tolerant line-by-line JSONL visiting (blank and malformed lines skipped)
//! - `token_count`        `adapters.base.count`
//! - `iso_time`           `core.jsonl.parse_ts` / `datetime.fromisoformat`
//! - `claude`, `codex`    per-turn token usage and latest-request context size
//! - `claude_background`  Claude background shells still running

pub mod claude;
pub mod claude_background;
pub mod codex;
pub mod iso_time;
pub mod jsonl_stream;
pub mod token_count;

use std::path::Path;

use chrono::{DateTime, Utc};

/// `adapters.base.TurnUsage`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    pub reasoning_tokens: Option<u64>,
    pub api_calls: u64,
    pub turn_id: Option<String>,
}

/// Agents with a transcript reader (`adapters.<agent>.transcript` modules that exist).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptAgent {
    Claude,
    Codex,
}

impl TranscriptAgent {
    /// `None` is Python's `ModuleNotFoundError` for `adapters.<agent>.transcript`.
    pub fn from_agent(agent: &str) -> Option<Self> {
        match agent {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }

    /// `transcript.usage(path, turn_id)`.
    pub fn turn_usage(self, path: &Path, turn_id: Option<&str>) -> TurnUsage {
        match self {
            Self::Claude => claude::turn_usage(path),
            Self::Codex => codex::turn_usage(path, turn_id),
        }
    }

    /// `transcript.context_tokens(path)`.
    pub fn context_tokens(self, path: &Path) -> Option<u64> {
        match self {
            Self::Claude => claude::context_tokens(path),
            Self::Codex => codex::context_tokens(path),
        }
    }
}

/// `core.tasks.state._background_shells`: only Claude has a background-shell reader.
pub fn background_shells(
    agent: &str,
    transcript_path: Option<&str>,
    now: DateTime<Utc>,
) -> Vec<String> {
    match transcript_path.filter(|path| !path.is_empty()) {
        Some(path) if agent == "claude" => {
            claude_background::background_shells(Path::new(path), now)
        }
        _ => Vec::new(),
    }
}
