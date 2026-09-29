//! Bounded JSONL rewrite shared by `notify-events.jsonl` (1000 records) and the usage fallback's
//! `turn-usage.jsonl` (500 records): keep the newest non-blank lines, append one, rewrite through
//! `<stem>.tmp` + replace. Python writes in text mode, so lines end with `\r\n` on Windows.
//! The caller holds the file's cross-process lock.

use std::io;
use std::path::Path;

use obs_cli::python_compat::with_platform_newlines;

use super::state_files::temp_name_for;
use crate::shared_state::atomic_write::replace_file;

/// `kept = [non-blank lines][-(max_records - 1):] + [line]`, then `"\n".join(kept) + "\n"`.
pub fn append_keeping_newest(path: &Path, max_records: usize, line: &str) -> io::Result<()> {
    let existing = read_existing_text(path)?;
    let lines: Vec<&str> = python_lines(&existing)
        .filter(|candidate| !candidate.trim().is_empty())
        .collect();
    let keep_from = lines.len().saturating_sub(max_records.saturating_sub(1));

    let mut rendered = String::with_capacity(existing.len() + line.len() + 2);
    for kept in &lines[keep_from..] {
        rendered.push_str(kept);
        rendered.push('\n');
    }
    rendered.push_str(line);
    rendered.push('\n');

    let bytes = with_platform_newlines(&rendered);
    replace_file(path, &temp_name_for(path), bytes.as_bytes(), false)
}

/// `path.read_text("utf-8") if path.exists() else ""` (strict UTF-8, like Python).
#[inline]
fn read_existing_text(path: &Path) -> io::Result<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error),
    }
}

/// Universal-newline `splitlines()` for `\n`, `\r\n` and `\r`.
#[inline]
fn python_lines(text: &str) -> impl Iterator<Item = &str> {
    text.split('\n')
        .flat_map(|segment| segment.strip_suffix('\r').unwrap_or(segment).split('\r'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_newest_records_and_drops_blank_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.jsonl");
        std::fs::write(&path, "a\r\n\r\nb\nc\n").unwrap();
        append_keeping_newest(&path, 3, "d").unwrap();
        let text = std::fs::read_to_string(&path)
            .unwrap()
            .replace("\r\n", "\n");
        assert_eq!(text, "b\nc\nd\n");
        assert!(!dir.path().join("log.tmp").exists());
    }

    #[test]
    fn creates_the_file_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.jsonl");
        append_keeping_newest(&path, 10, "{}").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap().trim_end(), "{}");
    }
}
