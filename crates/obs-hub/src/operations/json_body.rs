//! Request bodies parsed the way Node's `JSON.parse` accepted them: invalid UTF-8 bytes become
//! U+FFFD and lone `\uD800`-`\uDFFF` escapes survive (as sentinels, see `js_values`), where a
//! strict parser would reject the whole batch.

use serde_json::Value;

use super::js_values::lone_surrogate_sentinel;

const HIGH_SURROGATES: std::ops::RangeInclusive<u32> = 0xD800..=0xDBFF;
const LOW_SURROGATES: std::ops::RangeInclusive<u32> = 0xDC00..=0xDFFF;

pub fn parse_javascript_json(body: &[u8]) -> Result<Value, serde_json::Error> {
    let text = String::from_utf8_lossy(body);
    serde_json::from_str(&with_lone_surrogates_as_sentinels(&text))
}

// Mental model:
// copy characters -> inside strings, look at each `\u` escape -> keep valid pairs and BMP escapes,
// replace lone surrogate escapes with their sentinel character
fn with_lone_surrogates_as_sentinels(text: &str) -> String {
    let characters: Vec<char> = text.chars().collect();
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    let mut is_inside_string = false;

    while index < characters.len() {
        let character = characters[index];
        if !is_inside_string {
            is_inside_string = character == '"';
            output.push(character);
            index += 1;
            continue;
        }
        match character {
            '"' => {
                is_inside_string = false;
                output.push(character);
                index += 1;
            }
            '\\' => index = copy_escape(&characters, index, &mut output),
            _ => {
                output.push(character);
                index += 1;
            }
        }
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

    if HIGH_SURROGATES.contains(&unit) && is_low_surrogate_escape_at(characters, start + 6) {
        output.extend(&characters[start..start + 12]);
        return start + 12;
    }

    if is_surrogate(unit) {
        output.push(lone_surrogate_sentinel(unit));
    } else {
        output.extend(&characters[start..start + 6]);
    }
    start + 6
}

/// The code unit of a well-formed `\uXXXX` escape at `start`, if there is one.
fn unicode_escape_at(characters: &[char], start: usize) -> Option<u32> {
    let escape = characters.get(start..start + 6)?;
    if escape[0] != '\\' || escape[1] != 'u' {
        return None;
    }
    let digits: String = escape[2..].iter().collect();
    u32::from_str_radix(&digits, 16).ok()
}

#[inline]
fn is_low_surrogate_escape_at(characters: &[char], start: usize) -> bool {
    unicode_escape_at(characters, start).is_some_and(|unit| LOW_SURROGATES.contains(&unit))
}

#[inline]
fn is_surrogate(unit: u32) -> bool {
    HIGH_SURROGATES.contains(&unit) || LOW_SURROGATES.contains(&unit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::js_values::{escape_lone_surrogates, postgres_text};

    #[test]
    fn keeps_valid_json_unchanged() {
        let body = br#"{"a":"\u00e9\ud83d\ude00\\u12","b":[1,"\""]}"#;
        let value = parse_javascript_json(body).unwrap();
        assert_eq!(value["a"], "\u{e9}\u{1F600}\\u12");
        assert_eq!(value["b"][1], "\"");
    }

    #[test]
    fn accepts_lone_surrogates_like_node() {
        let value = parse_javascript_json(br#"{"a":"x\ud800y","b":"\udc00"}"#).unwrap();
        let text = value["a"].as_str().unwrap();
        assert_eq!(postgres_text(text), "x\u{FFFD}y");
        assert_eq!(escape_lone_surrogates(text), "x\\ud800y");
        assert_eq!(
            escape_lone_surrogates(value["b"].as_str().unwrap()),
            "\\udc00"
        );
    }

    #[test]
    fn replaces_invalid_utf8_bytes() {
        let value = parse_javascript_json(b"{\"a\":\"x\xffy\"}").unwrap();
        assert_eq!(value["a"], "x\u{FFFD}y");
    }

    #[test]
    fn still_rejects_broken_json() {
        assert!(parse_javascript_json(b"{not json").is_err());
    }
}
