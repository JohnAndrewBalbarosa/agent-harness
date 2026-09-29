//! Codex CLI turn token usage and context size from the rollout file (`adapters/codex/transcript.py`).
//!
//! Uses `token_usage_record` entries (one per model response, with `turn_id` and `response_id`),
//! deduplicated by `response_id`. OpenAI `input_tokens` already include cached tokens.

use std::collections::HashSet;
use std::path::Path;

use serde_json::{Map, Value};

use super::jsonl_stream::for_each_entry;
use super::token_count::count;
use super::TurnUsage;
use crate::events::{is_python_truthy, python_str};

/// `usage(path, turn_id)`.
pub fn turn_usage(path: &Path, turn_id: Option<&str>) -> TurnUsage {
    // Mental model: choose the target turn (the given one, else the last turn id in the file, which
    // needs a first pass), then sum that turn's distinct responses in a second pass. Two passes keep
    // memory bounded instead of holding every record like the Python list.
    let target = match turn_id.filter(|turn| !turn.is_empty()) {
        Some(turn) => Value::String(turn.to_owned()),
        None => latest_turn_id(path),
    };
    let mut tally = CodexTally::default();
    for_each_usage_record(path, |record| tally.add_if_in_turn(record, &target));
    tally.finish(&target)
}

/// `context_tokens(path)`: `token_count.info.last_token_usage.input_tokens` of the latest request.
pub fn context_tokens(path: &Path) -> Option<u64> {
    let mut latest = None;
    for_each_entry(path, |entry| {
        if let Some(last_usage) = last_token_usage(&entry) {
            latest = Some(count(last_usage.get("input_tokens")));
        }
    });
    latest
}

#[derive(Default)]
struct CodexTally {
    input: u64,
    cached: u64,
    output: u64,
    reasoning: u64,
    seen_responses: HashSet<String>,
}

impl CodexTally {
    fn add_if_in_turn(&mut self, record: &Map<String, Value>, target: &Value) {
        let record_turn = record.get("turn_id").unwrap_or(&Value::Null);
        if record_turn != target {
            return;
        }
        if !self.seen_responses.insert(response_id_of(record)) {
            return;
        }
        let empty = Map::new();
        let tokens = record
            .get("usage")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        self.input += count(tokens.get("input_tokens"));
        self.cached += count(tokens.get("cached_input_tokens"));
        self.output += count(tokens.get("output_tokens"));
        self.reasoning += count(tokens.get("reasoning_output_tokens"));
    }

    fn finish(self, target: &Value) -> TurnUsage {
        TurnUsage {
            input_tokens: self.input,
            output_tokens: self.output,
            cached_input_tokens: self.cached,
            reasoning_tokens: Some(self.reasoning),
            api_calls: self.seen_responses.len() as u64,
            turn_id: (!target.is_null()).then(|| python_str(target)),
        }
    }
}

/// Visits the `payload` of every `token_usage_record` entry (`e.get("payload") or {}`).
fn for_each_usage_record(path: &Path, mut visit: impl FnMut(&Map<String, Value>)) {
    let empty = Map::new();
    for_each_entry(path, |entry| {
        if entry.get("type").and_then(Value::as_str) != Some("token_usage_record") {
            return;
        }
        visit(
            entry
                .get("payload")
                .and_then(Value::as_object)
                .unwrap_or(&empty),
        );
    });
}

/// The last truthy `turn_id` among usage records (`Value::Null` when none).
fn latest_turn_id(path: &Path) -> Value {
    let mut latest = Value::Null;
    for_each_usage_record(path, |record| {
        if let Some(turn) = record.get("turn_id").filter(|turn| is_python_truthy(turn)) {
            latest = turn.clone();
        }
    });
    latest
}

/// `str(record.get("response_id") or "")`.
#[inline]
fn response_id_of(record: &Map<String, Value>) -> String {
    record
        .get("response_id")
        .filter(|value| is_python_truthy(value))
        .map(python_str)
        .unwrap_or_default()
}

/// `payload.info.last_token_usage` of an `event_msg/token_count` entry.
#[inline]
fn last_token_usage(entry: &Map<String, Value>) -> Option<&Map<String, Value>> {
    let payload = entry.get("payload")?.as_object()?;
    if payload.get("type").and_then(Value::as_str) != Some("token_count") {
        return None;
    }
    payload
        .get("info")?
        .as_object()?
        .get("last_token_usage")?
        .as_object()
}
