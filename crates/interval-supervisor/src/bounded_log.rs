//! `deque(maxlen=...)` tails shared between a drain thread and the supervisor, plus
//! interval_supervisor.py:130 `persist_tail`.

use std::collections::VecDeque;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use anyhow::Context;
use obs_cli::python_compat::with_platform_newlines;

/// A tail that keeps only the newest `capacity` lines.
#[derive(Debug)]
pub struct BoundedLog {
    lines: VecDeque<String>,
    capacity: usize,
}

impl BoundedLog {
    pub fn new(capacity: usize) -> Self {
        Self {
            lines: VecDeque::new(),
            capacity,
        }
    }

    /// `deque.append` with `maxlen`: the oldest line falls off.
    pub fn push(&mut self, line: String) {
        if self.lines.len() == self.capacity {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }

    pub fn snapshot(&self) -> Vec<String> {
        self.lines.iter().cloned().collect()
    }
}

/// The drain thread appends; the supervisor snapshots at checkpoints and at the end.
#[derive(Debug, Clone)]
pub struct SharedLog(Arc<Mutex<BoundedLog>>);

impl SharedLog {
    pub fn new(capacity: usize) -> Self {
        Self(Arc::new(Mutex::new(BoundedLog::new(capacity))))
    }

    /// A poisoned lock only means a drain thread panicked mid-push; the tail is still usable.
    pub fn push(&self, line: String) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(line);
    }

    pub fn snapshot(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .snapshot()
    }
}

/// `persist_tail`: newline-joined lines plus a final newline, or an empty file; written in
/// Python text mode (`\r\n` on Windows).
pub fn persist_tail(path: &Path, lines: &[String]) -> anyhow::Result<()> {
    write_text(path, &tail_file_text(lines))
}

#[inline]
fn tail_file_text(lines: &[String]) -> String {
    let content = lines.join("\n");
    if content.is_empty() {
        content
    } else {
        format!("{content}\n")
    }
}

/// Python `Path.write_text(text, encoding="utf-8")`.
pub fn write_text(path: &Path, text: &str) -> anyhow::Result<()> {
    fs::write(path, with_platform_newlines(text))
        .with_context(|| format!("unable to write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_the_newest_lines() {
        let mut log = BoundedLog::new(3);
        for line in ["one", "two", "three", "SUCCESS"] {
            log.push(line.to_owned());
        }
        assert_eq!(log.snapshot(), ["two", "three", "SUCCESS"]);
    }

    #[test]
    fn persisted_text_matches_python() {
        assert_eq!(tail_file_text(&[]), "");
        assert_eq!(tail_file_text(&["a".to_owned(), "b".to_owned()]), "a\nb\n");
    }
}
