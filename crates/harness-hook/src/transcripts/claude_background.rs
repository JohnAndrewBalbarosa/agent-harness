//! Claude Code background shells still running, from the session transcript
//! (`adapters/claude/background.py`; no platform hook exists for them).
//!
//! Launch markers are accepted only at the start of a tool result, so output that merely echoes them
//! (logs, greps) is ignored; completions (`<task-id>`, any status) only from real notification entries.
//! The Python regexes are matched by hand (no regex dependency):
//! - launch:     `\A(?:Command running in background with ID: (\w+)`
//!   continued:  `|Command did not complete[^\n]*?moved to the background \(ID: (\w+)\))`
//! - completion: `<task-id>(\w+)</task-id>` (findall)

use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use obs_cli::python_compat::{to_python_json, JsonLayout};
use serde_json::Value;

use super::iso_time::parse_timestamp_value;
use super::jsonl_stream::{for_each_entry, Entry};
use crate::events::python_str;

pub const MAX_AGE_HOURS: i64 = 3;
const RUNNING_PREFIX: &str = "Command running in background with ID: ";
const TIMEOUT_PREFIX: &str = "Command did not complete";
const MOVED_MARKER: &str = "moved to the background (ID: ";
const TASK_ID_OPEN: &str = "<task-id>";
const TASK_ID_CLOSE: &str = "</task-id>";
const TASK_NOTIFICATION_PREFIX: &str = "<task-notification>";
const NOTIFICATION_ENTRY_TYPES: [&str; 2] = ["queue-operation", "attachment"];

/// `background_shells(transcript, now)`, streamed.
pub fn background_shells(transcript: &Path, now: DateTime<Utc>) -> Vec<String> {
    let mut tracker = ShellTracker::default();
    for_each_entry(transcript, |entry| tracker.observe(&entry));
    tracker.running_at(now)
}

/// `scan(entries, now)` over already-parsed entries.
pub fn scan<'a>(entries: impl IntoIterator<Item = &'a Entry>, now: DateTime<Utc>) -> Vec<String> {
    let mut tracker = ShellTracker::default();
    for entry in entries {
        tracker.observe(entry);
    }
    tracker.running_at(now)
}

/// Launched-but-not-completed shells in first-launch order (Python dict insertion order).
#[derive(Default)]
struct ShellTracker {
    running: Vec<(String, Option<DateTime<Utc>>)>,
}

impl ShellTracker {
    fn observe(&mut self, entry: &Entry) {
        let launched_at = parse_timestamp_value(entry.get("timestamp"));
        for text in tool_result_texts(entry) {
            if let Some(shell_id) = launched_shell_id(&text) {
                self.mark_running(shell_id, launched_at);
            }
        }
        for task_id in completed_task_ids(&notification_text(entry)) {
            self.running
                .retain(|(running_id, _)| running_id.as_str() != task_id);
        }
    }

    /// `running[id] = ts`: an existing key keeps its position and takes the new timestamp.
    fn mark_running(&mut self, shell_id: &str, launched_at: Option<DateTime<Utc>>) {
        match self
            .running
            .iter_mut()
            .find(|(running_id, _)| running_id.as_str() == shell_id)
        {
            Some(existing) => existing.1 = launched_at,
            None => self.running.push((shell_id.to_owned(), launched_at)),
        }
    }

    fn running_at(self, now: DateTime<Utc>) -> Vec<String> {
        let max_age = Duration::hours(MAX_AGE_HOURS);
        self.running
            .into_iter()
            .filter(|(_, started)| started.is_none_or(|started| now - started <= max_age))
            .map(|(shell_id, _)| shell_id)
            .collect()
    }
}

/// `_tool_result_texts(entry)`: the text of every `tool_result` block of a user entry.
fn tool_result_texts(entry: &Entry) -> Vec<String> {
    if entry.get("type").and_then(Value::as_str) != Some("user") {
        return Vec::new();
    }
    let Some(Value::Array(blocks)) = entry
        .get("message")
        .and_then(Value::as_object)
        .and_then(|message| message.get("content"))
    else {
        return Vec::new();
    };
    blocks
        .iter()
        .filter_map(Value::as_object)
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
        .filter_map(|block| tool_result_text(block.get("content")))
        .collect()
}

/// A string result as-is; a list result as its items' `text` joined by newlines.
#[inline]
fn tool_result_text(inner: Option<&Value>) -> Option<String> {
    match inner {
        Some(Value::String(text)) => Some(text.clone()),
        Some(Value::Array(items)) => Some(
            items
                .iter()
                .filter_map(Value::as_object)
                .map(|item| item.get("text").map(python_str).unwrap_or_default())
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        _ => None,
    }
}

/// `_notification_text(entry)`: whole notification entries as JSON text, or a user
/// `<task-notification>` message; anything else is empty.
fn notification_text(entry: &Entry) -> String {
    let entry_type = entry.get("type").and_then(Value::as_str);
    if entry_type.is_some_and(|kind| NOTIFICATION_ENTRY_TYPES.contains(&kind)) {
        return to_python_json(&Value::Object(entry.clone()), JsonLayout::Spaced, false);
    }
    let content = entry
        .get("message")
        .and_then(Value::as_object)
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str);
    match content {
        Some(text)
            if entry_type == Some("user")
                && text.trim_start().starts_with(TASK_NOTIFICATION_PREFIX) =>
        {
            text.to_owned()
        }
        _ => String::new(),
    }
}

/// `SHELL_LAUNCH.match(text)`: the shell id when the text starts with a launch marker.
pub fn launched_shell_id(text: &str) -> Option<&str> {
    if let Some(rest) = text.strip_prefix(RUNNING_PREFIX) {
        return leading_word(rest);
    }
    let rest = text.strip_prefix(TIMEOUT_PREFIX)?;
    // `[^\n]*?` cannot cross a newline, so the marker must sit on the first line; the lazy scan
    // takes the first marker position whose `(\w+)\)` tail matches.
    let first_line = rest.split('\n').next().unwrap_or_default();
    let mut search_from = 0;
    while let Some(offset) = first_line[search_from..].find(MOVED_MARKER) {
        let marker_start = search_from + offset;
        let tail = &first_line[marker_start + MOVED_MARKER.len()..];
        if let Some(word) = leading_word(tail).filter(|word| tail[word.len()..].starts_with(')')) {
            return Some(word);
        }
        search_from = marker_start + 1;
    }
    None
}

/// `COMPLETION.findall(text)`.
pub fn completed_task_ids(text: &str) -> Vec<&str> {
    let mut task_ids = Vec::new();
    let mut cursor = 0;
    while let Some(offset) = text[cursor..].find(TASK_ID_OPEN) {
        let open_at = cursor + offset;
        let body_start = open_at + TASK_ID_OPEN.len();
        let tail = &text[body_start..];
        match leading_word(tail).filter(|word| tail[word.len()..].starts_with(TASK_ID_CLOSE)) {
            Some(word) => {
                task_ids.push(word);
                cursor = body_start + word.len() + TASK_ID_CLOSE.len();
            }
            None => cursor = open_at + 1,
        }
    }
    task_ids
}

/// The maximal leading `\w+` run (Unicode word characters), `None` when empty.
#[inline]
fn leading_word(text: &str) -> Option<&str> {
    let end = text
        .char_indices()
        .find(|(_, character)| !is_word_char(*character))
        .map_or(text.len(), |(index, _)| index);
    if end == 0 {
        None
    } else {
        Some(&text[..end])
    }
}

#[inline]
fn is_word_char(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_markers_match_only_at_the_start() {
        assert_eq!(
            launched_shell_id("Command running in background with ID: b1x. Output"),
            Some("b1x")
        );
        assert_eq!(
            launched_shell_id("Command did not complete within 120s and was moved to the background (ID: b2y). Out"),
            Some("b2y")
        );
        assert_eq!(
            launched_shell_id("x Command running in background with ID: zz"),
            None
        );
        assert_eq!(
            launched_shell_id("Command did not complete\nmoved to the background (ID: b3)"),
            None
        );
        assert_eq!(
            launched_shell_id("Command running in background with ID: "),
            None
        );
    }

    #[test]
    fn lazy_scan_skips_markers_without_a_closing_paren() {
        let text = "Command did not complete moved to the background (ID: a b) moved to the background (ID: ok)";
        assert_eq!(launched_shell_id(text), Some("ok"));
    }

    #[test]
    fn completion_findall_is_non_overlapping() {
        let text = "<task-id>a1</task-id><task-id>bad<task-id>b_2</task-id>";
        assert_eq!(completed_task_ids(text), ["a1", "b_2"]);
    }
}
