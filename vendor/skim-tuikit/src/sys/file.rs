use crate::Result;
use std::os::fd::BorrowedFd;
use std::time::Duration;

use crate::error::TuikitError;
use nix::sys::select;
use nix::sys::time::{TimeVal, TimeValLike};

fn duration_to_timeval(duration: Duration) -> TimeVal {
    let sec = duration.as_secs() * 1000 + (duration.subsec_millis() as u64);
    TimeVal::milliseconds(sec as i64)
}

pub fn wait_until_ready(fd: BorrowedFd, signal_fd: Option<BorrowedFd>, timeout: Duration) -> Result<()> {
    let mut timeout_spec = if timeout == Duration::new(0, 0) {
        None
    } else {
        Some(duration_to_timeval(timeout))
    };

    let mut fdset = select::FdSet::new();
    fdset.insert(fd);

    if let Some(f) = signal_fd {
        fdset.insert(f);
    }
    let n = select::select(None, &mut fdset, None, None, &mut timeout_spec)?;

    if n < 1 {
        Err(TuikitError::Timeout(timeout)) // this error message will be used in input.rs
    } else if fdset.contains(fd) {
        Ok(())
    } else {
        Err(TuikitError::Interrupted)
    }
}
