//! Taglish communication check on `request_user_input` tool calls (hook.py
//! `communication_language_mismatches` + the PostToolUse `language_mismatch` event and error record).

use obs_cli::python_compat::{python_utc_now, uuid7_text};
use serde_json::{json, Value};

use super::behaviour_audit::normalized_tool_name;
use super::event_records::{
    error_record_data, event_emit_data, PolicyViolation, Signal, TraceSlot,
};
use super::fingerprints::sha256_hex;
use super::operations::operation;
use super::payload_fields::{python_or, Payload};
use crate::events::{is_python_truthy, python_str};

const REQUEST_INPUT_SUFFIX: &str = "request_user_input";
const MIN_WORDS: usize = 5;
const TAGALOG_MARKERS: [&str; 28] = [
    "ang", "ano", "at", "ay", "dahil", "gamit", "gamitin", "ikaw", "ilagay", "ipakita", "ito",
    "iyon", "kapag", "kung", "mag", "may", "mga", "na", "nang", "ng", "para", "pero", "pinili",
    "pwede", "sa", "saka", "tapos", "yung",
];
const MISMATCH_SUMMARY: &str =
    "English-only explanatory text appeared in a Taglish user-input surface.";

/// Field paths (`questions[i].question`, `questions[i].options[j].description`) whose text has at least
/// five words and none of the Tagalog markers.
pub fn communication_language_mismatches(payload: &Payload, tool: &str) -> Vec<String> {
    if !normalized_tool_name(tool).ends_with(REQUEST_INPUT_SUFFIX) {
        return Vec::new();
    }
    let Some(tool_input) = decoded_tool_input(payload) else {
        return Vec::new();
    };
    question_fields(&tool_input)
        .into_iter()
        .filter(|(_, text)| lacks_taglish_signal(text))
        .map(|(path, _)| path)
        .collect()
}

/// `value.get("tool_input") or value.get("toolInput") or {}`, JSON-decoding a string (None on bad JSON).
fn decoded_tool_input(payload: &Payload) -> Option<Value> {
    let raw = python_or(payload, &["tool_input", "toolInput"]);
    match raw {
        Value::String(text) if !text.is_empty() => serde_json::from_str(&text).ok(),
        other if is_python_truthy(&other) => Some(other),
        _ => Some(json!({})),
    }
}

/// Mental model: every dict question contributes its question text, then each dict option's description.
fn question_fields(tool_input: &Value) -> Vec<(String, Value)> {
    let questions = tool_input
        .get("questions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut fields = Vec::new();

    for (question_index, question) in questions.iter().enumerate() {
        let Some(question) = question.as_object() else {
            continue;
        };
        let text = question.get("question").cloned().unwrap_or(Value::Null);
        fields.push((format!("questions[{question_index}].question"), text));

        let options = question
            .get("options")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for (option_index, option) in options.iter().enumerate() {
            if let Some(option) = option.as_object() {
                let text = option.get("description").cloned().unwrap_or(Value::Null);
                fields.push((
                    format!("questions[{question_index}].options[{option_index}].description"),
                    text,
                ));
            }
        }
    }
    fields
}

/// `len(words) >= 5 and not TAGALOG_MARKERS.intersection(words)` over `[A-Za-zÀ-ÿ]+` of `str(text or "").lower()`.
fn lacks_taglish_signal(text: &Value) -> bool {
    let lowered = if is_python_truthy(text) {
        python_str(text).to_lowercase()
    } else {
        String::new()
    };
    let words: Vec<&str> = lowered
        .split(|character: char| !is_word_character(character))
        .filter(|word| !word.is_empty())
        .collect();
    words.len() >= MIN_WORDS && !words.iter().any(|word| TAGALOG_MARKERS.contains(word))
}

#[inline]
fn is_word_character(character: char) -> bool {
    character.is_ascii_alphabetic() || ('\u{C0}'..='\u{FF}').contains(&character)
}

/// The PostToolUse `agent.communication.language_mismatch` event plus its `error.record`.
pub fn mismatch_operations(
    slot: &TraceSlot,
    sequence_no: i64,
    field_paths: &[String],
) -> Vec<Value> {
    let observed_at = python_utc_now();
    let event_id = uuid7_text();
    let path_hashes: Vec<String> = field_paths.iter().map(|path| sha256_hex(path)).collect();
    let signal = Signal {
        code: "agent.communication.language_mismatch",
        category: 3,
        severity: 3,
        outcome: 3,
        summary: "User-facing choice text did not contain the configured Taglish signal."
            .to_owned(),
        attributes: json!({
            "surface": REQUEST_INPUT_SUFFIX,
            "expectedLanguage": "tl-en",
            "fieldPaths": field_paths,
            "fieldPathHashes": path_hashes,
        }),
    };
    let event = event_emit_data(&event_id, slot, sequence_no, signal);
    let violation = PolicyViolation {
        fingerprint_source: format!(
            "{}|agent.communication.language_mismatch|request_user_input",
            python_str(slot.project_id)
        ),
        exception_class: "CommunicationPolicyViolation",
        summary: MISMATCH_SUMMARY,
        event_id: &event_id,
        observed_at: &observed_at,
    };
    vec![
        operation("event.emit", Value::Object(event), Some(&observed_at)),
        operation(
            "error.record",
            error_record_data(slot.project_id, &violation),
            Some(&observed_at),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(value: Value) -> Payload {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn request_input_language_mismatch_checks_gray_descriptions() {
        let english = payload(
            json!({"tool_input": {"questions": [{"question": "Which technology should we use?", "options": [{"description": "Use the maintained package with documented public APIs."}]}]}}),
        );
        let taglish = payload(
            json!({"tool_input": {"questions": [{"question": "Aling technology ang gagamitin natin?", "options": [{"description": "Gamitin ang maintained package at documented public API."}]}]}}),
        );
        assert_eq!(
            communication_language_mismatches(&english, "request_user_input"),
            [
                "questions[0].question",
                "questions[0].options[0].description"
            ]
        );
        assert!(communication_language_mismatches(&taglish, "request_user_input").is_empty());
    }

    #[test]
    fn string_tool_input_is_decoded_and_other_tools_are_ignored() {
        let encoded = payload(
            json!({"toolInput": "{\"questions\": [{\"question\": \"Which option do you want here?\"}]}"}),
        );
        assert_eq!(
            communication_language_mismatches(&encoded, "functions.request-user-input"),
            ["questions[0].question"]
        );
        assert!(communication_language_mismatches(&encoded, "functions.exec").is_empty());
        let broken = payload(json!({"tool_input": "{not json"}));
        assert!(communication_language_mismatches(&broken, "request_user_input").is_empty());
    }
}
