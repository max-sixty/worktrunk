//! Unix pipe waits with a shared deadline and an explicit output cancellation fence.
//!
//! A cancelled reader consumes only the bytes queued when it observes the fence,
//! then closes: a surviving descendant cannot prolong cancellation by writing.
//! Ordinary completion retains EOF semantics. Deadlines cover I/O as well as exit.

use std::io::{self, Read};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::Instant;

use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};

pub fn set_nonblocking(fd: impl AsFd) -> io::Result<()> {
    let flags = OFlag::from_bits_truncate(fcntl(&fd, FcntlArg::F_GETFL)?);
    fcntl(&fd, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))?;
    Ok(())
}

/// Wake a nonblocking poll peer. A full buffer already guarantees readiness;
/// a retired peer needs no wake. Interrupted writes must retry the notification.
pub fn wake(fd: impl AsFd) {
    while let Err(nix::errno::Errno::EINTR) = nix::unistd::write(&fd, &[1]) {}
}

/// Retry interrupted polls against the original deadline, never extending it.
pub fn poll_until_ready(fds: &mut [PollFd<'_>], deadline: Option<Instant>) -> io::Result<()> {
    loop {
        let timeout = match deadline {
            Some(deadline) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "command timed out"));
                }
                PollTimeout::try_from(remaining).unwrap_or(PollTimeout::MAX)
            }
            None => PollTimeout::NONE,
        };
        match poll(fds, timeout) {
            Ok(0) => continue,
            Ok(_) => return Ok(()),
            Err(nix::errno::Errno::EINTR) => continue,
            Err(error) => return Err(error.into()),
        }
    }
}

pub struct PipeReader {
    stream: OwnedFd,
    deadline: Option<Instant>,
    cancel: Option<Arc<UnixStream>>,
    remaining: Option<u64>,
}

impl PipeReader {
    pub fn new(
        stream: impl Into<OwnedFd>,
        deadline: Option<Instant>,
        cancel: Option<Arc<UnixStream>>,
    ) -> io::Result<Self> {
        let stream = stream.into();
        set_nonblocking(&stream)?;
        Ok(Self {
            stream,
            deadline,
            cancel,
            remaining: None,
        })
    }

    /// Finish only the bytes already queued; future descendant output is excluded.
    pub fn cancel(&mut self) -> io::Result<()> {
        self.remaining = Some(rustix::io::ioctl_fionread(&self.stream)?);
        self.cancel.take();
        Ok(())
    }

    /// Read one nonblocking chunk for callers that already own the poll loop.
    pub fn read_ready(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let limit = self.remaining.map_or(bytes.len(), |remaining| {
            remaining.min(bytes.len() as u64) as usize
        });
        if limit == 0 {
            return Ok(0);
        }
        match nix::unistd::read(&self.stream, &mut bytes[..limit]).map_err(io::Error::from) {
            Ok(count) => {
                if let Some(remaining) = &mut self.remaining {
                    *remaining -= count as u64;
                }
                Ok(count)
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock && self.remaining.is_some() => {
                Ok(0)
            }
            Err(error) => Err(error),
        }
    }
}

impl AsFd for PipeReader {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.stream.as_fd()
    }
}

impl Read for PipeReader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        loop {
            if self.remaining == Some(0) {
                return Ok(0);
            }
            let mut fds = vec![PollFd::new(self.stream.as_fd(), PollFlags::POLLIN)];
            if let Some(cancel) = &self.cancel {
                fds.push(PollFd::new(cancel.as_fd(), PollFlags::POLLIN));
            }
            poll_until_ready(&mut fds, self.deadline)?;
            if fds
                .get(1)
                .is_some_and(|fd| !fd.revents().unwrap_or(PollFlags::empty()).is_empty())
            {
                self.cancel()?;
            }
            match self.read_ready(bytes) {
                Ok(count) => return Ok(count),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error),
            }
        }
    }
}
