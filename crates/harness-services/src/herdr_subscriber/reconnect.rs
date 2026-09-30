//! subscriber.py `main()` loop: connect → subscribe → read events until the stream fails → log
//! `subscriber.reconnect` → sleep (1 s doubling to 30 s, reset after every event) → again, forever.

use std::convert::Infallible;
use std::time::Duration;

use harness_hook::consumers::herdr_bridge::bridge_log::BridgeLog;
use serde_json::json;
use tokio::io::{AsyncWriteExt, BufReader};

use super::event_recorder::EventRecorder;
use super::framing::{read_frame, FrameError};
use super::subscription::{classify_line, new_request_id, subscribe_line, IncomingMessage};
use super::transport;

const READ_BUFFER_BYTES: usize = 64 * 1024;
/// The Node helper's exit code after a pipe error (`process.exitCode = 1`).
#[cfg_attr(not(windows), allow(dead_code))]
const TRANSPORT_EXIT_CODE: i32 = 1;

/// Python's `delay`: 1 s, doubled after each failure, capped at 30 s, back to 1 s after an event.
#[derive(Debug, Clone, PartialEq)]
pub struct ReconnectBackoff {
    delay: Duration,
}

impl ReconnectBackoff {
    pub const INITIAL: Duration = Duration::from_secs(1);
    pub const MAX: Duration = Duration::from_secs(30);

    pub fn new() -> Self {
        Self {
            delay: Self::INITIAL,
        }
    }

    pub fn current(&self) -> Duration {
        self.delay
    }

    /// `delay = min(30.0, delay * 2)`.
    pub fn advance(&mut self) {
        self.delay = (self.delay * 2).min(Self::MAX);
    }

    pub fn reset(&mut self) {
        self.delay = Self::INITIAL;
    }
}

impl Default for ReconnectBackoff {
    fn default() -> Self {
        Self::new()
    }
}

/// Why one connection ended; each maps to the Python exception that caused the reconnect.
#[derive(Debug)]
pub enum SessionEnd {
    /// Connect/read/write failure of the pipe (Node helper error → `EOFError`, Unix → `OSError` subclass).
    Transport(std::io::Error),
    /// Clean close (`EOFError`).
    Closed,
    /// Line over 1 MiB (`ValueError`).
    Oversized,
    /// Herdr answered the subscription with an error (`ValueError`).
    Rejected,
    /// Undecodable line (`JSONDecodeError`).
    Malformed,
}

impl SessionEnd {
    /// `type(exc).__name__` logged by `subscriber.reconnect`.
    pub fn python_error_name(&self) -> &'static str {
        match self {
            Self::Transport(error) => transport_error_name(error),
            Self::Closed => "EOFError",
            Self::Oversized | Self::Rejected => "ValueError",
            Self::Malformed => "JSONDecodeError",
        }
    }
}

#[cfg(windows)]
#[inline]
fn transport_error_name(_error: &std::io::Error) -> &'static str {
    "EOFError"
}

#[cfg(not(windows))]
#[inline]
fn transport_error_name(error: &std::io::Error) -> &'static str {
    transport::python_os_error_name(error)
}

pub struct SubscriberSession<'log> {
    socket_path: String,
    socket_hash: String,
    log: &'log BridgeLog,
    recorder: EventRecorder<'log>,
    backoff: ReconnectBackoff,
    /// Global event counter; never reset across reconnects.
    sequence: u64,
}

impl<'log> SubscriberSession<'log> {
    pub fn new(
        socket_path: &str,
        socket_hash: &str,
        log: &'log BridgeLog,
        recorder: EventRecorder<'log>,
    ) -> Self {
        Self {
            socket_path: socket_path.to_owned(),
            socket_hash: socket_hash.to_owned(),
            log,
            recorder,
            backoff: ReconnectBackoff::new(),
            sequence: 0,
        }
    }

    // Mental model: one connection until it ends → log why → wait the backoff → double it; never returns
    // (the caller cancels this future on stop).
    pub async fn reconnect_forever(&mut self) -> Infallible {
        loop {
            let end = self.stream_once().await;
            self.log_session_end(&end);
            let delay = self.backoff.current();
            tokio::time::sleep(delay).await;
            self.backoff.advance();
        }
    }

    // Mental model: connect → send the subscribe line → per line: reply (reject → end, else `connected`) or
    // event (reset backoff, next sequence, record).
    async fn stream_once(&mut self) -> SessionEnd {
        let connection = match transport::connect(&self.socket_path).await {
            Ok(connection) => connection,
            Err(error) => return SessionEnd::Transport(error),
        };
        let (read_half, mut write_half) = tokio::io::split(connection);
        let request_id = new_request_id();
        if let Err(error) = send_line(&mut write_half, &subscribe_line(&request_id)).await {
            return SessionEnd::Transport(error);
        }
        let mut reader = BufReader::with_capacity(READ_BUFFER_BYTES, read_half);
        let mut line = Vec::new();
        loop {
            if let Err(error) = read_frame(&mut reader, &mut line).await {
                return frame_failure(error);
            }
            match classify_line(&line, &request_id) {
                IncomingMessage::Ignored => {}
                IncomingMessage::Reply { is_rejected: true } => return SessionEnd::Rejected,
                IncomingMessage::Reply { is_rejected: false } => self.log.event(
                    "subscriber.connected",
                    "succeeded",
                    vec![("socket_hash", json!(self.socket_hash))],
                ),
                IncomingMessage::Event(message) => {
                    self.backoff.reset();
                    self.sequence += 1;
                    self.recorder.record(&message, self.sequence);
                }
                IncomingMessage::Malformed => return SessionEnd::Malformed,
            }
        }
    }

    /// Windows transport failures first log `subscriber.transport.exited` (the Node helper's exit), then every
    /// end logs `subscriber.reconnect` with the pending delay in seconds.
    fn log_session_end(&self, end: &SessionEnd) {
        if let SessionEnd::Transport(error) = end {
            self.log_transport_exit(error);
        }
        self.log.event(
            "subscriber.reconnect",
            "failed",
            vec![
                ("error", json!(end.python_error_name())),
                ("retry_seconds", json!(self.backoff.current().as_secs_f64())),
            ],
        );
    }

    #[cfg(windows)]
    #[inline]
    fn log_transport_exit(&self, error: &std::io::Error) {
        self.log.event(
            "subscriber.transport.exited",
            "failed",
            vec![
                ("exitCode", json!(TRANSPORT_EXIT_CODE)),
                ("error", json!(transport::node_error_code(error))),
            ],
        );
    }

    #[cfg(not(windows))]
    #[inline]
    fn log_transport_exit(&self, _error: &std::io::Error) {}
}

/// `writer.write(line); writer.flush()`.
#[inline]
async fn send_line<W>(writer: &mut W, line: &str) -> std::io::Result<()>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    writer.write_all(line.as_bytes()).await?;
    writer.flush().await
}

/// A broken pipe while reading is Herdr closing the stream (Node saw a normal close → `EOFError`).
#[inline]
fn frame_failure(error: FrameError) -> SessionEnd {
    match error {
        FrameError::Closed => SessionEnd::Closed,
        FrameError::Oversized => SessionEnd::Oversized,
        FrameError::Io(io) if io.kind() == std::io::ErrorKind::BrokenPipe => SessionEnd::Closed,
        FrameError::Io(io) => SessionEnd::Transport(io),
    }
}
