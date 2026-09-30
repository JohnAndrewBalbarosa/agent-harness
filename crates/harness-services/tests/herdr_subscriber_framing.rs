//! NDJSON framing over an in-memory reader (subscriber.py `read_line` limits).

use harness_services::herdr_subscriber::framing::{read_frame, FrameError, MAX_LINE_BYTES};
use harness_services::herdr_subscriber::reconnect::ReconnectBackoff;
use std::time::Duration;
use tokio::io::BufReader;

#[tokio::test]
async fn lines_are_split_on_newlines_across_small_buffers() {
    let input = b"{\"a\":1}\n\n{\"b\":2}\npartial".to_vec();
    let mut reader = BufReader::with_capacity(3, &input[..]);
    let mut line = Vec::new();

    read_frame(&mut reader, &mut line).await.unwrap();
    assert_eq!(line, b"{\"a\":1}");
    read_frame(&mut reader, &mut line).await.unwrap();
    assert_eq!(line, b"");
    read_frame(&mut reader, &mut line).await.unwrap();
    assert_eq!(line, b"{\"b\":2}");
    assert!(matches!(
        read_frame(&mut reader, &mut line).await,
        Err(FrameError::Closed)
    ));
}

#[tokio::test]
async fn a_line_of_exactly_one_mebibyte_is_accepted() {
    let mut input = vec![b'x'; MAX_LINE_BYTES];
    input.push(b'\n');
    let mut reader = BufReader::new(&input[..]);
    let mut line = Vec::new();
    read_frame(&mut reader, &mut line).await.unwrap();
    assert_eq!(line.len(), MAX_LINE_BYTES);
}

#[tokio::test]
async fn a_longer_line_is_rejected_with_or_without_its_newline() {
    for has_newline in [true, false] {
        let mut input = vec![b'x'; MAX_LINE_BYTES + 1];
        if has_newline {
            input.push(b'\n');
        }
        let mut reader = BufReader::new(&input[..]);
        let mut line = Vec::new();
        assert!(matches!(
            read_frame(&mut reader, &mut line).await,
            Err(FrameError::Oversized)
        ));
    }
}

#[test]
fn backoff_doubles_to_thirty_seconds_and_resets() {
    let mut backoff = ReconnectBackoff::new();
    let mut seen = Vec::new();
    for _ in 0..7 {
        seen.push(backoff.current().as_secs());
        backoff.advance();
    }
    assert_eq!(seen, [1, 2, 4, 8, 16, 30, 30]);
    backoff.reset();
    assert_eq!(backoff.current(), Duration::from_secs(1));
}
