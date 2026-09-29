//! Claude Code turn token usage and context size (`adapters/claude/transcript.py`).
//!
//! A turn starts at the last user entry that is not meta and not a tool result (real prompts and
//! background-task notifications). One API response is split across several transcript lines with
//! identical usage, so responses are deduplicated by `message.id` (then `requestId`, then `uuid`).

use std::collections::HashSet;
use std::path::Path;

use serde_json::{Map, Value};

use super::jsonl_stream::{for_each_entry, Entry};
use super::token_count::count;
use super::TurnUsage;
use crate::events::{is_python_truthy, python_str};

/// `usage(path, turn_id)`: Claude's Stop payload has no turn id, so the latest turn is always used.
pub fn turn_usage(path: &Path) -> TurnUsage {
    // Mental model: every turn start restarts the tally, so the tally left at EOF is the latest turn
    // (Python: sum of `entries[last_start + 1:]`, or of every entry when no turn start exists).
    let mut tally = TurnTally::starting(None);
    for_each_entry(path, |entry| {
        if is_turn_start(&entry) {
            tally = TurnTally::starting(turn_id_of(&entry));
        } else {
            tally.add_entry(&entry);
        }
    });
    tally.finish()
}

/// `context_tokens(path)`: prompt size of the latest API request (input + cache read + cache write).
pub fn context_tokens(path: &Path) -> Option<u64> {
    let mut latest = None;
    for_each_entry(path, |entry| {
        if let Some(usage) = assistant_usage(&entry) {
            latest = Some(prompt_size(usage));
        }
    });
    latest
}

/// `is_turn_start(entry)`.
pub fn is_turn_start(entry: &Entry) -> bool {
    let is_real_user = entry.get("type").and_then(Value::as_str) == Some("user")
        && !entry.get("isMeta").is_some_and(is_python_truthy);
    if !is_real_user {
        return false;
    }
    match message_of(entry).and_then(|message| message.get("content")) {
        Some(Value::String(_)) => true,
        Some(Value::Array(blocks)) => !blocks.iter().any(is_tool_result_block),
        _ => false,
    }
}

/// Running per-turn totals (`sum_usage`).
struct TurnTally {
    input: u64,
    output: u64,
    cache_creation: u64,
    cache_read: u64,
    thinking: Option<u64>,
    seen_responses: HashSet<Option<String>>,
    turn_id: Option<String>,
}

impl TurnTally {
    fn starting(turn_id: Option<String>) -> Self {
        Self {
            input: 0,
            output: 0,
            cache_creation: 0,
            cache_read: 0,
            thinking: None,
            seen_responses: HashSet::new(),
            turn_id,
        }
    }

    fn add_entry(&mut self, entry: &Entry) {
        let Some(usage) = assistant_usage(entry) else {
            return;
        };
        if !self.seen_responses.insert(response_key(entry)) {
            return;
        }
        self.input += count(usage.get("input_tokens"));
        self.output += count(usage.get("output_tokens"));
        self.cache_creation += count(usage.get("cache_creation_input_tokens"));
        self.cache_read += count(usage.get("cache_read_input_tokens"));
        if let Some(thinking) = thinking_tokens(usage) {
            self.thinking = Some(self.thinking.unwrap_or(0) + thinking);
        }
    }

    fn finish(self) -> TurnUsage {
        TurnUsage {
            input_tokens: self.input + self.cache_read + self.cache_creation,
            output_tokens: self.output,
            cached_input_tokens: self.cache_read,
            reasoning_tokens: self.thinking,
            api_calls: self.seen_responses.len() as u64,
            turn_id: self.turn_id,
        }
    }
}

/// `entry.get("message") or {}` when it is an object.
#[inline]
fn message_of(entry: &Entry) -> Option<&Map<String, Value>> {
    entry.get("message").and_then(Value::as_object)
}

/// The `usage` object of an assistant entry.
#[inline]
fn assistant_usage(entry: &Entry) -> Option<&Map<String, Value>> {
    if entry.get("type").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    message_of(entry)?.get("usage").and_then(Value::as_object)
}

#[inline]
fn prompt_size(usage: &Map<String, Value>) -> u64 {
    count(usage.get("input_tokens"))
        + count(usage.get("cache_read_input_tokens"))
        + count(usage.get("cache_creation_input_tokens"))
}

/// `output_tokens_details.thinking_tokens` when present and not null.
#[inline]
fn thinking_tokens(usage: &Map<String, Value>) -> Option<u64> {
    let details = usage.get("output_tokens_details")?.as_object()?;
    details
        .get("thinking_tokens")
        .filter(|value| !value.is_null())
        .map(|value| count(Some(value)))
}

/// `message.get("id") or entry.get("requestId") or entry.get("uuid")`: the first truthy value, else
/// the last candidate as-is. JSON text keeps `1` and `"1"` distinct like Python's set does.
#[inline]
fn response_key(entry: &Entry) -> Option<String> {
    let candidates = [
        message_of(entry).and_then(|message| message.get("id")),
        entry.get("requestId"),
        entry.get("uuid"),
    ];
    let chosen = candidates
        .iter()
        .flatten()
        .find(|value| is_python_truthy(value))
        .copied()
        .or(candidates[2]);
    chosen
        .filter(|value| !value.is_null())
        .map(Value::to_string)
}

/// `entries[start].get("uuid")`.
#[inline]
fn turn_id_of(entry: &Entry) -> Option<String> {
    entry
        .get("uuid")
        .filter(|value| !value.is_null())
        .map(python_str)
}

#[inline]
fn is_tool_result_block(block: &Value) -> bool {
    block.get("type").and_then(Value::as_str) == Some("tool_result")
}
