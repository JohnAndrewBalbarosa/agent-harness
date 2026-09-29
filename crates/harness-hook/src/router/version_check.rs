//! `_check_version(agent, payload, log)`: on session start, a payload `version` missing from
//! `<root>/adapters/<agent>/manifest.toml` `tested_versions` is logged (never blocked).
//!
//! Deviation: instead of tomllib this reads only the top-level `tested_versions = [ "..", '..' ]` array
//! (single or multi-line, comments allowed). A manifest it cannot read or parse is skipped, as Python skips
//! an unreadable or invalid manifest.

use std::path::Path;

use serde_json::{Map, Value};

use super::lifecycle_records;
use crate::events::{is_python_truthy, python_str};

const TESTED_VERSIONS_KEY: &str = "tested_versions";

pub fn warn_when_untested(
    harness_root: &Path,
    agent: &str,
    payload: &Map<String, Value>,
    log: &Path,
) {
    let Some(version) = payload
        .get("version")
        .filter(|value| is_python_truthy(value))
        .map(python_str)
    else {
        return;
    };
    let manifest_path = harness_root
        .join("adapters")
        .join(agent)
        .join("manifest.toml");
    let Ok(manifest) = std::fs::read_to_string(&manifest_path) else {
        return;
    };
    let Some(tested) = tested_versions(&manifest) else {
        return;
    };

    if !tested.contains(&version) {
        lifecycle_records::version_untested(log, agent, &version);
    }
}

// Mental model: scan top-level lines (stop at the first `[table]`) for `tested_versions =`, then parse the
// array from the text that follows. Missing key → empty list (Python `manifest.get(..., [])`).
/// `manifest.get("tested_versions", [])`; `None` when the value is not a readable string array.
pub fn tested_versions(manifest: &str) -> Option<Vec<String>> {
    let mut offset = 0;
    for line in manifest.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            return Some(Vec::new());
        }
        if let Some(after_equals) = key_assignment(trimmed) {
            let value_start = offset + (line.len() - after_equals.len());
            return parse_string_array(&manifest[value_start..]);
        }
        offset += line.len();
    }
    Some(Vec::new())
}

/// `tested_versions = <rest>` → `<rest>`.
fn key_assignment(line: &str) -> Option<&str> {
    let after_key = line.strip_prefix(TESTED_VERSIONS_KEY)?;
    let after_equals = after_key
        .trim_start_matches([' ', '\t'])
        .strip_prefix('=')?;
    Some(after_equals)
}

// Mental model: `[` → repeat { skip blanks/commas/comments; `]` ends; quoted string → push } .
fn parse_string_array(text: &str) -> Option<Vec<String>> {
    let mut rest = text.trim_start_matches([' ', '\t']).strip_prefix('[')?;
    let mut values = Vec::new();

    loop {
        rest = skip_separators(rest);
        let first = rest.chars().next()?;
        match first {
            ']' => return Some(values),
            '"' | '\'' => {
                let (value, remaining) = parse_string(rest, first)?;
                values.push(value);
                rest = remaining;
            }
            _ => return None,
        }
    }
}

fn skip_separators(mut text: &str) -> &str {
    loop {
        let trimmed = text.trim_start_matches([' ', '\t', '\r', '\n', ',']);
        let Some(comment) = trimmed.strip_prefix('#') else {
            return trimmed;
        };
        text = comment.find('\n').map_or("", |index| &comment[index..]);
    }
}

/// Basic (`"`, with `\"` / `\\` escapes) or literal (`'`) single-line string.
fn parse_string(text: &str, quote: char) -> Option<(String, &str)> {
    let body = &text[1..];
    let mut value = String::new();
    let mut characters = body.char_indices();

    while let Some((index, character)) = characters.next() {
        match character {
            '\n' => return None,
            c if c == quote => return Some((value, &body[index + 1..])),
            '\\' if quote == '"' => value.push(escaped_character(characters.next()?.1)?),
            other => value.push(other),
        }
    }
    None
}

#[inline]
fn escaped_character(character: char) -> Option<char> {
    match character {
        '"' => Some('"'),
        '\\' => Some('\\'),
        'n' => Some('\n'),
        't' => Some('\t'),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_manifest_array() {
        let manifest = "agent = \"claude\"\nevents = [\"Stop\"]\ntested_versions = [\"2.1.283\"]\nbinary = \"claude\"\n";
        assert_eq!(tested_versions(manifest), Some(vec!["2.1.283".to_owned()]));
    }

    #[test]
    fn reads_multiline_arrays_with_comments() {
        let manifest = "tested_versions = [\n  \"1.0\", # first\n  '2.0',\n]\n";
        assert_eq!(
            tested_versions(manifest),
            Some(vec!["1.0".to_owned(), "2.0".to_owned()])
        );
    }

    #[test]
    fn missing_key_is_an_empty_list_and_garbage_is_skipped() {
        assert_eq!(
            tested_versions("agent = \"x\"\n[extra]\ntested_versions = [\"9\"]\n"),
            Some(Vec::new())
        );
        assert_eq!(tested_versions("tested_versions = [1, 2]\n"), None);
    }
}
