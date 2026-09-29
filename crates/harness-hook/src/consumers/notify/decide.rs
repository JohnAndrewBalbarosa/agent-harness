//! Pure notification decision engine (`core/notify/decide.py`).
//!
//! prompt.submit marks the session pending; turn.stop toasts "done" once the prompt is finished and no
//! background work (subagents, background shells) is still running; attention.needed toasts "request"
//! (idle prompts wait for background work). A global rate limit caps toasts per window.

use serde_json::{Map, Value};

use crate::events::{is_python_truthy, python_str, Event};

pub const DONE_MESSAGE: &str = "Tapos na ang prompt!";
pub const ATTENTION_MESSAGE: &str = "Kailangan ng input mo!";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotifyState {
    pub pending: bool,
    /// Epoch seconds of recent toasts (the `_rate.json` window).
    pub recent_toasts: Vec<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Toast,
    Defer,
    Skip,
    Mark,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Toast => "toast",
            Self::Defer => "defer",
            Self::Skip => "skip",
            Self::Mark => "mark",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sound {
    Done,
    Request,
}

impl Sound {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Request => "request",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub action: Action,
    pub reason: &'static str,
    pub sound: Option<Sound>,
    pub message: Option<String>,
}

impl Decision {
    #[inline]
    fn quiet(action: Action, reason: &'static str) -> Self {
        Self {
            action,
            reason,
            sound: None,
            message: None,
        }
    }
}

/// `NOTIFY_RATE_LIMIT` = `<limit>/<window seconds>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimit {
    pub limit: i64,
    pub window_s: i64,
}

impl Default for RateLimit {
    fn default() -> Self {
        Self {
            limit: 5,
            window_s: 60,
        }
    }
}

/// `decide(event, state, outstanding_tasks, now_s, rate)`; never mutates `state`.
pub fn decide(
    event: &Event,
    state: &NotifyState,
    outstanding_tasks: &[String],
    now_s: i64,
    rate: RateLimit,
) -> (Decision, NotifyState) {
    match event.event.as_str() {
        "prompt.submit" => (
            Decision::quiet(Action::Mark, "pending"),
            NotifyState {
                pending: true,
                ..state.clone()
            },
        ),
        "turn.stop" => decide_turn_stop(event, state, outstanding_tasks, now_s, rate),
        "attention.needed" => decide_attention(event, state, outstanding_tasks, now_s, rate),
        _ => (
            Decision::quiet(Action::Skip, "unhandled-event"),
            state.clone(),
        ),
    }
}

fn decide_turn_stop(
    event: &Event,
    state: &NotifyState,
    outstanding_tasks: &[String],
    now_s: i64,
    rate: RateLimit,
) -> (Decision, NotifyState) {
    if event
        .data
        .get("stop_hook_active")
        .is_some_and(is_python_truthy)
    {
        return (
            Decision::quiet(Action::Skip, "stop-hook-active"),
            state.clone(),
        );
    }
    if !state.pending {
        return (
            Decision::quiet(Action::Skip, "no-pending-prompt"),
            state.clone(),
        );
    }
    if !outstanding_tasks.is_empty() {
        return (
            Decision::quiet(Action::Defer, "background-tasks"),
            state.clone(),
        );
    }
    toast(
        state,
        now_s,
        rate,
        Sound::Done,
        DONE_MESSAGE.to_owned(),
        false,
    )
}

fn decide_attention(
    event: &Event,
    state: &NotifyState,
    outstanding_tasks: &[String],
    now_s: i64,
    rate: RateLimit,
) -> (Decision, NotifyState) {
    let empty = Map::new();
    let attention = event
        .data
        .get("attention")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let is_idle = attention.get("kind").and_then(Value::as_str) == Some("idle");
    if is_idle && !outstanding_tasks.is_empty() {
        return (
            Decision::quiet(Action::Defer, "background-tasks"),
            state.clone(),
        );
    }
    let message = attention
        .get("message")
        .filter(|message| is_python_truthy(message))
        .map_or_else(|| ATTENTION_MESSAGE.to_owned(), python_str);
    toast(state, now_s, rate, Sound::Request, message, state.pending)
}

/// `_toast`: consumes one rate slot, or skips when the window is full (the pruned window is kept).
fn toast(
    state: &NotifyState,
    now_s: i64,
    rate: RateLimit,
    sound: Sound,
    message: String,
    pending: bool,
) -> (Decision, NotifyState) {
    let (allowed, recent_toasts) = within_rate(&state.recent_toasts, now_s, rate);
    let new_state = NotifyState {
        pending,
        recent_toasts,
    };
    if !allowed {
        return (Decision::quiet(Action::Skip, "rate-limited"), new_state);
    }
    let decision = Decision {
        action: Action::Toast,
        reason: sound.as_str(),
        sound: Some(sound),
        message: Some(message),
    };
    (decision, new_state)
}

/// `_within_rate`: keeps toasts inside `[now - window, now]` (future stamps from clock skew expire).
#[inline]
fn within_rate(recent: &[i64], now_s: i64, rate: RateLimit) -> (bool, Vec<i64>) {
    let mut kept: Vec<i64> = recent
        .iter()
        .copied()
        .filter(|stamp| now_s - rate.window_s <= *stamp && *stamp <= now_s)
        .collect();
    if kept.len() as i64 >= rate.limit {
        return (false, kept);
    }
    kept.push(now_s);
    (true, kept)
}
