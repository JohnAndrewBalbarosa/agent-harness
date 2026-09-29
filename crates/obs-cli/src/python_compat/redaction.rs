//! obs.py:33-36 and :80 `redact`: secret-looking keys become `[REDACTED]` (unless they are
//! token *counts*), strings are cut to 1200 code points, `secret=value` assignments and
//! `Bearer value` credentials are masked, and lists keep at most 100 items.
//!
//! The regexes are hand-matched (ASCII case-insensitive) to keep the binary free of a regex engine:
//! - keyword  `password|passwd|secret|token|api[_-]?key|cookie|authorization|private[_-]?key`
//! - assign   `(keyword)(\s*[:=]\s*)\S+`  -> `\1\2[REDACTED]`
//! - bearer   `Bearer\s+\S+`              -> `Bearer [REDACTED]`

use serde_json::{Map, Value};

const REDACTED: &str = "[REDACTED]";
const MAX_TEXT_CHARS: usize = 1200;
const MAX_LIST_ITEMS: usize = 100;
const PLAIN_SECRET_KEYWORDS: [&str; 6] = [
    "password",
    "passwd",
    "secret",
    "token",
    "cookie",
    "authorization",
];
const SEPARATED_KEY_PREFIXES: [&str; 2] = ["api", "private"];
const TOKEN_COUNT_STEMS: [&str; 17] = [
    "input",
    "output",
    "cachedinput",
    "cached_input",
    "cachereadinput",
    "cache_readinput",
    "cacheread_input",
    "cache_read_input",
    "cachecreationinput",
    "cache_creationinput",
    "cachecreation_input",
    "cache_creation_input",
    "reasoning",
    "thinking",
    "total",
    "prompt",
    "completion",
];

pub fn redact(value: &Value) -> Value {
    match value {
        Value::Object(entries) => Value::Object(redact_entries(entries)),
        Value::Array(items) => {
            Value::Array(items.iter().take(MAX_LIST_ITEMS).map(redact).collect())
        }
        Value::String(text) => Value::String(redact_text(text)),
        other => other.clone(),
    }
}

fn redact_entries(entries: &Map<String, Value>) -> Map<String, Value> {
    entries
        .iter()
        .map(|(key, item)| {
            let replacement = if is_secret_field(key, item) {
                Value::String(REDACTED.to_owned())
            } else {
                redact(item)
            };
            (key.clone(), replacement)
        })
        .collect()
}

#[inline]
fn is_secret_field(key: &str, item: &Value) -> bool {
    contains_secret_keyword(key) && !is_token_count(key, item)
}

/// Mental model: bound the text first, then mask assignments, then mask bearer credentials.
pub fn redact_text(text: &str) -> String {
    let bounded: Vec<char> = text.chars().take(MAX_TEXT_CHARS).collect();
    let without_assignments = mask_secret_assignments(&bounded);
    mask_bearer_credentials(&without_assignments)
        .into_iter()
        .collect()
}

fn contains_secret_keyword(key: &str) -> bool {
    let characters: Vec<char> = key.chars().collect();
    (0..characters.len()).any(|start| match_secret_keyword(&characters, start).is_some())
}

/// obs.py:74 `is_token_count`: `None` or a non-negative integer under a `*_tokens` usage key.
fn is_token_count(key: &str, item: &Value) -> bool {
    let is_count = item.is_null() || item.as_u64().is_some();
    is_count && is_token_count_key(key)
}

fn is_token_count_key(key: &str) -> bool {
    let lowered = key.to_ascii_lowercase();
    let Some(stem) = lowered.strip_suffix("tokens") else {
        return false;
    };
    let stem = stem.strip_suffix('_').unwrap_or(stem);
    TOKEN_COUNT_STEMS.contains(&stem)
}

/// Returns the end index of a secret keyword starting at `start`.
fn match_secret_keyword(characters: &[char], start: usize) -> Option<usize> {
    let plain = PLAIN_SECRET_KEYWORDS
        .iter()
        .find_map(|keyword| match_literal(characters, start, keyword));

    plain.or_else(|| {
        SEPARATED_KEY_PREFIXES
            .iter()
            .find_map(|prefix| match_separated_key(characters, start, prefix))
    })
}

/// `prefix[_-]?key`.
fn match_separated_key(characters: &[char], start: usize, prefix: &str) -> Option<usize> {
    let after_prefix = match_literal(characters, start, prefix)?;
    let after_separator = match characters.get(after_prefix) {
        Some('_') | Some('-') => after_prefix + 1,
        _ => after_prefix,
    };
    match_literal(characters, after_separator, "key")
        .or_else(|| match_literal(characters, after_prefix, "key"))
}

fn match_literal(characters: &[char], start: usize, literal: &str) -> Option<usize> {
    let mut index = start;
    for expected in literal.chars() {
        let actual = characters.get(index)?;
        if actual.to_ascii_lowercase() != expected {
            return None;
        }
        index += 1;
    }
    Some(index)
}

/// `(keyword)(\s*[:=]\s*)\S+` -> keyword + separator + `[REDACTED]`, scanning left to right.
fn mask_secret_assignments(characters: &[char]) -> Vec<char> {
    let mut masked = Vec::with_capacity(characters.len());
    let mut index = 0;

    while index < characters.len() {
        match match_secret_assignment(characters, index) {
            Some((value_start, value_end)) => {
                masked.extend_from_slice(&characters[index..value_start]);
                masked.extend(REDACTED.chars());
                index = value_end;
            }
            None => {
                masked.push(characters[index]);
                index += 1;
            }
        }
    }
    masked
}

/// Returns `(value_start, value_end)` of the masked `\S+` part.
fn match_secret_assignment(characters: &[char], start: usize) -> Option<(usize, usize)> {
    let after_keyword = match_secret_keyword(characters, start)?;
    let before_operator = skip_whitespace(characters, after_keyword);

    if !matches!(characters.get(before_operator), Some(':') | Some('=')) {
        return None;
    }

    let value_start = skip_whitespace(characters, before_operator + 1);
    let value_end = skip_non_whitespace(characters, value_start);
    (value_end > value_start).then_some((value_start, value_end))
}

/// `Bearer\s+\S+` -> `Bearer [REDACTED]`.
fn mask_bearer_credentials(characters: &[char]) -> Vec<char> {
    let mut masked = Vec::with_capacity(characters.len());
    let mut index = 0;

    while index < characters.len() {
        match match_bearer_credential(characters, index) {
            Some(end) => {
                masked.extend("Bearer ".chars().chain(REDACTED.chars()));
                index = end;
            }
            None => {
                masked.push(characters[index]);
                index += 1;
            }
        }
    }
    masked
}

fn match_bearer_credential(characters: &[char], start: usize) -> Option<usize> {
    let after_word = match_literal(characters, start, "bearer")?;
    let value_start = skip_whitespace(characters, after_word);

    if value_start == after_word {
        return None;
    }

    let value_end = skip_non_whitespace(characters, value_start);
    (value_end > value_start).then_some(value_end)
}

#[inline]
fn skip_whitespace(characters: &[char], start: usize) -> usize {
    start
        + characters[start..]
            .iter()
            .take_while(|c| c.is_whitespace())
            .count()
}

#[inline]
fn skip_non_whitespace(characters: &[char], start: usize) -> usize {
    start
        + characters[start..]
            .iter()
            .take_while(|c| !c.is_whitespace())
            .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redacts_secret_fields_recursively_like_obs_py_test() {
        let value = redact(
            &json!({"token": "secret", "nested": {"apiKey": "secret", "safe": "authorization=private Bearer hidden"}}),
        );

        assert_eq!(value["token"], "[REDACTED]");
        assert_eq!(value["nested"]["apiKey"], "[REDACTED]");
        assert_eq!(
            value["nested"]["safe"],
            "authorization=[REDACTED] Bearer [REDACTED]"
        );
    }

    #[test]
    fn keeps_token_counts() {
        let value = redact(
            &json!({"input_tokens": 12, "outputTokens": null, "token": 5, "total_tokens": -1}),
        );

        assert_eq!(value["input_tokens"], 12);
        assert_eq!(value["outputTokens"], Value::Null);
        assert_eq!(value["token"], "[REDACTED]");
        assert_eq!(value["total_tokens"], "[REDACTED]");
    }

    #[test]
    fn masks_assignments_with_spacing_and_separators() {
        assert_eq!(redact_text("api-key : abc def"), "api-key : [REDACTED] def");
        assert_eq!(redact_text("PRIVATE_KEY=x"), "PRIVATE_KEY=[REDACTED]");
        assert_eq!(redact_text("token="), "token=");
        assert_eq!(redact_text("bearer   xyz"), "Bearer [REDACTED]");
    }

    #[test]
    fn bounds_text_and_lists() {
        let long = "a".repeat(1300);
        assert_eq!(redact_text(&long).chars().count(), 1200);

        let items: Vec<Value> = (0..150).map(Value::from).collect();
        assert_eq!(
            redact(&Value::Array(items)).as_array().map(Vec::len),
            Some(100)
        );
    }
}
