//! receiver.py:128 `_ExclusiveServer`: one receiver per port. On Windows a plain bind lets a second
//! process that sets `SO_REUSEADDR` (Python's `HTTPServer` default) share the port, so the socket
//! claims `SO_EXCLUSIVEADDRUSE` before binding. Elsewhere the default (no `SO_REUSEADDR`) already
//! refuses a second bind.

use std::io;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};

use socket2::{Domain, Protocol, Socket, Type};

/// Pending-connection queue (Python used 5; concurrent handling makes a deeper queue useful).
const LISTEN_BACKLOG: i32 = 128;

/// A non-blocking listener on `127.0.0.1:port` that no other process can share. `port` 0 picks a
/// free port (tests).
pub fn bind_exclusive(port: u16) -> io::Result<TcpListener> {
    // Mental model: socket → exclusive-use claim → bind loopback → listen → non-blocking std listener.
    let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    claim_exclusive_address(&socket)?;
    socket.bind(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)).into())?;
    socket.listen(LISTEN_BACKLOG)?;
    let listener: TcpListener = socket.into();
    listener.set_nonblocking(true)?;
    Ok(listener)
}

#[cfg(windows)]
fn claim_exclusive_address(socket: &Socket) -> io::Result<()> {
    use std::os::windows::io::AsRawSocket;

    /// WinSock `SOL_SOCKET`.
    const SOL_SOCKET: i32 = 0xffff;
    /// WinSock `SO_EXCLUSIVEADDRUSE` (`~SO_REUSEADDR`).
    const SO_EXCLUSIVEADDRUSE: i32 = !4;

    #[link(name = "ws2_32")]
    extern "system" {
        fn setsockopt(socket: usize, level: i32, name: i32, value: *const u8, length: i32) -> i32;
    }

    let enabled: i32 = 1;
    let raw_socket = usize::try_from(socket.as_raw_socket())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "socket handle out of range"))?;
    // SAFETY: `raw_socket` is a live socket owned by `socket` for the whole call; `value` points to a
    // properly aligned, initialised 4-byte BOOL and `length` is exactly its size.
    let status = unsafe {
        setsockopt(
            raw_socket,
            SOL_SOCKET,
            SO_EXCLUSIVEADDRUSE,
            std::ptr::from_ref(&enabled).cast::<u8>(),
            std::mem::size_of::<i32>() as i32,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(windows))]
#[inline]
fn claim_exclusive_address(_socket: &Socket) -> io::Result<()> {
    Ok(())
}
