//! `_payload(stdin)`: UTF-8 JSON object, empty stdin meaning `{}`; anything else is invalid.
//! Python's `json.loads` also accepts lone UTF-16 surrogate escapes (`"\ud800"`), which Claude and Codex
//! can emit when they truncate text; `serde_json` rejects them. A second, lenient pass replaces each lone
//! surrogate escape with U+FFFD so the event is still processed instead of being dropped.

use serde_json::{Map, Value};

const EMPTY_PAYLOAD: &str = "{}";
const REPLACEMENT_ESCAPE: &str = "\\ufffd";

pub fn parse_payload(stdin: &[u8]) -> Option<Map<String, Value>> {
    let text = std::str::from_utf8(stdin).ok()?;
    let text = if text.is_empty() { EMPTY_PAYLOAD } else { text };

    let value = serde_json::from_str::<Value>(text)
        .ok()
        .or_else(|| serde_json::from_str::<Value>(&with_lone_surrogates_replaced(text)).ok())?;
    match value {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

// Mental model:
// copy characters -> inside strings, check each `\uxxxx` escape -> keep surrogate pairs and BMP escapes,
// replace lone surrogate escapes with `\ufffd`
fn with_lone_surrogates_replaced(text: &str) -> String {
    let characters: Vec<char> = text.chars().collect();
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    let mut is_inside_string = false;

    while index < characters.len() {
        let character = characters[index];
        if is_inside_string && character == '\\' {
            index = copy_escape(&characters, index, &mut output);
            continue;
        }
        if character == '"' {
            is_inside_string = !is_inside_string;
        }
        output.push(character);
        index += 1;
    }
    output
}

/// Copies the escape starting at `start` (a backslash) and returns the index after it.
fn copy_escape(characters: &[char], start: usize, output: &mut String) -> usize {
    let Some(unit) = unicode_escape_at(characters, start) else {
        let end = (start + 2).min(characters.len());
        output.extend(&characters[start..end]);
        return end;
    };

    if is_high_surrogate(unit)
        && unicode_escape_at(characters, start + 6).is_some_and(is_low_surrogate)
    {
        output.extend(&characters[start..start + 12]);
        return start + 12;
    }
    if is_high_surrogate(unit) || is_low_surrogate(unit) {
        output.push_str(REPLACEMENT_ESCAPE);
    } else {
        output.extend(&characters[start..start + 6]);
    }
    start + 6
}

fn unicode_escape_at(characters: &[char], start: usize) -> Option<u32> {
    let escape = characters.get(start..start + 6)?;
    if escape[0] != '\\' || escape[1] != 'u' {
        return None;
    }
    let digits: String = escape[2..].iter().collect();
    u32::from_str_radix(&digits, 16).ok()
}

#[inline]
fn is_high_surrogate(unit: u32) -> bool {
    (0xD800..=0xDBFF).contains(&unit)
}

#[inline]
fn is_low_surrogate(unit: u32) -> bool {
    (0xDC00..=0xDFFF).contains(&unit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_stdin_is_an_empty_object() {
        assert_eq!(parse_payload(b""), Some(Map::new()));
    }

    #[test]
    fn malformed_non_object_and_non_utf8_are_invalid() {
        assert_eq!(parse_payload(b"{not json"), None);
        assert_eq!(parse_payload(b"[1]"), None);
        assert_eq!(parse_payload(b"  "), None);
        assert_eq!(parse_payload(&[0xff, b'{', b'}']), None);
    }

    #[test]
    fn keeps_key_order() {
        let map = parse_payload(br#"{"b": 1, "a": 2}"#).unwrap();
        assert_eq!(map.keys().collect::<Vec<_>>(), ["b", "a"]);
    }

    #[test]
    fn lone_surrogates_are_replaced_instead_of_dropping_the_event() {
        let map =
            parse_payload(br#"{"prompt": "x\ud800y", "pair": "\ud83d\ude00", "esc": "\\ud800"}"#)
                .unwrap();
        assert_eq!(map["prompt"], "x\u{FFFD}y");
        assert_eq!(map["pair"], "\u{1F600}");
        assert_eq!(map["esc"], "\\ud800");
    }
}
