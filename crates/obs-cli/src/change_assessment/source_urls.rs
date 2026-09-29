//! obs.py:426 `_source_urls`: research links recorded with a change assessment.

use serde_json::Value;

use crate::failure::{CliFailure, CliResult};
use crate::python_compat::truncate_chars;

const MAX_SOURCE_URLS: usize = 50;
const MAX_SOURCE_URL_CHARS: usize = 1200;
const MAX_REPORTED_URL_CHARS: usize = 120;
const ALLOWED_SCHEMES: [&str; 2] = ["http://", "https://"];

/// Trimmed, http(s)-only, de-duplicated in first-seen order, at most 50.
pub fn validated_source_urls(values: &[String]) -> CliResult<Vec<String>> {
    let mut accepted: Vec<String> = Vec::new();

    for value in values {
        let url = value.trim();
        if !has_http_scheme(url) {
            return Err(CliFailure::refusal(format!(
                "Source URL must use http or https: {}",
                truncate_chars(url, MAX_REPORTED_URL_CHARS)
            )));
        }
        // obs.py compares the untruncated URL against the stored (truncated) ones.
        if !accepted.iter().any(|existing| existing == url) {
            accepted.push(truncate_chars(url, MAX_SOURCE_URL_CHARS));
        }
    }

    accepted.truncate(MAX_SOURCE_URLS);
    Ok(accepted)
}

/// obs.py:491 `list(dict.fromkeys([*stored, *new]))[:50]`.
pub fn merge_source_urls(stored_json: &str, new_urls: &[String]) -> CliResult<Vec<String>> {
    let stored = parse_stored_list(stored_json)?;
    let mut merged: Vec<String> = Vec::new();

    for url in stored.into_iter().chain(new_urls.iter().cloned()) {
        if !merged.contains(&url) {
            merged.push(url);
        }
    }

    merged.truncate(MAX_SOURCE_URLS);
    Ok(merged)
}

/// A JSON list of strings stored by obs.py (`risk_flags`, `source_urls`).
pub fn parse_stored_list(stored_json: &str) -> CliResult<Vec<String>> {
    let parsed: Vec<Value> = serde_json::from_str(stored_json).map_err(|error| {
        CliFailure::Internal(anyhow::anyhow!(
            "invalid JSON list stored in the spool: {error}"
        ))
    })?;

    Ok(parsed
        .into_iter()
        .map(|item| match item {
            Value::String(text) => text,
            other => other.to_string(),
        })
        .collect())
}

#[inline]
fn has_http_scheme(url: &str) -> bool {
    let lowered_prefix: String = url.chars().take(8).collect::<String>().to_ascii_lowercase();
    ALLOWED_SCHEMES
        .iter()
        .any(|scheme| lowered_prefix.starts_with(scheme))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn trims_dedupes_and_validates_scheme() {
        let urls =
            validated_source_urls(&strings(&[" https://a ", "HTTP://b", "https://a"])).unwrap();
        assert_eq!(urls, strings(&["https://a", "HTTP://b"]));

        let refused = validated_source_urls(&strings(&["ftp://x"])).unwrap_err();
        assert!(
            matches!(refused, CliFailure::Refusal(message) if message == "Source URL must use http or https: ftp://x")
        );
    }

    #[test]
    fn merge_keeps_stored_order_first() {
        let merged = merge_source_urls(
            "[\"https://a\", \"https://b\"]",
            &strings(&["https://b", "https://c"]),
        )
        .unwrap();
        assert_eq!(merged, strings(&["https://a", "https://b", "https://c"]));
    }
}
