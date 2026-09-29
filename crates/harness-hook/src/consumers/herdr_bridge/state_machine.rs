//! Canonical agent state and its Herdr projection (`tools/herdr-bridge/state.py`).
//!
//! canonical `idle | working | ready | needs_input | blocked` → projected Herdr state
//! (`needs_input` shows as `blocked`, `ready` as `idle`). Ranking: priority, then `needs_input` oldest-first,
//! every other state newest-first, then session id.

use std::cmp::Ordering;

use serde_json::{Map, Value};

use super::session_state::python_float_or;
use crate::events::{is_python_truthy, python_str};

const PERMISSION_REASON_LIMIT: usize = 160;
const AUTHORITY: &str = "codex_hook";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateDecision {
    pub canonical: &'static str,
    pub projected: &'static str,
    pub reason: String,
    pub authority: &'static str,
    pub signal: String,
}

impl StateDecision {
    #[inline]
    fn new(canonical: &'static str, projected: &'static str, reason: String, event: &str) -> Self {
        Self {
            canonical,
            projected,
            reason,
            authority: AUTHORITY,
            signal: event.to_owned(),
        }
    }

    /// `status` field: `working` is stored as `running` for the orb-era readers.
    #[inline]
    pub fn status(&self) -> &'static str {
        if self.canonical == "working" {
            "running"
        } else {
            self.canonical
        }
    }
}

/// `decision_for_hook(event, payload)`.
pub fn decision_for_hook(event: &str, payload: &Map<String, Value>) -> Option<StateDecision> {
    let decision = match event {
        "SessionStart" => StateDecision::new("idle", "idle", "session_started".into(), event),
        "UserPromptSubmit" => {
            StateDecision::new("working", "working", "prompt_submitted".into(), event)
        }
        "PermissionRequest" => {
            StateDecision::new("needs_input", "blocked", permission_reason(payload), event)
        }
        "PostToolUse" => StateDecision::new("working", "working", "tool_completed".into(), event),
        "Stop" => StateDecision::new("ready", "idle", "turn_stopped".into(), event),
        _ => return None,
    };
    Some(decision)
}

/// `str(payload.get("permission_reason") or payload.get("reason") or "permission")[:160]`.
fn permission_reason(payload: &Map<String, Value>) -> String {
    ["permission_reason", "reason"]
        .iter()
        .filter_map(|name| payload.get(*name))
        .find(|value| is_python_truthy(value))
        .map(python_str)
        .unwrap_or_else(|| "permission".to_owned())
        .chars()
        .take(PERMISSION_REASON_LIMIT)
        .collect()
}

/// `PRIORITY.get(state, -1)`.
#[inline]
pub fn state_priority(state: &str) -> i32 {
    match state {
        "idle" => 0,
        "working" => 1,
        "ready" => 2,
        "needs_input" => 3,
        "blocked" => 4,
        _ => -1,
    }
}

/// `ranking_key(session, now)` as `(priority, time_key, session_id)`.
pub fn ranking_key(session: &Map<String, Value>, now: f64) -> (i32, f64, String) {
    let state = first_truthy_text(session, &["canonical_state", "status"])
        .unwrap_or_else(|| "idle".to_owned());
    let entered = ["state_entered_at", "updated_at"]
        .iter()
        .filter_map(|name| session.get(*name))
        .find(|value| is_python_truthy(value))
        .map_or(now, |value| python_float_or(value, now));
    let time_key = if state == "needs_input" {
        -entered
    } else {
        entered
    };
    let session_id = first_truthy_text(session, &["session_id"]).unwrap_or_default();
    (state_priority(&state), time_key, session_id)
}

/// `(state priority, recency, session id)` used to rank sessions.
type RankingKey = (i32, f64, String);

/// `highest_priority(sessions)`: Python `max` keeps the first of equal keys.
pub fn highest_priority(sessions: &[Map<String, Value>], now: f64) -> Option<&Map<String, Value>> {
    let mut best: Option<(&Map<String, Value>, RankingKey)> = None;
    for session in sessions {
        let key = ranking_key(session, now);
        let is_better = best
            .as_ref()
            .is_none_or(|(_, best_key)| compare_keys(&key, best_key) == Ordering::Greater);
        if is_better {
            best = Some((session, key));
        }
    }
    best.map(|(session, _)| session)
}

#[inline]
fn compare_keys(left: &(i32, f64, String), right: &(i32, f64, String)) -> Ordering {
    left.0
        .cmp(&right.0)
        .then(left.1.partial_cmp(&right.1).unwrap_or(Ordering::Equal))
        .then_with(|| left.2.cmp(&right.2))
}

#[inline]
fn first_truthy_text(session: &Map<String, Value>, names: &[&str]) -> Option<String> {
    names
        .iter()
        .filter_map(|name| session.get(*name))
        .find(|value| is_python_truthy(value))
        .map(python_str)
}
