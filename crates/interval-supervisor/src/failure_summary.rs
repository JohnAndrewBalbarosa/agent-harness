//! interval_supervisor.py:97 `select_failure_summary`: prefer a specific failure over trailing
//! aggregate or artifact chatter. Each line scores by its FIRST matching pattern; ties go to the
//! later line (`score >= best_score`).
//!
//! | score | Python pattern (re.IGNORECASE)                  |
//! |-------|--------------------------------------------------|
//! | 100   | `^error\s*:`                                     |
//! | 90    | `\b(exception\|fatal\|traceback)\b`              |
//! | 80    | `\b(timed out\|timeout\|assertion)\b`            |
//! | 20    | `\b(failed\|failure)\b`                          |

use crate::python_text::{is_python_space, is_regex_word, matches_ignoring_case};

const ERROR_PREFIX_SCORE: i32 = 100;
const RANKED_WORDS: [(i32, &[&str]); 3] = [
    (90, &["exception", "fatal", "traceback"]),
    (80, &["timed out", "timeout", "assertion"]),
    (20, &["failed", "failure"]),
];

/// Mental model: score every line → keep the best (latest on ties) → "" when nothing matched.
pub fn select_failure_summary(lines: &[String]) -> String {
    let mut best_score = -1;
    let mut best_line = "";

    for line in lines {
        if let Some(score) = line_score(line) {
            if score >= best_score {
                best_score = score;
                best_line = line.as_str();
            }
        }
    }
    best_line.to_owned()
}

fn line_score(line: &str) -> Option<i32> {
    let characters: Vec<char> = line.chars().collect();
    if starts_with_error_label(&characters) {
        return Some(ERROR_PREFIX_SCORE);
    }
    RANKED_WORDS
        .iter()
        .find(|(_, words)| words.iter().any(|word| contains_word(&characters, word)))
        .map(|(score, _)| *score)
}

/// `^error\s*:`.
fn starts_with_error_label(characters: &[char]) -> bool {
    let keyword: Vec<char> = "error".chars().collect();
    if !matches_at(characters, 0, &keyword) {
        return false;
    }
    characters[keyword.len()..]
        .iter()
        .find(|character| !is_python_space(**character))
        == Some(&':')
}

/// `\bword\b` anywhere in the line.
fn contains_word(characters: &[char], word: &str) -> bool {
    let pattern: Vec<char> = word.chars().collect();
    (0..characters.len()).any(|start| {
        matches_at(characters, start, &pattern)
            && is_word_boundary(characters, start)
            && is_word_boundary(characters, start + pattern.len())
    })
}

#[inline]
fn matches_at(characters: &[char], start: usize, pattern: &[char]) -> bool {
    characters
        .get(start..start + pattern.len())
        .is_some_and(|window| {
            window
                .iter()
                .zip(pattern)
                .all(|(subject, expected)| matches_ignoring_case(*subject, *expected))
        })
}

/// `\b` between `characters[position - 1]` and `characters[position]`.
#[inline]
fn is_word_boundary(characters: &[char], position: usize) -> bool {
    let before = position
        .checked_sub(1)
        .and_then(|index| characters.get(index))
        .is_some_and(|character| is_regex_word(*character));
    let after = characters
        .get(position)
        .is_some_and(|character| is_regex_word(*character));
    before != after
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn prefers_specific_error_over_aggregate() {
        assert_eq!(
            select_failure_summary(&lines(&[
                "Error: page context was destroyed",
                "trace.zip",
                "3 failed"
            ])),
            "Error: page context was destroyed"
        );
    }

    #[test]
    fn later_line_wins_ties_and_word_boundaries_apply() {
        assert_eq!(
            select_failure_summary(&lines(&["1 failed", "2 FAILED"])),
            "2 FAILED"
        );
        assert_eq!(select_failure_summary(&lines(&["unfailedness"])), "");
        assert_eq!(
            select_failure_summary(&lines(&["ERROR  : x", "Traceback (most recent call last)"])),
            "ERROR  : x"
        );
        assert_eq!(
            select_failure_summary(&lines(&["request timed out", "3 failures"])),
            "request timed out"
        );
    }

    #[test]
    fn error_prefix_needs_the_colon() {
        assert_eq!(select_failure_summary(&lines(&["errors found"])), "");
        assert_eq!(select_failure_summary(&lines(&[" error: x"])), "");
    }
}
