//! interval_supervisor.py:24 `ANSI_ESCAPE` and :77 `normalize_line`: keep the final
//! carriage-return frame, remove terminal control sequences, strip, and cut to the limit.

use crate::python_text::python_strip;

const ESCAPE: char = '\u{1b}';
const BELL: char = '\u{7}';
const ELLIPSIS: char = '\u{2026}';

/// Mental model: last `\r` frame → drop ANSI escapes → Python `strip()` → cut to
/// `maximum - 1` characters plus `…` when longer than `maximum` (code points, like Python).
pub fn normalize_line(value: &str, maximum: usize) -> String {
    let frame = final_carriage_return_frame(value);
    let without_escapes = strip_ansi_escapes(&frame);
    let line = python_strip(&without_escapes);
    cut_to_limit(line, maximum)
}

/// `value.replace("\r\n", "\n").split("\r")[-1]`.
#[inline]
fn final_carriage_return_frame(value: &str) -> String {
    let unified = value.replace("\r\n", "\n");
    match unified.rsplit_once('\r') {
        Some((_, last)) => last.to_owned(),
        None => unified,
    }
}

#[inline]
fn cut_to_limit(line: &str, maximum: usize) -> String {
    if line.chars().count() <= maximum {
        return line.to_owned();
    }
    let mut cut: String = line.chars().take(maximum.saturating_sub(1)).collect();
    cut.push(ELLIPSIS);
    cut
}

/// `ANSI_ESCAPE.sub("", text)` for
/// `\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07]*(?:\x07|\x1b\\))`, scanning left to right; an escape
/// that matches neither form is kept verbatim.
pub fn strip_ansi_escapes(text: &str) -> String {
    let characters: Vec<char> = text.chars().collect();
    let mut kept = String::with_capacity(text.len());
    let mut index = 0;

    while index < characters.len() {
        let escape_end = match characters[index] {
            ESCAPE => escape_sequence_end(&characters, index),
            _ => None,
        };
        match escape_end {
            Some(end) => index = end,
            None => {
                kept.push(characters[index]);
                index += 1;
            }
        }
    }
    kept
}

/// Exclusive end of the escape sequence starting at `start` (which holds ESC), if it matches.
#[inline]
fn escape_sequence_end(characters: &[char], start: usize) -> Option<usize> {
    match characters.get(start + 1) {
        Some('[') => control_sequence_end(characters, start + 2),
        Some(']') => operating_system_command_end(characters, start + 2),
        _ => None,
    }
}

/// CSI: parameter bytes `[0-?]*`, intermediate bytes `[ -/]*`, one final byte `[@-~]`.
fn control_sequence_end(characters: &[char], body_start: usize) -> Option<usize> {
    let mut index = body_start;
    while characters
        .get(index)
        .is_some_and(|character| ('0'..='?').contains(character))
    {
        index += 1;
    }
    while characters
        .get(index)
        .is_some_and(|character| (' '..='/').contains(character))
    {
        index += 1;
    }
    characters
        .get(index)
        .filter(|character| ('@'..='~').contains(*character))
        .map(|_| index + 1)
}

/// OSC: greedy `[^\x07]*` then BEL, or (after backtracking) the LAST `ESC \` before the first BEL.
fn operating_system_command_end(characters: &[char], body_start: usize) -> Option<usize> {
    let body = characters.get(body_start..)?;
    if let Some(bell) = body.iter().position(|character| *character == BELL) {
        return Some(body_start + bell + 1);
    }
    body.windows(2)
        .rposition(|pair| pair[0] == ESCAPE && pair[1] == '\\')
        .map(|terminator| body_start + terminator + 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_final_progress_frame_and_strips_ansi() {
        assert_eq!(
            normalize_line("loading 90%\r\u{1b}[32mAll tests passed!\u{1b}[0m\n", 200),
            "All tests passed!"
        );
    }

    #[test]
    fn crlf_is_not_a_frame_break() {
        assert_eq!(normalize_line("done\r\n", 200), "done");
    }

    #[test]
    fn cuts_long_lines_with_an_ellipsis() {
        assert_eq!(normalize_line("abcdef", 4), "abc\u{2026}");
        assert_eq!(normalize_line("abcd", 4), "abcd");
        assert_eq!(normalize_line("abc", 1), "\u{2026}");
    }

    #[test]
    fn removes_operating_system_commands() {
        assert_eq!(strip_ansi_escapes("a\u{1b}]0;title\u{7}b"), "ab");
        assert_eq!(strip_ansi_escapes("a\u{1b}]8;;x\u{1b}\\b"), "ab");
    }

    #[test]
    fn keeps_unterminated_escapes() {
        assert_eq!(strip_ansi_escapes("a\u{1b}[12"), "a\u{1b}[12");
        assert_eq!(strip_ansi_escapes("\u{1b}x"), "\u{1b}x");
    }
}
