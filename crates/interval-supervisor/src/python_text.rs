//! Python `str` predicates the supervisor relies on (`isspace`, `strip`, `isalnum`, regex `\w`,
//! `re.IGNORECASE` letter equality), expressed on `char`.

/// Python `str.isspace()` for one character: Unicode White_Space plus the ASCII information
/// separators U+001C..U+001F that CPython also treats as whitespace.
#[inline]
pub fn is_python_space(character: char) -> bool {
    character.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&character)
}

/// Python `str.strip()` with no arguments.
#[inline]
pub fn python_strip(text: &str) -> &str {
    text.trim_matches(is_python_space)
}

/// Python `str.isalnum()` for one character (approximated by Unicode Alphabetic | Numeric).
#[inline]
pub fn is_python_alnum(character: char) -> bool {
    character.is_alphanumeric()
}

/// Python `re` Unicode `\w`.
#[inline]
pub fn is_regex_word(character: char) -> bool {
    is_python_alnum(character) || character == '_'
}

/// `re.IGNORECASE` equality between a subject character and a lowercase ASCII pattern letter,
/// including the non-ASCII simple case folds sre applies to ASCII letters
/// (`ſ`→s, Kelvin `K`→k, `İ`/`ı`→i).
#[inline]
pub fn matches_ignoring_case(subject: char, pattern_lowercase: char) -> bool {
    let folded = match subject {
        '\u{17f}' => 's',
        '\u{212a}' => 'k',
        '\u{130}' | '\u{131}' => 'i',
        other => other.to_ascii_lowercase(),
    };
    folded == pattern_lowercase
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_matches_python_whitespace() {
        assert_eq!(python_strip("\u{1c} ok \t\n"), "ok");
        assert_eq!(python_strip("\u{a0}x\u{3000}"), "x");
    }

    #[test]
    fn ignore_case_handles_ascii_and_sre_folds() {
        assert!(matches_ignoring_case('E', 'e'));
        assert!(matches_ignoring_case('\u{212a}', 'k'));
        assert!(!matches_ignoring_case('x', 'e'));
    }
}
