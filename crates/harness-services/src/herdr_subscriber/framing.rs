//! NDJSON framing of the Herdr event stream (subscriber.py `read_line`): one message per `\n`-terminated line,
//! at most 1 MiB per line (a line longer than that fails like Python's `ValueError`), read through a buffer
//! instead of one syscall per byte (deliberate deviation). Bytes after the last newline at EOF are dropped.

use tokio::io::{AsyncBufRead, AsyncBufReadExt};

/// subscriber.py raises once the line holds more than `1024 * 1024` bytes.
pub const MAX_LINE_BYTES: usize = 1024 * 1024;
const NEWLINE: u8 = b'\n';

#[derive(Debug)]
pub enum FrameError {
    /// Peer closed the stream (`EOFError("Herdr socket closed")`).
    Closed,
    /// A line exceeded [`MAX_LINE_BYTES`] (`ValueError("Herdr event exceeded 1 MiB")`).
    Oversized,
    /// Transport read failure.
    Io(std::io::Error),
}

// Mental model: fill the buffer → newline present? take up to it and stop : take all and keep going;
// the cap is checked against the accumulated line length at every step.
/// Reads the next line (without its `\n`) into `line`, replacing its previous content.
pub async fn read_frame<R>(reader: &mut R, line: &mut Vec<u8>) -> Result<(), FrameError>
where
    R: AsyncBufRead + Unpin,
{
    line.clear();
    loop {
        let (consumed, is_complete) = {
            let available = reader.fill_buf().await.map_err(FrameError::Io)?;
            if available.is_empty() {
                return Err(FrameError::Closed);
            }
            take_line_chunk(available, line)
        };
        reader.consume(consumed);
        if line.len() > MAX_LINE_BYTES {
            return Err(FrameError::Oversized);
        }
        if is_complete {
            return Ok(());
        }
    }
}

/// Appends the bytes before the first newline (or all of them); returns `(bytes consumed, line complete)`.
#[inline]
fn take_line_chunk(available: &[u8], line: &mut Vec<u8>) -> (usize, bool) {
    match available.iter().position(|byte| *byte == NEWLINE) {
        Some(end) => {
            line.extend_from_slice(&available[..end]);
            (end + 1, true)
        }
        None => {
            line.extend_from_slice(available);
            (available.len(), false)
        }
    }
}
