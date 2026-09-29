//! Environment every consumer sees (`core/config.py` `runtime_env` + `core/router/harness_hook.py` `_child_env`).
//!
//! - Runtime keys from `<HARNESS_HOME>/.env` are merged under the process environment (process wins, even when
//!   the process value is empty, exactly like `{**file_values, **os.environ}`).
//! - File values may reference environment variables as `%NAME%` (case-insensitive on Windows).
//! - `HERDR_BIN` is also exported as `HERDR_BIN_PATH`, the name herdr-bridge reads.
//! - On Windows the process variable names are upper-cased, as Python's `os.environ` does, so every consumer
//!   can look keys up by their canonical upper-case name.

use std::collections::BTreeMap;
use std::path::Path;

use crate::events::Event;

pub type EnvMap = BTreeMap<String, String>;

/// `RUNTIME_KEYS` (`core/config.py`); `HUB_DIR` is intentionally absent (removed upstream).
pub const RUNTIME_KEYS: [&str; 7] = [
    "OBS_ENDPOINT",
    "OTLP_PORT",
    "NOTIFY_RATE_LIMIT",
    "HERDR_BIN",
    "HARNESS_PYTHON",
    "ORB_UI",
    "CONTEXT_WARN_TOKENS",
];

/// `.env` next to the harness root.
pub const ENV_FILE_NAME: &str = ".env";

/// The process environment with Python-on-Windows key casing.
pub fn process_environment() -> EnvMap {
    std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .map(|(key, value)| (platform_key(&key), value))
        .collect()
}

// Mental model: read the file (missing/unreadable = empty) → keep non-empty runtime keys, expanded →
// overlay the whole process environment → derive HERDR_BIN_PATH.
/// `runtime_env(env_file, environ)`.
pub fn runtime_env(env_file: &Path, environ: &EnvMap) -> EnvMap {
    let file_values = read_env_file(env_file);

    let mut merged: EnvMap = file_values
        .into_iter()
        .filter(|(key, value)| is_runtime_key(key) && !value.is_empty())
        .map(|(key, value)| {
            let expanded = expand(&value, environ);
            (key, expanded)
        })
        .collect();
    merged.extend(
        environ
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );

    export_herdr_bin_path(merged)
}

/// `_child_env(env, event)`: the runtime env plus the event identity and the Python-child conventions
/// (`PYTHONUTF8=1`, `PYTHONPATH=<root>[;existing]`) that the Python router gave every consumer.
pub fn consumer_env(runtime: &EnvMap, event: &Event, harness_root: &Path) -> EnvMap {
    let mut child = with_python_child_conventions(runtime, harness_root);
    child.insert("AH_AGENT".into(), event.agent.clone());
    child.insert("AH_INSTANCE".into(), event.instance.clone());
    child.insert("AH_EVENT".into(), event.event.clone());
    child
}

/// `PYTHONUTF8=1` and `PYTHONPATH = os.pathsep.join(filter(None, [root, existing]))`.
pub fn with_python_child_conventions(env: &EnvMap, harness_root: &Path) -> EnvMap {
    let mut child = env.clone();
    let existing = child.get("PYTHONPATH").cloned().unwrap_or_default();
    let root_text = harness_root.to_string_lossy().into_owned();
    let python_path = [root_text, existing]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(PATH_LIST_SEPARATOR);

    child.insert("PYTHONUTF8".into(), "1".into());
    child.insert("PYTHONPATH".into(), python_path);
    child
}

/// `parse_env_text(text)`: `KEY=value` lines, `#` comments, one pair of matching quotes stripped.
pub fn parse_env_text(text: &str) -> Vec<(String, String)> {
    let mut values: Vec<(String, String)> = Vec::new();
    for raw in python_splitlines(text) {
        if let Some((key, value)) = parse_env_line(raw) {
            upsert(&mut values, key, value);
        }
    }
    values
}

/// `expand(value, environ)`: `%NAME%` → the variable's value, unknown names left verbatim.
pub fn expand(value: &str, environ: &EnvMap) -> String {
    let mut expanded = String::with_capacity(value.len());
    let mut rest = value;

    while let Some(start) = rest.find('%') {
        expanded.push_str(&rest[..start]);
        let candidate = &rest[start..];
        match percent_reference(candidate) {
            Some((name, consumed)) => {
                let replacement = lookup(environ, name).unwrap_or(&candidate[..consumed]);
                expanded.push_str(replacement);
                rest = &candidate[consumed..];
            }
            None => {
                expanded.push('%');
                rest = &candidate[1..];
            }
        }
    }

    expanded.push_str(rest);
    expanded
}

#[cfg(windows)]
const PATH_LIST_SEPARATOR: &str = ";";
#[cfg(not(windows))]
const PATH_LIST_SEPARATOR: &str = ":";

/// `read_text(encoding="utf-8")`; `OSError` (and, as a deviation, undecodable bytes) → no values.
fn read_env_file(env_file: &Path) -> Vec<(String, String)> {
    std::fs::read_to_string(env_file)
        .map(|text| parse_env_text(&text))
        .unwrap_or_default()
}

fn parse_env_line(raw: &str) -> Option<(String, String)> {
    let line = python_strip(raw);
    if line.is_empty() || line.starts_with('#') || !line.contains('=') {
        return None;
    }
    let (key, value) = line.split_once('=')?;
    Some((
        python_strip(key).to_owned(),
        unquote(python_strip(value)).to_owned(),
    ))
}

/// `value[1:-1]` when it is wrapped in one matching pair of `"` or `'`.
fn unquote(value: &str) -> &str {
    let bytes = value.as_bytes();
    let is_wrapped = bytes.len() >= 2
        && bytes[0] == bytes[bytes.len() - 1]
        && (bytes[0] == b'"' || bytes[0] == b'\'');
    if is_wrapped {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

/// Python dict assignment: a repeated key keeps its first position and takes the last value.
fn upsert(values: &mut Vec<(String, String)>, key: String, value: String) {
    match values.iter_mut().find(|(existing, _)| *existing == key) {
        Some(entry) => entry.1 = value,
        None => values.push((key, value)),
    }
}

/// Matches `%[A-Za-z_][A-Za-z0-9_]*%` at the start of `text`; returns the name and the matched length.
fn percent_reference(text: &str) -> Option<(&str, usize)> {
    let body = text.strip_prefix('%')?;
    let name_length = body
        .char_indices()
        .take_while(|(index, character)| is_name_character(*character, *index == 0))
        .map(|(_, character)| character.len_utf8())
        .sum::<usize>();
    let has_closing_percent = body[name_length..].starts_with('%');
    if name_length == 0 || !has_closing_percent {
        return None;
    }
    Some((&body[..name_length], name_length + 2))
}

#[inline]
fn is_name_character(character: char, is_first: bool) -> bool {
    character == '_' || character.is_ascii_alphabetic() || (!is_first && character.is_ascii_digit())
}

#[inline]
fn is_runtime_key(key: &str) -> bool {
    RUNTIME_KEYS.contains(&key)
}

fn export_herdr_bin_path(mut merged: EnvMap) -> EnvMap {
    let herdr_bin = merged
        .get("HERDR_BIN")
        .filter(|value| !value.is_empty())
        .cloned();
    let has_bridge_name = merged
        .get("HERDR_BIN_PATH")
        .is_some_and(|value| !value.is_empty());
    if let (Some(path), false) = (herdr_bin, has_bridge_name) {
        merged.insert("HERDR_BIN_PATH".into(), path);
    }
    merged
}

/// `os.environ.get(name)`: case-insensitive on Windows.
fn lookup<'a>(environ: &'a EnvMap, name: &str) -> Option<&'a str> {
    environ
        .get(name)
        .or_else(|| environ.get(&platform_key(name)))
        .map(String::as_str)
}

#[inline]
fn platform_key(key: &str) -> String {
    if cfg!(windows) {
        key.to_uppercase()
    } else {
        key.to_owned()
    }
}

/// Python `str.strip()` (Unicode whitespace plus the ASCII separators `\x1c`-`\x1f`).
pub fn python_strip(text: &str) -> &str {
    text.trim_matches(is_python_whitespace)
}

#[inline]
fn is_python_whitespace(character: char) -> bool {
    character.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&character)
}

/// Python `str.splitlines()` line boundaries.
fn python_splitlines(text: &str) -> impl Iterator<Item = &str> {
    let mut lines = Vec::new();
    let mut rest = text;
    while let Some(index) = rest.find(is_python_line_boundary) {
        lines.push(&rest[..index]);
        let boundary = rest[index..].chars().next().map_or(1, char::len_utf8);
        let crlf_extra = usize::from(rest[index..].starts_with("\r\n"));
        rest = &rest[index + boundary + crlf_extra..];
    }
    if !rest.is_empty() {
        lines.push(rest);
    }
    lines.into_iter()
}

#[inline]
fn is_python_line_boundary(character: char) -> bool {
    matches!(
        character,
        '\n' | '\r'
            | '\u{0b}'
            | '\u{0c}'
            | '\u{1c}'
            | '\u{1d}'
            | '\u{1e}'
            | '\u{85}'
            | '\u{2028}'
            | '\u{2029}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environ() -> EnvMap {
        [
            ("APPDATA", r"C:\Users\Juan dela Cruz\AppData\Roaming"),
            ("USERPROFILE", r"C:\Users\Juan dela Cruz"),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
    }

    fn runtime(text: &str, extra: &[(&str, &str)]) -> EnvMap {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        std::fs::write(&path, text).unwrap();
        let mut environ = environ();
        for (key, value) in extra {
            environ.insert((*key).to_owned(), (*value).to_owned());
        }
        runtime_env(&path, &environ)
    }

    #[test]
    fn file_values_reach_consumers_expanded() {
        let env = runtime(
            "OTLP_PORT=4399\nNOTIFY_RATE_LIMIT=2/30\nOBS_ENDPOINT=http://x:1\nHARNESS_PYTHON=%APPDATA%\\py.exe\n",
            &[],
        );
        assert_eq!(env["OTLP_PORT"], "4399");
        assert_eq!(env["NOTIFY_RATE_LIMIT"], "2/30");
        assert_eq!(env["OBS_ENDPOINT"], "http://x:1");
        assert_eq!(
            env["HARNESS_PYTHON"],
            r"C:\Users\Juan dela Cruz\AppData\Roaming\py.exe"
        );
        assert_eq!(env["USERPROFILE"], r"C:\Users\Juan dela Cruz");
    }

    #[test]
    fn process_environment_wins() {
        assert_eq!(
            runtime("OTLP_PORT=4399\n", &[("OTLP_PORT", "5000")])["OTLP_PORT"],
            "5000"
        );
    }

    #[test]
    fn herdr_bin_is_exported_under_the_bridge_name() {
        assert_eq!(
            runtime("HERDR_BIN=C:\\h\\herdr.exe\n", &[])["HERDR_BIN_PATH"],
            r"C:\h\herdr.exe"
        );
    }

    #[test]
    fn missing_file_returns_environment() {
        let environ: EnvMap = [("A".to_owned(), "1".to_owned())].into_iter().collect();
        assert_eq!(
            runtime_env(Path::new("does-not-exist.env"), &environ),
            environ
        );
    }

    #[test]
    fn unknown_keys_are_not_exported() {
        assert!(!runtime("SECRET=x\n", &[]).contains_key("SECRET"));
    }

    #[test]
    fn expansion_keeps_unknown_and_malformed_references() {
        let environ = environ();
        assert_eq!(expand("%NOPE%\\x", &environ), "%NOPE%\\x");
        assert_eq!(
            expand("100% %APPDATA%", &environ),
            r"100% C:\Users\Juan dela Cruz\AppData\Roaming"
        );
        assert_eq!(expand("%1A%", &environ), "%1A%");
    }

    #[test]
    fn parse_strips_quotes_comments_and_last_value_wins() {
        let parsed = parse_env_text("# c\nA=\"x\"\r\nB='y'\nnoequals\nA=z\n");
        assert_eq!(
            parsed,
            vec![
                ("A".to_owned(), "z".to_owned()),
                ("B".to_owned(), "y".to_owned())
            ]
        );
    }

    #[test]
    fn python_child_conventions_prefix_the_root() {
        let env: EnvMap = [("PYTHONPATH".to_owned(), "extra".to_owned())]
            .into_iter()
            .collect();
        let child = with_python_child_conventions(&env, Path::new("root"));
        assert_eq!(
            child["PYTHONPATH"],
            format!("root{PATH_LIST_SEPARATOR}extra")
        );
        assert_eq!(child["PYTHONUTF8"], "1");
    }
}
