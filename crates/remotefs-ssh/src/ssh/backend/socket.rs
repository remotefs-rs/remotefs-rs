use std::io;
use std::mem::ManuallyDrop;

use socket2::Socket;

#[cfg(unix)]
#[cfg(test)]
pub(super) fn keepalive(socket: &impl std::os::fd::AsRawFd) -> io::Result<bool> {
    use std::os::fd::FromRawFd as _;

    // SAFETY: `socket` owns this valid descriptor for the duration of the borrow, and
    // `ManuallyDrop` prevents the temporary `Socket` from closing it.
    let socket = ManuallyDrop::new(unsafe { Socket::from_raw_fd(socket.as_raw_fd()) });
    socket.keepalive()
}

#[cfg(unix)]
pub(super) fn set_keepalive(socket: &impl std::os::fd::AsRawFd, keepalive: bool) -> io::Result<()> {
    use std::os::fd::FromRawFd as _;

    // SAFETY: `socket` owns this valid descriptor for the duration of the borrow, and
    // `ManuallyDrop` prevents the temporary `Socket` from closing it.
    let socket = ManuallyDrop::new(unsafe { Socket::from_raw_fd(socket.as_raw_fd()) });
    socket.set_keepalive(keepalive)
}

#[cfg(windows)]
#[cfg(test)]
pub(super) fn keepalive(socket: &impl std::os::windows::io::AsRawSocket) -> io::Result<bool> {
    use std::os::windows::io::FromRawSocket as _;

    // SAFETY: `socket` owns this valid handle for the duration of the borrow, and
    // `ManuallyDrop` prevents the temporary `Socket` from closing it.
    let socket = ManuallyDrop::new(unsafe { Socket::from_raw_socket(socket.as_raw_socket()) });
    socket.keepalive()
}

#[cfg(windows)]
pub(super) fn set_keepalive(
    socket: &impl std::os::windows::io::AsRawSocket,
    keepalive: bool,
) -> io::Result<()> {
    use std::os::windows::io::FromRawSocket as _;

    // SAFETY: `socket` owns this valid handle for the duration of the borrow, and
    // `ManuallyDrop` prevents the temporary `Socket` from closing it.
    let socket = ManuallyDrop::new(unsafe { Socket::from_raw_socket(socket.as_raw_socket()) });
    socket.set_keepalive(keepalive)
}
