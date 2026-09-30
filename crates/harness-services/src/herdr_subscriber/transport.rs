//! Connection to Herdr's socket without the Node helper: on Windows the named pipe
//! `\\.\pipe\<HERDR_SOCKET_PATH>` (socket-stream.mjs: `` `\\\\.\\pipe\\${socketPath}` ``, path inserted verbatim),
//! elsewhere the Unix socket at the path.
//!
//! Failure naming keeps subscriber.py's logs: on Windows the Node helper printed `error.code || error.name`
//! and exited 1, which Python logged as `subscriber.transport.exited` before the `EOFError` reconnect; on Unix
//! Python connected itself and logged the `OSError` subclass name.

use std::io;

use tokio::io::{AsyncRead, AsyncWrite};

/// Windows `ERROR_PIPE_BUSY` (libuv → `EBUSY`).
const ERROR_PIPE_BUSY: i32 = 231;

/// A connected, bidirectional byte stream.
pub trait HerdrConnection: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> HerdrConnection for T {}

/// The address socket-stream.mjs builds on Windows.
#[inline]
pub fn windows_pipe_address(socket_path: &str) -> String {
    format!(r"\\.\pipe\{socket_path}")
}

#[cfg(windows)]
pub async fn connect(socket_path: &str) -> io::Result<Box<dyn HerdrConnection>> {
    let client = tokio::net::windows::named_pipe::ClientOptions::new()
        .open(windows_pipe_address(socket_path))?;
    Ok(Box::new(client))
}

#[cfg(unix)]
pub async fn connect(socket_path: &str) -> io::Result<Box<dyn HerdrConnection>> {
    let stream = tokio::net::UnixStream::connect(socket_path).await?;
    Ok(Box::new(stream))
}

/// Node's `error.code || error.name` for a pipe failure (what `subscriber.transport.exited` carried).
pub fn node_error_code(error: &io::Error) -> &'static str {
    if error.raw_os_error() == Some(ERROR_PIPE_BUSY) {
        return "EBUSY";
    }
    match error.kind() {
        io::ErrorKind::NotFound => "ENOENT",
        io::ErrorKind::PermissionDenied => "EPERM",
        io::ErrorKind::ConnectionRefused => "ECONNREFUSED",
        io::ErrorKind::ConnectionReset => "ECONNRESET",
        io::ErrorKind::BrokenPipe => "EPIPE",
        io::ErrorKind::TimedOut => "ETIMEDOUT",
        _ => "Error",
    }
}

/// `type(exc).__name__` of the `OSError` Python's own Unix socket code raised.
pub fn python_os_error_name(error: &io::Error) -> &'static str {
    match error.kind() {
        io::ErrorKind::NotFound => "FileNotFoundError",
        io::ErrorKind::PermissionDenied => "PermissionError",
        io::ErrorKind::ConnectionRefused => "ConnectionRefusedError",
        io::ErrorKind::ConnectionReset => "ConnectionResetError",
        io::ErrorKind::BrokenPipe => "BrokenPipeError",
        io::ErrorKind::TimedOut => "TimeoutError",
        _ => "OSError",
    }
}
