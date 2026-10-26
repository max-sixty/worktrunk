//! Foreground commands share the caller's Unix job group, independently of stdin.
//!
//! The terminal delivers Ctrl-C and Ctrl-Z to the whole job, so wt and its
//! child both receive the key. Like make or cargo, and unlike a shell's
//! wait-and-cooperative-exit, wt treats its own receipt of SIGINT while a
//! child runs as cancellation: the child handles the key its own way and is
//! never signalled for it, but once it exits the command starts nothing more
//! and exits interrupted, whatever status the child chose. Receipt is the
//! evidence; a numeric exit status such as 130 never is. A child the command
//! hands the terminal to ([`TerminalInterrupt::ChildInput`]: a pager, an
//! `--execute` program, a custom subcommand, an alias's last serial step) owns
//! the key as input instead, and its exit status stands. SIGTERM addressed to
//! wt cancels its command and is forwarded to live direct children by PID;
//! SharedChild serializes those deliveries with reaping. No caller group is killed.
//! A group-wide TERM may also reach a child through this PID forwarding;
//! repeated TERM deliveries never trigger escalation here.
//! The process owner receives signals and performs foreground admission on the
//! same thread, so a handler cannot lag behind a spawn on a different thread.
//! Spawn return commits admission. Startup cancellation retires an uncommitted
//! child with TERM; its cleanup is best effort, while admitted children finish
//! native-key cleanup. TERM is cooperative, with no escalation.
//! Ignored inherited dispositions stay ignored. Outside active commands, signal
//! handlers retain native default behavior, including between pipeline steps.
//! A confirmed INT/TERM child exit wakes all current foreground scopes, so
//! parallel output waits also stop. Execution leases end with direct waits;
//! output observers remain through EOF and keep reading the scope's latched
//! SIGINT flag, so a key received at any point stops the remaining drain.
//! A command-operation guard retains cancellation across gaps between foreground
//! scopes without disabling idle native defaults. New scopes share that operation's
//! latch; dropping the guard makes the next operation independent.

use crate::shell_exec::TerminalInterrupt;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::thread;

use shared_child::{SharedChild, unix::SharedChildExt};
use signal_hook::iterator::{Handle, Signals};
use signal_hook::{
    SigId,
    consts::{SIGCONT, SIGINT, SIGTERM},
};

struct Activity {
    count: usize,
    idle: Arc<AtomicBool>,
    watched: Vec<i32>,
    scopes: Vec<Weak<Interruption>>,
    operation: Weak<OperationCancellation>,
}

// A typed child interruption cancels the current foreground operation,
// including parallel scopes whose direct children have already finished.
// Weak observers neither retain completed scopes nor carry cancellation into
// a later operation. Callbacks only wake waiters; they never run under Activity.
struct Interruption {
    signal: Arc<AtomicI32>,
    admission: AtomicI32,
    wake: OnceLock<Arc<dyn Fn() + Send + Sync>>,
}

impl Interruption {
    fn signal(&self) -> Option<i32> {
        match self.signal.load(Ordering::SeqCst) {
            0 => None,
            signal => Some(signal),
        }
    }
}

/// Publish a typed interruption accepted by the foreground command.
/// Capture callers use this when propagating cancellation, so aggregating
/// concurrent errors cannot discard cancellation behind an earlier diagnostic.
pub fn cancel_foreground(signal: i32) {
    let Some(Ok(state)) = ACTIVITY.get() else {
        return;
    };
    let scopes: Vec<_> = {
        let mut activity = state.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(operation) = activity.operation.upgrade() {
            let _ =
                operation
                    .signal
                    .compare_exchange(0, signal, Ordering::SeqCst, Ordering::SeqCst);
        }
        activity.scopes.retain(|scope| scope.strong_count() != 0);
        let scopes: Vec<_> = activity.scopes.iter().filter_map(Weak::upgrade).collect();
        for scope in &scopes {
            let _ = scope
                .signal
                .compare_exchange(0, signal, Ordering::SeqCst, Ordering::SeqCst);
        }
        scopes
    };
    for scope in scopes {
        if let Some(wake) = scope.wake.get() {
            wake();
        }
    }
}

static ACTIVITY: OnceLock<Result<Mutex<Activity>, String>> = OnceLock::new();

#[derive(Default)]
struct OperationCancellation {
    signal: Arc<AtomicI32>,
    terminated: Arc<AtomicBool>,
    /// Terminal SIGINT wt received while a [`TerminalInterrupt::Cancels`]
    /// scope ran. Those scopes register this flag; handoff scopes do not.
    interrupted: Arc<AtomicBool>,
}

impl OperationCancellation {
    fn signal(&self) -> Option<i32> {
        if self.terminated.load(Ordering::SeqCst) {
            Some(SIGTERM)
        } else {
            match self.signal.load(Ordering::SeqCst) {
                0 => self.interrupted.load(Ordering::SeqCst).then_some(SIGINT),
                signal => Some(signal),
            }
        }
    }
}

/// Own cancellation for one CLI command, including later parallel admissions.
/// This guard holds no execution lease: idle terminal signals keep native defaults.
pub struct CommandOperation {
    state: &'static Mutex<Activity>,
    cancellation: Arc<OperationCancellation>,
    _registration: Registration,
}

impl CommandOperation {
    pub fn start() -> std::io::Result<Self> {
        let state = Lease::state()?;
        let mut activity = state.lock().unwrap_or_else(|error| error.into_inner());
        if activity.operation.upgrade().is_some() {
            return Err(std::io::Error::other(
                "a command operation is already active",
            ));
        }
        let cancellation = Arc::new(OperationCancellation::default());
        let mut registration = Registration(Vec::new());
        if activity.watched.contains(&SIGTERM) {
            registration.0.push(signal_hook::flag::register(
                SIGTERM,
                cancellation.terminated.clone(),
            )?);
        }
        activity.operation = Arc::downgrade(&cancellation);
        Ok(Self {
            state,
            cancellation,
            _registration: registration,
        })
    }

    pub fn interrupt_signal(&self) -> Option<i32> {
        self.cancellation.signal()
    }
}

impl Drop for CommandOperation {
    fn drop(&mut self) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .operation = Weak::new();
    }
}

/// Cancellation of the command whose work is currently being admitted.
pub fn operation_interrupt_signal() -> Option<i32> {
    let state = ACTIVITY.get()?.as_ref().ok()?;
    let operation = state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .operation
        .upgrade()?;
    operation.signal()
}

struct Lease(&'static Mutex<Activity>);

impl Lease {
    fn state() -> std::io::Result<&'static Mutex<Activity>> {
        ACTIVITY
            .get_or_init(|| {
                let idle = Arc::new(AtomicBool::new(true));
                let watched = [SIGINT, SIGTERM]
                    .into_iter()
                    .filter_map(|signal| match ignored(signal) {
                        Ok(false) => Some(Ok(signal)),
                        Ok(true) => None,
                        Err(error) => Some(Err(error)),
                    })
                    .collect::<std::io::Result<Vec<_>>>()
                    .map_err(|error| error.to_string())?;
                for &signal in &watched {
                    signal_hook::flag::register_conditional_default(signal, idle.clone())
                        .map_err(|error| error.to_string())?;
                }
                Ok(Mutex::new(Activity {
                    count: 0,
                    idle,
                    watched,
                    scopes: Vec::new(),
                    operation: Weak::new(),
                }))
            })
            .as_ref()
            .map_err(|error| std::io::Error::other(error.clone()))
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        // Cleanup must restore defaults even if another owner panicked.
        let mut activity = self.0.lock().unwrap_or_else(|error| error.into_inner());
        activity.count -= 1;
        if activity.count == 0 {
            activity.idle.store(true, Ordering::SeqCst);
        }
    }
}

/// Query an inherited disposition without replacing its handler.
#[allow(unsafe_code)]
fn ignored(signal: i32) -> std::io::Result<bool> {
    let mut action = std::mem::MaybeUninit::<nix::libc::sigaction>::uninit();
    // SAFETY: null new-action is a read-only query; action is valid output storage.
    if unsafe { nix::libc::sigaction(signal, std::ptr::null(), action.as_mut_ptr()) } == -1 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: successful sigaction initialized the returned disposition.
    Ok(unsafe { action.assume_init() }.sa_sigaction == nix::libc::SIG_IGN)
}

struct Registration(Vec<SigId>);

impl Drop for Registration {
    fn drop(&mut self) {
        for id in self.0.drain(..) {
            signal_hook::low_level::unregister(id);
        }
    }
}

pub struct ForegroundSignals {
    signals: Signals,
    admission: Admission,
    terminal_interrupt: TerminalInterrupt,
    lease: Lease,
    registration: Registration,
}

#[derive(Clone)]
struct Admission {
    terminated: Arc<AtomicBool>,
    /// SIGINT received since install. A `Cancels` scope shares the operation's
    /// flag and keeps observing it while children run; a `ChildInput` scope
    /// consults its own flag only until spawn commits admission.
    interrupted: Arc<AtomicBool>,
    scope: Arc<Interruption>,
}

/// Physical child status and independent foreground cancellation.
pub struct ForegroundOutcome {
    pub status: std::process::ExitStatus,
    pub cancellation: Option<i32>,
}

impl ForegroundSignals {
    /// Install before spawning so startup signals remain queued.
    pub fn install(terminal_interrupt: TerminalInterrupt) -> std::io::Result<Self> {
        let state = Lease::state()?;
        // Fallible setup precedes admission, so an unwinding setup leaves
        // existing leases intact. Recover as the other Activity owners do.
        let mut activity = state.lock().unwrap_or_else(|error| error.into_inner());
        let operation = activity.operation.upgrade();
        let scope = Arc::new(Interruption {
            signal: operation
                .as_ref()
                .map(|operation| operation.signal.clone())
                .unwrap_or_default(),
            admission: AtomicI32::new(0),
            wake: OnceLock::new(),
        });
        let watched = &activity.watched;
        let terminated = operation
            .as_ref()
            .map(|operation| operation.terminated.clone())
            .unwrap_or_default();
        let interrupted = match (&operation, terminal_interrupt) {
            (Some(operation), TerminalInterrupt::Cancels) => operation.interrupted.clone(),
            _ => Arc::default(),
        };
        let mut registration = Registration(Vec::new());
        for &signal in watched {
            if signal == SIGTERM && operation.is_some() {
                // CommandOperation already registered this shared TERM flag.
                continue;
            }
            let flag = if signal == SIGTERM {
                terminated.clone()
            } else {
                interrupted.clone()
            };
            registration
                .0
                .push(signal_hook::flag::register(signal, flag)?);
        }
        // signal-hook creates its wake socket pair internally, with the same
        // macOS close-on-exec window as our own output sockets.
        let signals = crate::shell_exec::with_process_creation_guard(|| {
            Signals::new(watched.iter().copied())
        })?;
        // Admit only after handlers are ready. Before this point an idle job
        // keeps its native default, and an overlapping job latches startup keys.
        activity.scopes.retain(|scope| scope.strong_count() != 0);
        activity.scopes.push(Arc::downgrade(&scope));
        activity.count += 1;
        activity.idle.store(false, Ordering::SeqCst);
        drop(activity);
        Ok(Self {
            signals,
            admission: Admission {
                terminated,
                interrupted,
                scope,
            },
            terminal_interrupt,
            registration,
            lease: Lease(state),
        })
    }

    /// Admission commits when spawn returns. A cancelled in-flight child may
    /// have missed the native key; retire only that uncommitted child with TERM.
    /// Earlier admitted children finish native-key cleanup. Publish the admission
    /// origin before its teardown can report a different physical child signal.
    pub fn spawn(&self, command: &mut std::process::Command) -> anyhow::Result<SharedChild> {
        let admission = self.admission.clone();
        crate::shell_exec::with_process_owner(command, move |command| admission.spawn(command))?
    }

    /// Wait for the owned direct child and stop its signal listener on every path.
    pub fn wait(self, child: &Arc<SharedChild>) -> std::io::Result<ForegroundOutcome> {
        let forwarder = self.forward_to_children(vec![child.clone()], || {});
        let waited = crate::shell_exec::wait_foreground_child(child);
        if waited.is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let admission = forwarder.admission_interrupt_signal();
        let stopped = forwarder.stop();
        Ok(ForegroundOutcome {
            status: waited?,
            cancellation: stopped?.or(admission),
        })
    }

    pub fn forward_to_children(
        self,
        children: Vec<Arc<SharedChild>>,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> ActiveForwarder {
        let handle = self.signals.handle();
        let Self {
            mut signals,
            admission:
                Admission {
                    terminated,
                    interrupted,
                    scope,
                },
            terminal_interrupt,
            registration,
            lease,
        } = self;
        let wake = scope.wake.get_or_init(|| Arc::new(wake)).clone();
        if scope.signal().is_some() {
            wake();
        }
        let listener = thread::spawn(move || {
            for signal in signals.forever() {
                if signal == SIGTERM {
                    cancel_foreground(signal);
                    for child in &children {
                        let _ = child.send_signal(signal);
                        let _ = child.send_signal(SIGCONT);
                    }
                }
                // Wake output drains. Terminal INT stays with the children:
                // the drain observes the latched flag once they have exited.
                wake();
            }
        });
        ActiveForwarder {
            handle,
            listener: Some(listener),
            terminated,
            interrupted: (terminal_interrupt == TerminalInterrupt::Cancels).then_some(interrupted),
            scope,
            _registration: registration,
            _lease: Some(lease),
        }
    }
}

impl Admission {
    fn interrupt_signal(&self) -> Option<i32> {
        if self.terminated.load(Ordering::SeqCst) {
            Some(SIGTERM)
        } else {
            self.scope
                .signal()
                .or_else(|| self.interrupted.load(Ordering::SeqCst).then_some(SIGINT))
        }
    }

    fn spawn(&self, command: &mut std::process::Command) -> anyhow::Result<SharedChild> {
        if let Some(signal) = self.interrupt_signal() {
            self.cancel(signal);
            return Err(crate::git::WorktrunkError::Interrupted { signal, hint: None }.into());
        }
        match crate::shell_exec::spawn_shared_child_here(command) {
            Ok(child) => {
                if let Some(signal) = self.interrupt_signal() {
                    self.cancel(signal);
                    let _ = child.send_signal(SIGTERM);
                    let _ = child.send_signal(SIGCONT);
                } else if tracing::enabled!(target: crate::trace::WT_TRACE_TARGET, tracing::Level::DEBUG)
                {
                    crate::trace::instant(&format!("Foreground admitted:{}", child.id()));
                }
                Ok(child)
            }
            Err(error) => {
                if let Some(signal) = self.interrupt_signal() {
                    self.cancel(signal);
                    return Err(
                        crate::git::WorktrunkError::Interrupted { signal, hint: None }.into(),
                    );
                }
                Err(error.into())
            }
        }
    }

    fn cancel(&self, signal: i32) {
        let _ =
            self.scope
                .admission
                .compare_exchange(0, signal, Ordering::SeqCst, Ordering::SeqCst);
        cancel_foreground(signal);
    }
}

pub struct ActiveForwarder {
    handle: Handle,
    listener: Option<thread::JoinHandle<()>>,
    terminated: Arc<AtomicBool>,
    /// The scope's latched SIGINT, absent for a terminal handoff.
    interrupted: Option<Arc<AtomicBool>>,
    scope: Arc<Interruption>,
    _lease: Option<Lease>,
    _registration: Registration,
}

impl ActiveForwarder {
    /// The semantic startup cancellation, before any owned teardown signal.
    pub fn admission_interrupt_signal(&self) -> Option<i32> {
        match self.scope.admission.load(Ordering::SeqCst) {
            0 => None,
            signal => Some(signal),
        }
    }

    /// Release direct execution while continuing to observe output cancellation.
    pub fn finish_children(&mut self) {
        self._lease.take();
    }

    pub fn interrupt_signal(&self) -> Option<i32> {
        if self.terminated.load(Ordering::SeqCst) {
            Some(SIGTERM)
        } else {
            self.scope.signal().or_else(|| {
                if self
                    .interrupted
                    .as_ref()
                    .is_some_and(|flag| flag.load(Ordering::SeqCst))
                {
                    cancel_foreground(SIGINT);
                    Some(SIGINT)
                } else {
                    None
                }
            })
        }
    }

    pub fn stop(mut self) -> std::io::Result<Option<i32>> {
        self.handle.close();
        if let Some(listener) = self.listener.take() {
            listener
                .join()
                .map_err(|_| std::io::Error::other("foreground signal listener panicked"))?;
        }
        self._lease.take();
        Ok(self.interrupt_signal())
    }
}

impl Drop for ActiveForwarder {
    fn drop(&mut self) {
        self.handle.close();
        if let Some(listener) = self.listener.take() {
            let _ = listener.join();
        }
    }
}

/// Whether native re-raising is compatible with the inherited disposition.
pub fn should_reraise(signal: i32) -> std::io::Result<bool> {
    let activity = Lease::state()?
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    Ok(activity.watched.contains(&signal))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::ErrorExt;
    use crate::shell_exec::Cmd;

    /// Delay receipt before the flag callbacks, as when the kernel dequeues
    /// SIGINT onto a thread that is then descheduled. Admission must wait for
    /// that receipt instead of starting a child that missed the terminal key.
    #[test]
    #[allow(unsafe_code)]
    fn signal_receipt_and_child_admission_have_one_owner() {
        use nix::sys::signal::{SigSet, Signal, kill};
        use nix::unistd::Pid;
        use std::os::unix::process::CommandExt;
        use std::sync::mpsc;
        use std::time::{Duration, Instant};

        const CHILD_ENV: &str = "WORKTRUNK_TEST_SIGNAL_OWNER_CHILD";
        let mut signals = SigSet::empty();
        signals.add(Signal::SIGINT);
        signals.add(Signal::SIGTERM);
        if std::env::var_os(CHILD_ENV).is_none() {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "signal_forwarder::tests::signal_receipt_and_child_admission_have_one_owner",
                ])
                .env(CHILD_ENV, "1")
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            // The child libtest harness also creates a thread before entering
            // this test. Keep that harness thread out of signal delivery.
            // SAFETY: pthread_sigmask is async-signal-safe and uses only the
            // already-built signal set after fork.
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

        // This test thread stands in for main before it starts any workers.
        signals.thread_unblock().unwrap();
        crate::shell_exec::init_startup().unwrap();
        let operation = CommandOperation::start().unwrap();
        let entered = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        struct Release(Arc<AtomicBool>);
        impl Drop for Release {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let release_on_panic = Release(release.clone());
        let received = entered.clone();
        let released = release.clone();
        // SAFETY: the callback performs only lock-free atomic operations.
        let delayed = unsafe {
            signal_hook::low_level::register(SIGINT, move || {
                received.store(true, Ordering::SeqCst);
                while !released.load(Ordering::SeqCst) {
                    std::hint::spin_loop();
                }
            })
        }
        .unwrap();
        let foreground = ForegroundSignals::install(TerminalInterrupt::Cancels).unwrap();
        kill(Pid::this(), Signal::SIGINT).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while !entered.load(Ordering::SeqCst) {
            assert!(Instant::now() < deadline, "signal receiver did not run");
            thread::yield_now();
        }

        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("late-child");
        let child_marker = marker.clone();
        let (starting, started) = mpsc::channel();
        let admission = thread::spawn(move || {
            starting.send(()).unwrap();
            let mut command = std::process::Command::new("sh");
            command.args(["-c", "touch \"$1\"", "sh"]).arg(child_marker);
            let result = foreground.spawn(&mut command);
            // Always reap a child if the admission regression returns one.
            match result {
                Ok(child) => {
                    child.wait().unwrap();
                    None
                }
                Err(error) => error.interrupt_signal(),
            }
        });
        started.recv_timeout(Duration::from_secs(20)).unwrap();
        // Keep receipt delayed while the other thread attempts admission.
        thread::sleep(crate::testing::SLEEP_FOR_ABSENCE_CHECK);
        assert!(
            !marker.exists(),
            "child started before signal receipt finished"
        );
        drop(release_on_panic);
        assert_eq!(admission.join().unwrap(), Some(SIGINT));
        assert_eq!(operation.interrupt_signal(), Some(SIGINT));
        assert!(!marker.exists());
        signal_hook::low_level::unregister(delayed);

        // Plain captures share the owner too: their children must inherit the
        // original mask, and a reusable Command keeps its builder state.
        let mut command = std::process::Command::new("python3");
        command.args(["-c", "import signal; mask = signal.pthread_sigmask(signal.SIG_BLOCK, []); assert signal.SIGINT not in mask and signal.SIGTERM not in mask"]);
        for _ in 0..2 {
            assert!(
                crate::shell_exec::spawn(&mut command)
                    .unwrap()
                    .wait()
                    .unwrap()
                    .success()
            );
        }

        drop(operation);
        // A signal during spawn must retire an uncommitted child, or take
        // precedence over its exec error. Send TERM from pre_exec so receipt
        // falls between the owner's two admission checks in both cases.
        use std::os::unix::process::ExitStatusExt;
        let parent = Pid::this();
        for executable in [std::path::Path::new("sleep"), &temp.path().join("missing")] {
            let operation = CommandOperation::start().unwrap();
            let foreground = ForegroundSignals::install(TerminalInterrupt::Cancels).unwrap();
            assert_eq!(foreground.admission.interrupt_signal(), None);
            let mut command = std::process::Command::new(executable);
            command.arg("30");
            // SAFETY: kill is async-signal-safe; parent was captured before fork.
            unsafe {
                command.pre_exec(move || kill(parent, Signal::SIGTERM).map_err(Into::into));
            }
            match foreground.spawn(&mut command) {
                Ok(child) => {
                    assert_eq!(executable, std::path::Path::new("sleep"));
                    // No listener forwards TERM here: admission itself must
                    // retire this child, which missed the signal sent to wt.
                    assert_eq!(child.wait().unwrap().signal(), Some(SIGTERM));
                }
                Err(error) => {
                    assert_eq!(executable, temp.path().join("missing"));
                    assert_eq!(error.interrupt_signal(), Some(SIGTERM));
                }
            }
            assert_eq!(
                foreground.admission.scope.admission.load(Ordering::SeqCst),
                SIGTERM
            );
            assert_eq!(operation.interrupt_signal(), Some(SIGTERM));
            assert_eq!(
                foreground
                    .spawn(&mut command)
                    .unwrap_err()
                    .interrupt_signal(),
                Some(SIGTERM)
            );
        }
    }

    #[test]
    fn operation_cancellation_survives_scope_retirement_and_resets() {
        const CHILD_ENV: &str = "WORKTRUNK_OPERATION_LIFETIME_TEST_CHILD";
        if std::env::var_os(CHILD_ENV).is_none() {
            // Operation ownership is process-wide. Keep this real lifecycle
            // isolated even when cargo test runs other library tests in parallel.
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command.args([
                "--exact",
                "signal_forwarder::tests::operation_cancellation_survives_scope_retirement_and_resets",
            ]).env(CHILD_ENV, "1").stdout(std::process::Stdio::piped());
            let output = crate::shell_exec::spawn(&mut command)
                .unwrap()
                .wait_with_output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(output.status.success(), "{stdout}");
            // `--exact` matching nothing also exits 0; require that the child
            // actually ran this test.
            assert!(stdout.contains(" 1 passed;"), "{stdout}");
            return;
        }

        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("late-command-ran");
        let operation = CommandOperation::start().unwrap();
        let error = Cmd::new("python3")
            .args(["-c", "import os,signal; signal.signal(signal.SIGINT,signal.SIG_DFL); os.kill(os.getpid(),signal.SIGINT)"])
            .forward_signals()
            .stream()
            .unwrap_err();
        assert_eq!(error.interrupt_signal(), Some(SIGINT));
        assert_eq!(operation.interrupt_signal(), Some(SIGINT));

        // The first command and its signal scope have finished. A new scope
        // must still refuse work from the same cancelled operation.
        let error = Cmd::new("sh")
            .args(["-c", "touch \"$1\"", "sh", marker.to_str().unwrap()])
            .forward_signals()
            .stream()
            .unwrap_err();
        assert_eq!(error.interrupt_signal(), Some(SIGINT));
        assert!(!marker.exists());
        drop(operation);

        let next = CommandOperation::start().unwrap();
        Cmd::new("sh")
            .args(["-c", "touch \"$1\"", "sh", marker.to_str().unwrap()])
            .forward_signals()
            .stream()
            .unwrap();
        assert!(marker.exists());
        assert_eq!(next.interrupt_signal(), None);
    }
}
