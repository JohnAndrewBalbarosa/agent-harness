//! interval_supervisor.py:116 `drain_stream`, fed the way `Popen(text=True, encoding="utf-8",
//! errors="replace")` feeds it: universal newlines (`\n`, `\r\n` and a bare `\r` each end a
//! line), lossy UTF-8 decoding, blank lines dropped after normalization.

use std::io::{self, BufRead, BufReader, Read};
use std::sync::mpsc::Sender;
use std::thread::{self, JoinHandle};

use crate::bounded_log::SharedLog;
use crate::line_normalizer::normalize_line;

/// Raw bytes kept per line before normalization; the rest of a pathological newline-free
/// stream is discarded so memory stays bounded (Python's `readline` had no cap).
const RAW_LINE_BYTE_FLOOR: usize = 1 << 20;
const MAX_UTF8_BYTES_PER_CHAR: usize = 4;

/// Starts a thread that drains `stream` into `destination` and reports on `finished` when the
/// stream closes (the Python thread's `join(timeout=2)` waits on that signal).
pub fn spawn_drain<R>(
    stream: R,
    destination: SharedLog,
    maximum_line_chars: usize,
    finished: Sender<()>,
) -> io::Result<JoinHandle<()>>
where
    R: Read + Send + 'static,
{
    thread::Builder::new()
        .name("interval-supervisor-drain".to_owned())
        .spawn(move || {
            drain_stream(stream, &destination, maximum_line_chars);
            // The supervisor may already have stopped waiting; nothing to report then.
            let _ = finished.send(());
        })
}

/// Mental model: read one universal-newline line → normalize → keep it when non-empty.
pub fn drain_stream<R: Read>(stream: R, destination: &SharedLog, maximum_line_chars: usize) {
    let mut reader = BufReader::new(stream);
    let mut raw_line = Vec::new();
    let byte_cap = raw_line_byte_cap(maximum_line_chars);

    // A read error ends the stream exactly like EOF did for Python's `iter(readline, "")`.
    while let Ok(true) = read_universal_line(&mut reader, &mut raw_line, byte_cap) {
        let decoded = String::from_utf8_lossy(&raw_line);
        let line = normalize_line(&decoded, maximum_line_chars);
        if !line.is_empty() {
            destination.push(line);
        }
    }
}

#[inline]
fn raw_line_byte_cap(maximum_line_chars: usize) -> usize {
    maximum_line_chars
        .saturating_mul(MAX_UTF8_BYTES_PER_CHAR)
        .saturating_add(RAW_LINE_BYTE_FLOOR)
}

/// Fills `line` with the next line (terminator excluded). Returns `false` at EOF with nothing read.
pub fn read_universal_line<R: BufRead>(
    reader: &mut R,
    line: &mut Vec<u8>,
    byte_cap: usize,
) -> io::Result<bool> {
    line.clear();
    let mut read_anything = false;

    loop {
        let available = match reader.fill_buf() {
            Ok(available) => available,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if available.is_empty() {
            return Ok(read_anything);
        }
        read_anything = true;

        match available
            .iter()
            .position(|byte| *byte == b'\n' || *byte == b'\r')
        {
            Some(terminator_index) => {
                let ended_with_carriage_return = available[terminator_index] == b'\r';
                append_capped(line, &available[..terminator_index], byte_cap);
                reader.consume(terminator_index + 1);
                if ended_with_carriage_return {
                    consume_following_newline(reader)?;
                }
                return Ok(true);
            }
            None => {
                let consumed = available.len();
                append_capped(line, available, byte_cap);
                reader.consume(consumed);
            }
        }
    }
}

/// `\r\n` is one terminator; like Python's newline decoder this waits for the next byte.
#[inline]
fn consume_following_newline<R: BufRead>(reader: &mut R) -> io::Result<()> {
    loop {
        match reader.fill_buf() {
            Ok(available) => {
                if available.first() == Some(&b'\n') {
                    reader.consume(1);
                }
                return Ok(());
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
}

#[inline]
fn append_capped(line: &mut Vec<u8>, bytes: &[u8], byte_cap: usize) {
    let room = byte_cap.saturating_sub(line.len());
    line.extend_from_slice(&bytes[..bytes.len().min(room)]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines_of(input: &[u8]) -> Vec<String> {
        let mut reader = BufReader::with_capacity(2, input);
        let mut line = Vec::new();
        let mut lines = Vec::new();
        while read_universal_line(&mut reader, &mut line, 1024).unwrap() {
            lines.push(String::from_utf8_lossy(&line).into_owned());
        }
        lines
    }

    #[test]
    fn splits_on_every_universal_newline() {
        assert_eq!(lines_of(b"a\nb\r\nc\rd"), ["a", "b", "c", "d"]);
        assert_eq!(lines_of(b"x\r\r\ny"), ["x", "", "y"]);
    }

    #[test]
    fn drain_normalizes_and_drops_blank_lines() {
        let destination = SharedLog::new(10);
        drain_stream(
            &b"  \n\x1b[31mred\x1b[0m\n\nbad \xff byte\n"[..],
            &destination,
            200,
        );
        assert_eq!(destination.snapshot(), ["red", "bad \u{fffd} byte"]);
    }

    #[test]
    fn overlong_raw_lines_are_capped() {
        let mut reader = BufReader::new(&b"abcdefgh\nz"[..]);
        let mut line = Vec::new();
        assert!(read_universal_line(&mut reader, &mut line, 3).unwrap());
        assert_eq!(line, b"abc");
        assert!(read_universal_line(&mut reader, &mut line, 3).unwrap());
        assert_eq!(line, b"z");
    }
}
