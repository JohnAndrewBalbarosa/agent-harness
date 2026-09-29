//! `core.jsonl.load_entries`, streamed: each JSON-object line is handed to the visitor as soon as it
//! is read; blank, malformed and non-object lines are skipped; an unreadable file yields nothing.
//! Invalid UTF-8 is replaced (Python opens with `errors="replace"`).

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use serde_json::{Map, Value};

const READ_BUFFER_BYTES: usize = 64 * 1024;

pub type Entry = Map<String, Value>;

/// Visits every JSON object line of `path` in file order. Memory is bounded by the longest line.
pub fn for_each_entry(path: &Path, mut visit: impl FnMut(Entry)) {
    let Ok(file) = File::open(path) else {
        return;
    };
    let mut reader = BufReader::with_capacity(READ_BUFFER_BYTES, file);
    let mut line = Vec::new();

    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        if let Some(entry) = parse_entry(&line) {
            visit(entry);
        }
    }
}

#[inline]
fn parse_entry(line: &[u8]) -> Option<Entry> {
    let text = String::from_utf8_lossy(line);
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(entry)) => Some(entry),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_blank_malformed_and_non_object_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        std::fs::write(&path, "{\"a\":1}\r\n\nnot json\n[1]\n{\"b\":2}").unwrap();
        let mut seen = Vec::new();
        for_each_entry(&path, |entry| {
            seen.push(entry.keys().next().cloned().unwrap())
        });
        assert_eq!(seen, ["a", "b"]);
    }

    #[test]
    fn missing_file_visits_nothing() {
        let mut visited = 0;
        for_each_entry(Path::new("/does/not/exist.jsonl"), |_| visited += 1);
        assert_eq!(visited, 0);
    }
}
