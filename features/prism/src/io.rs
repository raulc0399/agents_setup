//! Bounded writes for private Unix transports.
use std::io::{self, ErrorKind};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

pub(crate) fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| io::Error::new(ErrorKind::TimedOut, "Unix transport deadline exceeded"))
}

/// A blocking send may keep progressing past SO_SNDTIMEO; poll nonblocking sends instead.
pub(crate) fn write_with_deadline(
    stream: &UnixStream,
    bytes: &[u8],
    deadline: Instant,
) -> io::Result<usize> {
    if bytes.is_empty() {
        return Ok(0);
    }
    loop {
        remaining(deadline)?;
        // SAFETY: the descriptor is live and the byte slice remains valid for this call.
        let written = unsafe {
            libc::send(
                stream.as_raw_fd(),
                bytes.as_ptr().cast(),
                bytes.len(),
                libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
            )
        };
        if written >= 0 {
            return Ok(written as usize);
        }
        match io::Error::last_os_error() {
            error if error.kind() == ErrorKind::Interrupted => continue,
            error if error.kind() == ErrorKind::WouldBlock => {}
            error => return Err(error),
        }
        let timeout_ms = remaining(deadline)?.as_millis().clamp(1, i32::MAX as u128) as i32;
        let mut descriptor = libc::pollfd {
            fd: stream.as_raw_fd(),
            events: libc::POLLOUT,
            revents: 0,
        };
        // SAFETY: one valid pollfd is supplied; poll does not retain its pointer.
        let ready = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
        if ready == 0 {
            return Err(io::Error::new(
                ErrorKind::TimedOut,
                "Unix write deadline exceeded",
            ));
        }
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
}
