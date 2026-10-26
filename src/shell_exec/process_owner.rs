//! One thread receives command interrupts and creates child processes.
//!
//! Startup blocks INT/TERM before application threads exist. Those threads and
//! dependency workers inherit the blocked mask; this owner restores the original
//! mask, which its children inherit. A signal cannot be dequeued to an unrelated
//! worker while the owner admits a child. If spawn temporarily blocks signals,
//! restoring its mask runs the pending handler before the admission check.
//!
//! Requests cover creation and admission, never child waits. Commands return to
//! their callers for reuse. The descriptor-creation lock remains inside each raw
//! spawn: callers must not hold it while waiting for an owner request.
//! Library users that do not initialize process startup keep direct spawning;
//! establishing inherited masks after arbitrary threads exist would be unsafe.

use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::process::Command;
use std::sync::{OnceLock, mpsc};

use nix::sys::signal::{SigSet, Signal};

type Request = Box<dyn FnOnce() + Send>;
static OWNER: OnceLock<io::Result<mpsc::Sender<Request>>> = OnceLock::new();

pub(super) fn init() -> io::Result<()> {
    OWNER
        .get_or_init(start)
        .as_ref()
        .map(|_| ())
        .map_err(copy_error)
}

fn copy_error(error: &io::Error) -> io::Error {
    io::Error::new(error.kind(), error.to_string())
}

fn start() -> io::Result<mpsc::Sender<Request>> {
    let original = SigSet::thread_get_mask()?;
    let mut blocked = original;
    blocked.add(Signal::SIGINT);
    blocked.add(Signal::SIGTERM);
    blocked.thread_set_mask()?;

    let result = (|| {
        let (sender, receiver) = mpsc::channel::<Request>();
        let (ready, started) = mpsc::channel();
        std::thread::Builder::new()
            .name("wt-process-owner".into())
            .spawn(move || {
                let restored = original.thread_set_mask();
                let running = restored.is_ok();
                if ready.send(restored).is_err() || !running {
                    return;
                }
                for request in receiver {
                    request();
                }
            })?;
        started
            .recv()
            .map_err(|error| io::Error::other(error.to_string()))??;
        Ok(sender)
    })();
    if result.is_err() {
        original.thread_set_mask()?;
    }
    result
}

pub(super) fn with_command<T: Send + 'static>(
    command: &mut Command,
    create: impl FnOnce(&mut Command) -> T + Send + 'static,
) -> io::Result<T> {
    let Some(owner) = OWNER.get() else {
        return Ok(create(command));
    };
    let owner = owner.as_ref().map_err(copy_error)?;
    let mut owned = std::mem::replace(command, Command::new(""));
    let (sender, receiver) = mpsc::channel();
    owner
        .send(Box::new(move || {
            // Keep the signal owner alive if a request panics, and put the
            // reusable command back before propagating that panic to its caller.
            let result = catch_unwind(AssertUnwindSafe(|| create(&mut owned)));
            let _ = sender.send((owned, result));
        }))
        .map_err(|error| io::Error::other(error.to_string()))?;
    let (returned, result) = receiver
        .recv()
        .map_err(|error| io::Error::other(error.to_string()))?;
    *command = returned;
    match result {
        Ok(result) => Ok(result),
        Err(panic) => resume_unwind(panic),
    }
}
