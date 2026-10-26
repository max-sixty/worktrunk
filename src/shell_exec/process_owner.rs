//! One thread receives command interrupts and creates child processes.
//!
//! Startup creates this owner with the original signal mask, then blocks
//! INT/TERM on the initializing thread before application workers exist. Those
//! workers inherit the blocked mask; the owner's children inherit the original
//! mask. A signal cannot be dequeued to an unrelated worker while the owner
//! admits a child. If spawn temporarily blocks signals, restoring its mask runs
//! the pending handler before the admission check.
//!
//! Requests cover creation and admission, never child waits. Commands return to
//! their callers for reuse. The descriptor-creation lock remains inside each raw
//! spawn: callers must not hold it while waiting for an owner request.
//! Library users that do not initialize process startup keep direct spawning;
//! establishing inherited masks after arbitrary threads exist would be unsafe.

use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::process::Command;
use std::sync::mpsc;

use nix::sys::signal::{SigSet, Signal};
use once_cell::sync::OnceCell;

type Request = Box<dyn FnOnce() + Send>;
static OWNER: OnceCell<mpsc::Sender<Request>> = OnceCell::new();

pub(super) fn init() -> io::Result<()> {
    OWNER.get_or_try_init(start).map(|_| ())
}

fn start() -> io::Result<mpsc::Sender<Request>> {
    let mut blocked = SigSet::thread_get_mask()?;
    blocked.add(Signal::SIGINT);
    blocked.add(Signal::SIGTERM);

    let (sender, receiver) = mpsc::channel::<Request>();
    std::thread::Builder::new()
        .name("wt-process-owner".into())
        .spawn(move || {
            for request in receiver {
                request();
            }
        })?;
    // No requests or application workers exist until initialization returns.
    // Spawn failure leaves our mask intact; mask failure drops the sender and
    // lets the unpublished owner exit.
    blocked.thread_set_mask()?;
    Ok(sender)
}

pub(super) fn with_command<T: Send + 'static>(
    command: &mut Command,
    create: impl FnOnce(&mut Command) -> T + Send + 'static,
) -> io::Result<T> {
    let Some(owner) = OWNER.get() else {
        return Ok(create(command));
    };
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;

    #[test]
    #[allow(unsafe_code)]
    fn panicking_request_restores_command_and_keeps_owner() {
        const CHILD_ENV: &str = "WORKTRUNK_TEST_PROCESS_OWNER_PANIC_CHILD";
        let mut signals = SigSet::empty();
        signals.add(Signal::SIGINT);
        signals.add(Signal::SIGTERM);
        if std::env::var_os(CHILD_ENV).is_none() {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "shell_exec::process_owner::tests::panicking_request_restores_command_and_keeps_owner",
                ])
                .env(CHILD_ENV, "1")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            // Keep the child's libtest harness thread out of signal delivery.
            // SAFETY: pthread_sigmask is async-signal-safe and only accesses
            // the signal set constructed before fork.
            unsafe {
                command.pre_exec(move || signals.thread_block().map_err(Into::into));
            }
            let output = crate::shell_exec::spawn(&mut command)
                .unwrap()
                .wait_with_output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(output.status.success(), "{stdout}\n{stderr}");
            assert!(stdout.contains(" 1 passed;"), "{stdout}");
            return;
        }

        signals.thread_unblock().unwrap();
        crate::shell_exec::init_startup().unwrap();
        let mut command = Command::new("sh");
        command.args(["-c", "test \"$1\" = after-panic", "owner-probe"]);
        let panic = catch_unwind(AssertUnwindSafe(|| {
            with_command(&mut command, |command| {
                command.arg("after-panic");
                std::panic::panic_any(std::thread::current().id());
            })
            .unwrap();
        }))
        .unwrap_err();
        let owner = *panic.downcast::<std::thread::ThreadId>().unwrap();
        assert_ne!(owner, std::thread::current().id());
        assert_eq!(
            with_command(&mut command, |_| std::thread::current().id()).unwrap(),
            owner
        );
        // The request's changes return even when it panics, and the same owner
        // still creates children with that reusable command.
        assert!(
            crate::shell_exec::spawn(&mut command)
                .unwrap()
                .wait()
                .unwrap()
                .success()
        );
    }
}
