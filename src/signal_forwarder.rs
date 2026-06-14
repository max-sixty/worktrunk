//! Forwards SIGINT/SIGTERM from `wt` to its foreground children.
//!
//! Unix-only — the whole module is `#[cfg(unix)]` at the `pub mod`
//! declaration in `lib.rs`. Windows has no process groups or POSIX
//! signals; `Cmd::stream` and the concurrent executor simply don't
//! forward there.
//!
//! `wt` runs many child processes. When the kernel delivers a tty-initiated
//! signal (Ctrl-C, hangup) to wt's foreground process group, every process
//! in that pgroup receives it directly — but `wt` isolates some children in
//! their own pgroups for clean teardown, and externally-delivered signals
//! (`kill -TERM <wt-pid>`) only reach `wt`. In both cases `wt` must
//! explicitly forward the signal to each child (or child-group) so the
//! whole tree shuts down.
//!
//! ## Two-phase setup
//!
//! Signal handlers must be installed *before* spawning children — otherwise
//! a SIGINT arriving mid-spawn would default-kill `wt` and orphan any
//! already-spawned children. But the listener can't start until the child
//! PIDs/PGIDs are known. So callers do:
//!
//! 1. [`ForegroundSignals::install`] — call before spawning. Queues any
//!    signal that arrives during spawn.
//! 2. [`ForegroundSignals::forward_to_pid`] / [`forward_to_pgids`] — call
//!    after spawn. Starts the listener; processes any queued signal
//!    immediately.
//! 3. [`ActiveForwarder::stop`] — call after waits return. Returns the
//!    user's *originating* signal (the first SIGINT/SIGTERM observed),
//!    which is what `wt`'s exit code should reflect even when the
//!    escalation chain ultimately killed each child with a later signal.
//!
//! [`forward_to_pgids`]: ForegroundSignals::forward_to_pgids
//!
//! ## Modes
//!
//! - **Single PID** ([`forward_to_pid`]) — `wt step <single-cmd>` and the
//!   `Cmd::stream` path. With `share_parent_pgroup=true`, the kernel
//!   already broadcasts tty signals to the child, so we deliver only as a
//!   single shot covering externally-delivered signals (no escalation,
//!   to avoid wedging an interactive child mid-tty-restore). With
//!   `share_parent_pgroup=false`, the child is in its own pgroup and we
//!   escalate SIGINT → SIGTERM → SIGKILL with grace windows.
//! - **Multi PGID** ([`forward_to_pgids`]) — `wt step <concurrent-alias>`.
//!   Forwards the user's signal once per pgroup; a second user signal
//!   SIGKILLs every still-live pgroup. No per-child escalation —
//!   cooperative children die from the user's signal so `wt`'s exit code
//!   reflects intent (130 on Ctrl-C, not 143 from a SIGTERM landed during
//!   a sub-second escalation grace window under CI scheduling latency).
//!   Stubborn children require a second Ctrl-C, matching `make` / `cargo`.
//!
//! [`forward_to_pid`]: ForegroundSignals::forward_to_pid
//!
//! Both modes record the first observed signal so `wt`'s exit code matches
//! what the user pressed, not whichever signal escalation ended up using.

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::thread;

use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::iterator::{Handle, Signals};

use crate::shell_exec::{forward_signal_to_pid, forward_signal_with_escalation};

/// Pre-spawn signal handler. Install before spawning any child so signals
/// arriving mid-spawn are queued rather than default-killing `wt`.
pub struct ForegroundSignals {
    signals: Signals,
}

impl ForegroundSignals {
    /// Register `wt`'s SIGINT/SIGTERM handler. Errors only if the
    /// `signal_hook` registration itself fails (extremely rare).
    pub fn install() -> std::io::Result<Self> {
        Ok(Self {
            signals: Signals::new([SIGINT, SIGTERM])?,
        })
    }

    /// Begin forwarding to a single child PID. See module docs for the
    /// `share_parent_pgroup` semantics.
    pub fn forward_to_pid(self, child_pid: i32, share_parent_pgroup: bool) -> ActiveForwarder {
        // First-write wins. Subsequent signals are ignored: single-child
        // escalation already walks SIGINT → SIGTERM → SIGKILL inside one
        // call, so re-pressing Ctrl-C wouldn't add anything actionable.
        self.run_listener(move |sig, originating| {
            if record_originating(originating, sig) {
                if share_parent_pgroup {
                    forward_signal_to_pid(child_pid, sig);
                } else {
                    forward_signal_with_escalation(child_pid, sig);
                }
            }
        })
    }

    /// Begin forwarding to N children's PGIDs. Forwards the same signal
    /// once on the first press; a second user signal SIGKILLs every
    /// still-live PGID immediately.
    ///
    /// No per-child escalation: cooperative children die from the user's
    /// signal, so `child.wait()` reports a cause of death that matches
    /// intent (e.g. `wt step <alias>` exits 130 on Ctrl-C, not 143 from a
    /// SIGTERM that landed during a 200 ms escalation grace window under
    /// CI scheduling latency). Stubborn children that ignore SIGINT
    /// require a second Ctrl-C — matching `make` / `cargo` behavior.
    pub fn forward_to_pgids(self, child_pgids: Vec<i32>) -> ActiveForwarder {
        let mut seen_once = false;
        self.run_listener(move |sig, originating| {
            record_originating(originating, sig);
            let signal_to_send = pgid_broadcast_signal(sig, seen_once);
            seen_once = true;
            for &pgid in &child_pgids {
                let _ = nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pgid), signal_to_send);
            }
        })
    }

    /// Common scaffolding: take ownership of the signal handle, spawn a
    /// listener thread that calls `body` on each received signal, and
    /// package the pieces into an [`ActiveForwarder`].
    fn run_listener(
        self,
        mut body: impl FnMut(i32, &AtomicI32) + Send + 'static,
    ) -> ActiveForwarder {
        let handle = self.signals.handle();
        let originating = Arc::new(AtomicI32::new(0));
        let listener = {
            let originating = Arc::clone(&originating);
            let mut signals = self.signals;
            thread::spawn(move || {
                for sig in signals.forever() {
                    body(sig, &originating);
                }
            })
        };
        ActiveForwarder {
            handle,
            listener,
            originating,
        }
    }
}

/// Record `sig` as the originating signal if nothing has been recorded yet.
/// Returns `true` when this call won the race (i.e., `sig` is the first
/// signal observed). The 0 sentinel is safe — POSIX signals are >= 1.
fn record_originating(slot: &AtomicI32, sig: i32) -> bool {
    slot.compare_exchange(0, sig, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

/// The signal to broadcast to every still-live pgroup on a user signal. The
/// first press forwards the user's own signal so cooperative children die with
/// the cause the user intended; any later press escalates to SIGKILL. The
/// listener registers only SIGINT and SIGTERM, so a non-SIGTERM press is
/// necessarily SIGINT.
fn pgid_broadcast_signal(sig: i32, already_signaled: bool) -> nix::sys::signal::Signal {
    use nix::sys::signal::Signal;
    if already_signaled {
        Signal::SIGKILL
    } else if sig == SIGTERM {
        Signal::SIGTERM
    } else {
        Signal::SIGINT
    }
}

/// Running signal-forwarder. Returned from
/// [`ForegroundSignals::forward_to_pid`] / [`ForegroundSignals::forward_to_pgids`].
/// Call [`stop`] after every child has been waited on.
///
/// [`stop`]: ActiveForwarder::stop
pub struct ActiveForwarder {
    handle: Handle,
    listener: thread::JoinHandle<()>,
    originating: Arc<AtomicI32>,
}

impl ActiveForwarder {
    /// Tear the listener down and return the user's originating signal,
    /// or `None` if no SIGINT/SIGTERM was received.
    pub fn stop(self) -> Option<i32> {
        // Closing the signal-hook handle unblocks `Signals::forever()`
        // so the listener thread returns; join it to avoid a leak.
        self.handle.close();
        let _ = self.listener.join();
        match self.originating.load(Ordering::SeqCst) {
            0 => None,
            sig => Some(sig),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nix::sys::signal::Signal;

    #[test]
    fn first_press_forwards_the_user_signal() {
        assert_eq!(pgid_broadcast_signal(SIGINT, false), Signal::SIGINT);
        assert_eq!(pgid_broadcast_signal(SIGTERM, false), Signal::SIGTERM);
    }

    #[test]
    fn later_press_escalates_to_sigkill() {
        assert_eq!(pgid_broadcast_signal(SIGINT, true), Signal::SIGKILL);
        assert_eq!(pgid_broadcast_signal(SIGTERM, true), Signal::SIGKILL);
    }
}
