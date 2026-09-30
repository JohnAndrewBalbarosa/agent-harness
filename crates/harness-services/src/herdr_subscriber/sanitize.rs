//! subscriber.py `sanitize(value, key, depth)`: bounds a Herdr event before it is logged or persisted.
//!
//! Rules, in order: depth > 5 → `"<depth-limited>"`; a key matching
//! `(token|secret|password|cookie|authorization|private.?key)` (case-insensitive) → `"<redacted>"`; objects keep
//! their first 80 entries with keys cut to 120 code points; arrays keep their first 80 items (items inherit the
//! parent key); strings are cut to 500 code points under a key matching `(surface|screen|buffer|output|content|raw)`
//! (case-insensitive), else 2000, with `"<truncated>"` appended when cut; other scalars pass through.

use obs_cli::python_compat::truncate_chars;
use serde_json::{Map, Value};

const MAX_DEPTH: usize = 5;
const MAX_ITEMS: usize = 80;
const MAX_KEY_CHARS: usize = 120;
const LARGE_TEXT_LIMIT: usize = 500;
const TEXT_LIMIT: usize = 2000;
const DEPTH_LIMITED: &str = "<depth-limited>";
const REDACTED: &str = "<redacted>";
const TRUNCATED: &str = "<truncated>";
const SENSITIVE_WORDS: [&str; 5] = ["token", "secret", "password", "cookie", "authorization"];
const LARGE_WORDS: [&str; 6] = ["surface", "screen", "buffer", "output", "content", "raw"];

/// `sanitize(event)` at the root (empty key, depth 0).
pub fn sanitize_event(value: &Value) -> Value {
    sanitize(value, "", 0)
}

/// `sanitize(event)` for an object body: at the root neither the depth guard nor the (empty) key applies.
pub fn sanitize_event_body(body: &Map<String, Value>) -> Map<String, Value> {
    sanitize_object(body, 0)
}

// Mental model: depth guard → key redaction → shape-specific bounding.
pub fn sanitize(value: &Value, key: &str, depth: usize) -> Value {
    if depth > MAX_DEPTH {
        return Value::from(DEPTH_LIMITED);
    }
    if is_sensitive_key(key) {
        return Value::from(REDACTED);
    }
    match value {
        Value::Object(map) => Value::Object(sanitize_object(map, depth)),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .take(MAX_ITEMS)
                .map(|item| sanitize(item, key, depth + 1))
                .collect(),
        ),
        Value::String(text) => Value::from(bounded_text(text, text_limit_for(key))),
        other => other.clone(),
    }
}

/// `{str(k)[:120]: sanitize(v, str(k), depth + 1) for k, v in list(value.items())[:80]}`; a cut key that
/// collides keeps its first position and the later value (Python dict semantics).
#[inline]
fn sanitize_object(map: &Map<String, Value>, depth: usize) -> Map<String, Value> {
    let mut bounded = Map::new();
    for (key, child) in map.iter().take(MAX_ITEMS) {
        bounded.insert(
            truncate_chars(key, MAX_KEY_CHARS),
            sanitize(child, key, depth + 1),
        );
    }
    bounded
}

/// `value[:limit] + ("<truncated>" if len(value) > limit else "")`.
#[inline]
pub fn bounded_text(text: &str, limit: usize) -> String {
    let mut chars = text.chars();
    let kept: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        kept + TRUNCATED
    } else {
        kept
    }
}

#[inline]
fn text_limit_for(key: &str) -> usize {
    if is_large_key(key) {
        LARGE_TEXT_LIMIT
    } else {
        TEXT_LIMIT
    }
}

/// `SENSITIVE_KEY.search(key)`.
#[inline]
pub fn is_sensitive_key(key: &str) -> bool {
    let folded = key.to_lowercase();
    SENSITIVE_WORDS.iter().any(|word| folded.contains(word)) || has_private_key_phrase(&folded)
}

/// `LARGE_KEY.search(key)`.
#[inline]
pub fn is_large_key(key: &str) -> bool {
    let folded = key.to_lowercase();
    LARGE_WORDS.iter().any(|word| folded.contains(word))
}

/// `private.?key`: "private", then at most one character other than a newline, then "key".
#[inline]
fn has_private_key_phrase(folded: &str) -> bool {
    folded.match_indices("private").any(|(start, word)| {
        let rest = &folded[start + word.len()..];
        if rest.starts_with("key") {
            return true;
        }
        let mut chars = rest.chars();
        match chars.next() {
            Some('\n') | None => false,
            Some(_) => chars.as_str().starts_with("key"),
        }
    })
}
