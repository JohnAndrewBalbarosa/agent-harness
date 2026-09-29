//! turn.stop consumer (`core/usage/context_guard.py`): warn when a session's context gets expensive.
//!
//! Every request re-sends the whole context, so a long session costs more per step. When the latest
//! request's prompt reaches `CONTEXT_WARN_TOKENS` (default 150000), notify once per 50K band per
//! session: compact or start a new session. State: `AH_CONTEXT_GUARD_DIR` (default
//! `<root>/var/context-guard`) `<agent>-<session≤120>.json` = `{"band": n, "tokens": n}`.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::Context;
use obs_cli::python_compat::{parse_python_int, to_python_json, JsonLayout};
use serde_json::{json, Value};

use crate::consumer::{Consumer, ConsumerResult};
use crate::consumers::notify::decide::Sound;
use crate::consumers::notify::deliver::{
    deliver, herdr_binary, toast_script_path, SystemRunner, ToastRequest,
};
use crate::consumers::notify::is_dry_run;
use crate::consumers::notify::state_files::{read_json, session_file_name};
use crate::events::Event;
use crate::hook_context::HookContext;
use crate::shared_state::file_lock::with_sidecar_lock;
use crate::shared_state::lifecycle_log::{details, emit};
use crate::transcripts::TranscriptAgent;

pub const NAME: &str = "context-guard";
pub const BAND: u64 = 50_000;
pub const DEFAULT_THRESHOLD: i64 = 150_000;
pub const TOAST_TITLE: &str = "agent-harness: context";
pub const LIFECYCLE_FILE: &str = "context-guard.lifecycle.jsonl";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardOutcome {
    Ignored,
    Unsupported,
    Below,
    AlreadyWarned,
    Warned,
}

impl GuardOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ignored => "ignored",
            Self::Unsupported => "unsupported",
            Self::Below => "below",
            Self::AlreadyWarned => "already_warned",
            Self::Warned => "warned",
        }
    }
}

pub struct ContextGuardConsumer;

impl Consumer for ContextGuardConsumer {
    fn name(&self) -> &'static str {
        NAME
    }

    fn subscribes_to(&self, event: &str) -> bool {
        event == "turn.stop"
    }

    fn run(&self, context: &HookContext) -> anyhow::Result<ConsumerResult> {
        let env = &context.env;
        let toast_script = toast_script_path(&context.harness_root);
        let mut notify = |message: &str| {
            // Deviation: honours AH_NOTIFY_DRY_RUN (Python always delivers) so sandboxed runs stay silent.
            if is_dry_run(env) {
                return;
            }
            let request = ToastRequest {
                title: TOAST_TITLE,
                message,
                sound: Sound::Request,
            };
            let herdr = herdr_binary(env);
            deliver(&request, herdr.as_deref(), &SystemRunner, &toast_script);
        };
        handle(&context.event, env, &context.harness_root, &mut notify)?;
        Ok(ConsumerResult::succeeded(NAME, ""))
    }
}

/// `handle(event, env, notify)`.
pub fn handle(
    event: &Event,
    env: &BTreeMap<String, String>,
    harness_root: &Path,
    notify: &mut dyn FnMut(&str),
) -> anyhow::Result<GuardOutcome> {
    // Mental model: latest request size → threshold → once-per-band state (locked) → toast → lifecycle.
    if event.event != "turn.stop" {
        return Ok(GuardOutcome::Ignored);
    }
    let Some(transcript) = event.transcript_path.as_deref() else {
        return Ok(GuardOutcome::Ignored);
    };
    let Some(reader) = TranscriptAgent::from_agent(&event.agent) else {
        return Ok(GuardOutcome::Unsupported);
    };
    let threshold = warn_threshold(env);
    let tokens = match reader.context_tokens(Path::new(transcript)) {
        Some(tokens) if i128::from(tokens) >= i128::from(threshold) => tokens,
        _ => return Ok(GuardOutcome::Below),
    };

    let state_dir = state_dir(env, harness_root);
    std::fs::create_dir_all(&state_dir)
        .with_context(|| format!("unable to create {}", state_dir.display()))?;
    let state_path = state_dir.join(session_file_name(&event.agent, &event.session_id));
    let is_new_band = record_band_if_new(&state_path, tokens / BAND, tokens)
        .with_context(|| format!("unable to update {}", state_path.display()))?;
    if !is_new_band {
        return Ok(GuardOutcome::AlreadyWarned);
    }

    notify(&warning_message(&event.agent, tokens));
    emit(
        &lifecycle_path(env, harness_root),
        NAME,
        "context.warned",
        "succeeded",
        details([
            ("agent", json!(event.agent)),
            ("instance", json!(event.instance)),
            ("session", json!(event.session_id)),
            ("tokens", json!(tokens)),
            ("threshold", json!(threshold)),
        ]),
    );
    Ok(GuardOutcome::Warned)
}

/// `int(CONTEXT_WARN_TOKENS or 150000)`, unparseable → default.
pub fn warn_threshold(env: &BTreeMap<String, String>) -> i64 {
    env.get("CONTEXT_WARN_TOKENS")
        .filter(|text| !text.is_empty())
        .map_or(Some(DEFAULT_THRESHOLD), |text| {
            parse_python_int(text.as_str())
        })
        .unwrap_or(DEFAULT_THRESHOLD)
}

/// `f"{agent}: {round(tokens / 1000)}K tokens per request. ..."` (banker's rounding like Python).
pub fn warning_message(agent: &str, tokens: u64) -> String {
    let thousands = (tokens as f64 / 1000.0).round_ties_even() as u64;
    format!(
        "{agent}: {thousands}K tokens per request. \
         Mag-/compact o magsimula ng bagong session para bumaba ang gastos bawat hakbang."
    )
}

/// Under the state file's sidecar lock: `true` (and the state rewritten) only for a higher band.
fn record_band_if_new(state_path: &Path, band: u64, tokens: u64) -> io::Result<bool> {
    with_sidecar_lock(state_path, || {
        if i128::from(band) <= i128::from(warned_band(state_path)) {
            return Ok(false);
        }
        let text = to_python_json(
            &json!({"band": band, "tokens": tokens}),
            JsonLayout::Spaced,
            true,
        );
        std::fs::write(state_path, text)?;
        Ok(true)
    })
}

/// `int(json.loads(state).get("band", -1))`, any failure → -1.
#[inline]
fn warned_band(state_path: &Path) -> i64 {
    let Some(Value::Object(state)) = read_json(state_path) else {
        return -1;
    };
    match state.get("band") {
        None => -1,
        Some(Value::Number(number)) => number
            .as_i64()
            .or_else(|| {
                number
                    .as_f64()
                    .filter(|float| float.is_finite())
                    .map(|float| float.trunc() as i64)
            })
            .unwrap_or(-1),
        Some(Value::Bool(flag)) => i64::from(*flag),
        Some(Value::String(text)) => parse_python_int(text).unwrap_or(-1),
        Some(_) => -1,
    }
}

#[inline]
fn state_dir(env: &BTreeMap<String, String>, harness_root: &Path) -> PathBuf {
    env.get("AH_CONTEXT_GUARD_DIR")
        .filter(|directory| !directory.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| harness_root.join("var").join("context-guard"))
}

/// `AH_LOG_DIR` or `<root>/var/logs`, then `context-guard.lifecycle.jsonl`.
#[inline]
fn lifecycle_path(env: &BTreeMap<String, String>, harness_root: &Path) -> PathBuf {
    env.get("AH_LOG_DIR")
        .filter(|directory| !directory.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| harness_root.join("var").join("logs"))
        .join(LIFECYCLE_FILE)
}
