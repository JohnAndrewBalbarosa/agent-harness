//! `observability.project.toml`: reading its `[project]` table and writing a new marker
//! exactly as `obs project register --init` does (obs.py:1374-1377).
//!
//! Deviation: instead of a full TOML parser (tomllib) this reads the subset markers use —
//! `[table]` headers and single-line `key = value` pairs whose values are basic/literal
//! strings, integers, floats, or booleans. Other syntax inside `[project]` is reported as an
//! error; other tables are skipped without validation.

use serde_json::{Number, Value};

use crate::python_compat::{python_float_repr, to_python_json, JsonLayout};

pub const MARKER_FILE_NAME: &str = "observability.project.toml";

#[derive(Debug, Clone, PartialEq)]
pub enum MarkerValue {
    Text(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
}

impl MarkerValue {
    /// Python `str(value)` for the loaded TOML value.
    pub fn to_python_str(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Integer(number) => number.to_string(),
            Self::Float(number) => python_float_repr(*number),
            Self::Boolean(true) => "True".to_owned(),
            Self::Boolean(false) => "False".to_owned(),
        }
    }

    pub fn to_json(&self) -> Value {
        match self {
            Self::Text(text) => Value::String(text.clone()),
            Self::Integer(number) => Value::from(*number),
            Self::Float(number) => Number::from_f64(*number).map_or(Value::Null, Value::Number),
            Self::Boolean(flag) => Value::Bool(*flag),
        }
    }
}

/// The `[project]` table in file order (Python dicts keep insertion order).
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectTable {
    entries: Vec<(String, MarkerValue)>,
}

impl ProjectTable {
    pub fn get(&self, key: &str) -> Option<&MarkerValue> {
        self.entries
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    pub fn to_json(&self) -> Value {
        let entries = self
            .entries
            .iter()
            .map(|(key, value)| (key.clone(), value.to_json()))
            .collect();
        Value::Object(entries)
    }
}

/// Mental model: track the current table header; collect `key = value` lines of `[project]`.
pub fn parse_project_table(text: &str) -> Result<ProjectTable, String> {
    let mut current_table: Option<String> = None;
    let mut entries = Vec::new();
    let mut saw_project_table = false;

    for (index, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim();

        if is_blank_or_comment(line) {
            continue;
        }

        if let Some(header) = table_header_name(line) {
            saw_project_table |= header == "project";
            current_table = Some(header);
            continue;
        }

        if current_table.as_deref() == Some("project") {
            let entry = parse_key_value(line)
                .map_err(|problem| format!("line {}: {problem}", index + 1))?;
            entries.push(entry);
        }
    }

    if !saw_project_table {
        return Err("no [project] table".to_owned());
    }
    Ok(ProjectTable { entries })
}

/// obs.py:1377 marker text; `json.dumps` strings are valid TOML basic strings. Python's
/// `write_text` translates `\n` to the platform line separator.
pub fn render_new_marker(project_id: &str, name: &str, slug: &str) -> String {
    let text = format!(
        "[project]\nid = \"{project_id}\"\nname = {}\nslug = {}\nschema_version = 1\n",
        python_json_string(name),
        python_json_string(slug),
    );

    if cfg!(windows) {
        text.replace('\n', "\r\n")
    } else {
        text
    }
}

#[inline]
fn python_json_string(value: &str) -> String {
    to_python_json(&Value::String(value.to_owned()), JsonLayout::Spaced, true)
}

#[inline]
fn is_blank_or_comment(line: &str) -> bool {
    line.is_empty() || line.starts_with('#')
}

/// `[name]` or `[[name]]` → `name` (quotes around the name are dropped).
fn table_header_name(line: &str) -> Option<String> {
    let inner = line
        .strip_prefix("[[")
        .and_then(|rest| rest.split_once("]]"))
        .or_else(|| line.strip_prefix('[').and_then(|rest| rest.split_once(']')))
        .map(|(name, _)| name.trim())?;
    Some(inner.trim_matches(|c| c == '"' || c == '\'').to_owned())
}

fn parse_key_value(line: &str) -> Result<(String, MarkerValue), String> {
    let (key, after_key) = parse_key(line)?;
    let after_equals = after_key
        .trim_start()
        .strip_prefix('=')
        .ok_or_else(|| "expected `=` after key".to_owned())?;
    let (value, remainder) = parse_value(after_equals.trim_start())?;

    if !is_blank_or_comment(remainder.trim()) {
        return Err("unexpected text after value".to_owned());
    }
    Ok((key, value))
}

fn parse_key(line: &str) -> Result<(String, &str), String> {
    if let Some(rest) = line.strip_prefix('"') {
        return parse_basic_string(rest);
    }
    if let Some(rest) = line.strip_prefix('\'') {
        return parse_literal_string(rest);
    }

    let end = line
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .unwrap_or(line.len());
    if end == 0 {
        return Err("expected a key".to_owned());
    }
    Ok((line[..end].to_owned(), &line[end..]))
}

fn parse_value(text: &str) -> Result<(MarkerValue, &str), String> {
    if text.starts_with("\"\"\"") || text.starts_with("'''") {
        return Err("multi-line strings are not supported".to_owned());
    }
    if let Some(rest) = text.strip_prefix('"') {
        return parse_basic_string(rest).map(|(value, rest)| (MarkerValue::Text(value), rest));
    }
    if let Some(rest) = text.strip_prefix('\'') {
        return parse_literal_string(rest).map(|(value, rest)| (MarkerValue::Text(value), rest));
    }

    let end = text
        .find(|c: char| c.is_whitespace() || c == '#')
        .unwrap_or(text.len());
    parse_scalar(&text[..end]).map(|value| (value, &text[end..]))
}

fn parse_scalar(token: &str) -> Result<MarkerValue, String> {
    match token {
        "true" => return Ok(MarkerValue::Boolean(true)),
        "false" => return Ok(MarkerValue::Boolean(false)),
        _ => {}
    }

    let digits: String = token.chars().filter(|c| *c != '_').collect();
    if let Ok(number) = digits.parse::<i64>() {
        return Ok(MarkerValue::Integer(number));
    }
    if looks_like_float(&digits) {
        if let Ok(number) = digits.parse::<f64>() {
            return Ok(MarkerValue::Float(number));
        }
    }
    Err(format!("unsupported value `{token}`"))
}

#[inline]
fn looks_like_float(token: &str) -> bool {
    token.contains(['.', 'e', 'E']) || matches!(token.trim_start_matches(['+', '-']), "inf" | "nan")
}

/// Body after the opening `"`; returns the decoded text and what follows the closing quote.
fn parse_basic_string(rest: &str) -> Result<(String, &str), String> {
    let mut decoded = String::new();
    let mut characters = rest.char_indices();

    while let Some((offset, character)) = characters.next() {
        match character {
            '"' => return Ok((decoded, &rest[offset + 1..])),
            '\\' => decoded.push(decode_escape(&mut characters)?),
            other => decoded.push(other),
        }
    }
    Err("unterminated string".to_owned())
}

fn decode_escape(characters: &mut std::str::CharIndices<'_>) -> Result<char, String> {
    let (_, escape) = characters
        .next()
        .ok_or_else(|| "unterminated escape".to_owned())?;

    match escape {
        'b' => Ok('\u{08}'),
        't' => Ok('\t'),
        'n' => Ok('\n'),
        'f' => Ok('\u{0c}'),
        'r' => Ok('\r'),
        '"' => Ok('"'),
        '\\' => Ok('\\'),
        'u' => decode_unicode_escape(characters, 4),
        'U' => decode_unicode_escape(characters, 8),
        other => Err(format!("invalid escape `\\{other}`")),
    }
}

fn decode_unicode_escape(
    characters: &mut std::str::CharIndices<'_>,
    width: usize,
) -> Result<char, String> {
    let digits: String = characters.by_ref().take(width).map(|(_, c)| c).collect();
    u32::from_str_radix(&digits, 16)
        .ok()
        .filter(|_| digits.len() == width)
        .and_then(char::from_u32)
        .ok_or_else(|| format!("invalid unicode escape `{digits}`"))
}

fn parse_literal_string(rest: &str) -> Result<(String, &str), String> {
    rest.split_once('\'')
        .map(|(value, after)| (value.to_owned(), after))
        .ok_or_else(|| "unterminated string".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_project_table_in_file_order() {
        let table = parse_project_table(
            "# marker\n[project]\nid = \"00000000-0000-7000-8000-000000000001\" # comment\nname = \"First \\u00e9\"\nslug='first'\nschema_version=1\n\n[other]\nx = [1, 2]\n",
        )
        .unwrap();

        assert_eq!(
            table.get("name"),
            Some(&MarkerValue::Text("First é".to_owned()))
        );
        assert_eq!(table.get("schema_version"), Some(&MarkerValue::Integer(1)));
        assert_eq!(
            serde_json::to_string(&table.to_json()).unwrap(),
            "{\"id\":\"00000000-0000-7000-8000-000000000001\",\"name\":\"First é\",\"slug\":\"first\",\"schema_version\":1}"
        );
    }

    #[test]
    fn missing_project_table_is_an_error() {
        assert!(parse_project_table("[other]\nid = 1\n").is_err());
    }

    #[test]
    fn unsupported_values_are_reported_with_line_numbers() {
        let problem = parse_project_table("[project]\nid = [1]\n").unwrap_err();
        assert!(problem.starts_with("line 2:"));
    }

    #[test]
    fn new_marker_round_trips_and_escapes_like_json_dumps() {
        let text = render_new_marker("id-1", "Café \"x\"", "caf-x");

        assert!(text.contains("name = \"Caf\\u00e9 \\\"x\\\"\""));
        let table = parse_project_table(&text).unwrap();
        assert_eq!(
            table.get("name"),
            Some(&MarkerValue::Text("Café \"x\"".to_owned()))
        );
        assert_eq!(
            table.get("id").map(MarkerValue::to_python_str),
            Some("id-1".to_owned())
        );
    }
}
