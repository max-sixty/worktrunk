//! Foreground commands share the caller's Unix job group, independently of stdin.
//!
//! The terminal delivers Ctrl-C and Ctrl-Z to the whole job. A handled Ctrl-C
//! therefore leaves each child's own exit status intact. SIGTERM addressed to
//! wt cancels its command and is forwarded to live direct children by PID;
//! SharedChild serializes those deliveries with reaping. No caller group is killed.
//! A group-wide TERM may also reach a child through this PID forwarding;
//! repeated TERM deliveries never trigger escalation here.
//! Spawn return commits admission. Startup cancellation retires an uncommitted
//! child with TERM; its cleanup is best effort, while admitted children finish
//! native-key cleanup. TERM is cooperative, with no escalation.
//! Ignored inherited dispositions stay ignored. Outside active commands, signal
//! handlers retain native default behavior, including between pipeline steps.
//! A confirmed INT/TERM child exit wakes all current foreground scopes, so
//! parallel output waits also stop. Execution leases end with direct waits;
//! output observers remain through EOF and distinguish newly delivered keys
//! using a flag registered at that transition, rather than notification timing.
//! A command-operation guard retains cancellation across gaps between foreground
//! scopes without disabling idle native defaults. New scopes share that operation's
//! latch; dropping the guard makes the next operation independent.

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
}

impl OperationCancellation {
    fn signal(&self) -> Option<i32> {
        if self.terminated.load(Ordering::SeqCst) {
            Some(SIGTERM)
        } else {
            match self.signal.load(Ordering::SeqCst) {
                0 => None,
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
    terminated: Arc<AtomicBool>,
    interrupted: Arc<AtomicBool>,
    scope: Arc<Interruption>,
    lease: Lease,
    registration: Registration,
}

/// Physical child status and independent foreground cancellation.
pub struct ForegroundOutcome {
    pub status: std::process::ExitStatus,
    pub cancellation: Option<i32>,
}

impl ForegroundSignals {
    /// Install before spawning so startup signals remain queued.
    pub fn install() -> std::io::Result<Self> {
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
        let interrupted = Arc::new(AtomicBool::new(false));
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
            terminated,
            interrupted,
            scope,
            registration,
            lease: Lease(state),
        })
    }

    /// Synchronously latched startup cancellation, before a listener exists.
    pub fn interrupt_signal(&self) -> Option<i32> {
        if self.terminated.load(Ordering::SeqCst) {
            Some(SIGTERM)
        } else {
            self.scope
                .signal()
                .or_else(|| self.interrupted.load(Ordering::SeqCst).then_some(SIGINT))
        }
    }

    /// Admission commits when spawn returns. A cancelled in-flight child may
    /// have missed the native key; retire only that uncommitted child with TERM.
    /// Earlier admitted children finish native-key cleanup. Publish the admission
    /// origin before its teardown can report a different physical child signal.
    pub fn spawn(&self, command: &mut std::process::Command) -> anyhow::Result<SharedChild> {
        if let Some(signal) = self.interrupt_signal() {
            self.cancel_admission(signal);
            return Err(crate::git::WorktrunkError::Interrupted { signal, hint: None }.into());
        }
        match crate::shell_exec::spawn_shared_child(command) {
            Ok(child) => {
                if let Some(signal) = self.interrupt_signal() {
                    self.cancel_admission(signal);
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
                    self.cancel_admission(signal);
                    return Err(
                        crate::git::WorktrunkError::Interrupted { signal, hint: None }.into(),
                    );
                }
                Err(error.into())
            }
        }
    }

    fn cancel_admission(&self, signal: i32) {
        let _ =
            self.scope
                .admission
                .compare_exchange(0, signal, Ordering::SeqCst, Ordering::SeqCst);
        cancel_foreground(signal);
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
            terminated,
            interrupted: _,
            scope,
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
                // Wake output drains; a caught terminal INT still keeps the
                // child's normal exit status, including a successful exit.
                wake();
            }
        });
        ActiveForwarder {
            handle,
            listener: Some(listener),
            terminated,
            output_interrupted: None,
            scope,
            _registration: registration,
            _lease: Some(lease),
        }
    }
}

pub struct ActiveForwarder {
    handle: Handle,
    listener: Option<thread::JoinHandle<()>>,
    terminated: Arc<AtomicBool>,
    output_interrupted: Option<Arc<AtomicBool>>,
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
    /// Registering a fresh flag here excludes older, already handled Ctrl-C
    /// deliveries, even when their iterator notifications are still queued.
    pub fn finish_children(&mut self) -> std::io::Result<()> {
        if let Some(lease) = &self._lease {
            let watched = lease
                .0
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .watched
                .contains(&SIGINT);
            if watched {
                let flag = Arc::new(AtomicBool::new(false));
                self._registration
                    .0
                    .push(signal_hook::flag::register(SIGINT, flag.clone())?);
                self.output_interrupted = Some(flag);
            }
            self._lease.take();
        }
        Ok(())
    }

    pub fn interrupt_signal(&self) -> Option<i32> {
        if self.terminated.load(Ordering::SeqCst) {
            Some(SIGTERM)
        } else {
            self.scope.signal().or_else(|| {
                if self
                    .output_interrupted
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
            ]).env(CHILD_ENV, "1");
            assert!(
                crate::shell_exec::spawn(&mut command)
                    .unwrap()
                    .wait()
                    .unwrap()
                    .success()
            );
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
