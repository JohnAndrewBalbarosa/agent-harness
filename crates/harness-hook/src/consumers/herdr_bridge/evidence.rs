//! Pane evidence on a state transition or failed projection (`hook.py` `capture_evidence`): the pane's recent
//! text (redacted, at most 32 KiB) and `herdr agent explain` output, queued as `pane-evidence.record`.

use std::time::Duration;

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::herdr_cli::{tail_chars, HerdrRunner};
use super::identity::is_python_space;
use super::observability_outbox::ObservabilityOutbox;

const EVIDENCE_TIMEOUT: Duration = Duration::from_secs(5);
const RECENT_LIMIT: usize = 32_768;
const EXPLAIN_LIMIT: usize = 12_000;
const EXPLAIN_ERROR_LIMIT: usize = 1000;
const EXPLAIN_RAW_LIMIT: usize = 4000;
const MAX_LINES: usize = 80;
/// `(authorization|api[_-]?key|token|password|secret|cookie)`, tried in this order at each position.
const SECRET_KEYWORDS: [&str; 6] = [
    "authorization",
    "api?key",
    "token",
    "password",
    "secret",
    "cookie",
];

pub struct EvidenceRequest<'a> {
    pub project_id: &'a str,
    pub native_session: &'a str,
    pub pane_id: &'a str,
    pub trigger: &'a str,
}

// Mental model: read pane + explain (both failing → nothing) → redact/bound → queue one record.
pub fn capture_evidence(
    request: &EvidenceRequest<'_>,
    runner: &dyn HerdrRunner,
    outbox: &mut ObservabilityOutbox,
) {
    let pane = request.pane_id.to_owned();
    let read = runner.run(
        &args(&[
            "pane",
            "read",
            &pane,
            "--source",
            "recent-unwrapped",
            "--lines",
            "80",
        ]),
        EVIDENCE_TIMEOUT,
        RECENT_LIMIT,
    );
    let explain = runner.run(
        &args(&["agent", "explain", &pane, "--json"]),
        EVIDENCE_TIMEOUT,
        EXPLAIN_LIMIT,
    );
    if read.exit_code != 0 && explain.exit_code != 0 {
        return;
    }

    let (recent, byte_count) = bounded_utf8(&redact_evidence(&read.output), RECENT_LIMIT);
    let explain_value = explain_document(explain.exit_code, &explain.output);
    let line_count = MAX_LINES.min(recent.matches('\n').count() + usize::from(!recent.is_empty()));

    let mut data = Map::new();
    data.insert("snapshotId".into(), json!(uuid::Uuid::new_v4().to_string()));
    data.insert("projectId".into(), json!(request.project_id));
    data.insert("nativeSession".into(), json!(request.native_session));
    data.insert("paneId".into(), json!(request.pane_id));
    data.insert("triggerCode".into(), json!(request.trigger));
    data.insert(
        "contentSha256".into(),
        json!(hex::encode(Sha256::digest(recent.as_bytes()))),
    );
    data.insert("lineCount".into(), json!(line_count));
    data.insert("byteCount".into(), json!(byte_count));
    data.insert("redactedContent".into(), json!(recent));
    data.insert("explain".into(), explain_value);
    outbox.record("pane-evidence.record", Value::Object(data));
}

/// `json.loads(raw)` on success, `{"error": raw[-1000:]}` on failure, `{"raw": ...}` when unparseable;
/// a non-object document is wrapped as `{"value": ...}`.
fn explain_document(exit_code: i32, raw: &str) -> Value {
    if exit_code != 0 {
        return json!({"error": tail_chars(raw, EXPLAIN_ERROR_LIMIT)});
    }
    match serde_json::from_str::<Value>(raw) {
        Ok(document @ Value::Object(_)) => document,
        Ok(other) => json!({"value": other}),
        Err(_) => json!({"raw": tail_chars(&redact_evidence(raw), EXPLAIN_RAW_LIMIT)}),
    }
}

/// `encode("utf-8")[:limit]` then lossy decode; returns the text and the truncated byte length.
fn bounded_utf8(text: &str, limit: usize) -> (String, usize) {
    let bytes = text.as_bytes();
    let kept = &bytes[..bytes.len().min(limit)];
    (String::from_utf8_lossy(kept).into_owned(), kept.len())
}

/// `redact_evidence(value)`: `re.sub(r"(?i)(authorization|api[_-]?key|token|password|secret|cookie)\s*[:=]\s*\S+",
/// r"\1=<redacted>", value)[-32768:]`.
pub fn redact_evidence(value: &str) -> String {
    let characters: Vec<char> = value.chars().collect();
    let mut redacted = String::with_capacity(value.len());
    let mut index = 0;
    while index < characters.len() {
        match secret_assignment_at(&characters, index) {
            Some((keyword_end, match_end)) => {
                redacted.extend(&characters[index..keyword_end]);
                redacted.push_str("=<redacted>");
                index = match_end;
            }
            None => {
                redacted.push(characters[index]);
                index += 1;
            }
        }
    }
    tail_chars(&redacted, RECENT_LIMIT)
}

/// At `start`: keyword, `\s*`, `[:=]`, `\s*`, `\S+` → `(keyword_end, match_end)`.
fn secret_assignment_at(characters: &[char], start: usize) -> Option<(usize, usize)> {
    SECRET_KEYWORDS.iter().find_map(|keyword| {
        let keyword_end = keyword_match_end(characters, start, keyword)?;
        let separator = skip_while(characters, keyword_end, is_python_space);
        if !matches!(characters.get(separator), Some(':' | '=')) {
            return None;
        }
        let value_start = skip_while(characters, separator + 1, is_python_space);
        let value_end = skip_while(characters, value_start, |character| {
            !is_python_space(character)
        });
        (value_end > value_start).then_some((keyword_end, value_end))
    })
}

/// Case-insensitive keyword match; `?` in the pattern stands for the optional `[_-]` of `api[_-]?key`.
fn keyword_match_end(characters: &[char], start: usize, keyword: &str) -> Option<usize> {
    let mut position = start;
    for expected in keyword.chars() {
        if expected == '?' {
            if matches!(characters.get(position), Some('_' | '-')) {
                position += 1;
            }
            continue;
        }
        let actual = characters.get(position)?;
        if !actual.eq_ignore_ascii_case(&expected) {
            return None;
        }
        position += 1;
    }
    Some(position)
}

#[inline]
fn skip_while(characters: &[char], start: usize, predicate: impl Fn(char) -> bool) -> usize {
    let mut position = start;
    while characters
        .get(position)
        .is_some_and(|character| predicate(*character))
    {
        position += 1;
    }
    position
}

#[inline]
fn args(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_owned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_redacted_keeping_the_keyword_case() {
        assert_eq!(
            redact_evidence("Token: abc123 next"),
            "Token=<redacted> next"
        );
        assert_eq!(redact_evidence("API-KEY=xyz;q"), "API-KEY=<redacted>");
        assert_eq!(redact_evidence("apikey = v"), "apikey=<redacted>");
        assert_eq!(redact_evidence("password is x"), "password is x");
        assert_eq!(redact_evidence("secret=  "), "secret=  ");
        assert_eq!(redact_evidence("a cookie:c1 b"), "a cookie=<redacted> b");
    }

    #[test]
    fn utf8_bound_counts_truncated_bytes() {
        let (text, bytes) = bounded_utf8("aé", 2);
        assert_eq!((text.as_str(), bytes), ("a\u{fffd}", 2));
    }

    #[test]
    fn explain_falls_back_like_python() {
        assert_eq!(explain_document(1, "boom"), json!({"error": "boom"}));
        assert_eq!(explain_document(0, "[1]"), json!({"value": [1]}));
        assert_eq!(explain_document(0, "not json"), json!({"raw": "not json"}));
        assert_eq!(explain_document(0, "{\"a\":1}"), json!({"a": 1}));
    }
}
