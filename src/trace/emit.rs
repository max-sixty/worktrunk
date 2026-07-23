//! Authoritative emitter for `[wt-trace]` records.
//!
//! Records emit as `tracing` events under [`WT_TRACE_TARGET`] with typed
//! structured fields; the logging layers in `src/logging.rs` render them two
//! ways. This module is the single source of truth for the fields — any field
//! change happens here.
//!
//! # Two renderings, decoupled
//!
//! - **Human** (`trace.log`, and stderr at `RUST_LOG=debug`): a readable line
//!   per record, where a command's start echo and finish record pair up —
//!   `✓ git status [worktree]  12.3ms` / `✗ …`, `· Showed skeleton`,
//!   `◷ build_hook_context  8.2ms`. Rendered by `logging.rs::format_wt_trace`.
//! - **Machine** (`trace.jsonl`): one JSON object per record, carrying every
//!   field — `{"kind":"cmd_completed","ts":…,"tid":…,"seq":…,"cmd":…,"dur_us":…,"ok":…}`.
//!   Rendered by `logging.rs::event_json`, and parsed back by [`super::parse`]
//!   and the `wt-perf` binary. This is the sole format any tool reads.
//!
//! Keeping the human and machine formats separate means the readable lines
//! never carry `key=value` clutter and a parser never reads them.
//!
//! # Fields
//!
//! Typed structured fields on each event: `kind`, `ts`, `tid`, `seq`, `cmd`,
//! `dur_us`, `ok`, `err`, `event`, `span`, `context`, `stdin`. `seq` is a
//! process-global monotonic command counter (command records only); the same
//! value is printed into the per-command header in `subprocess.log`, so a raw
//! output block there joins back to its command record via the `seq` field in
//! `trace.jsonl`. `stdin` (command records, [`CommandTrace::reads_stdin`])
//! marks a command that consumed stdin the `cmd` string doesn't capture, so the
//! cache analysis in `super::profile` can skip it — `trace.jsonl` carries it;
//! the human line omits it.
//!
//! This split — structured fields at the emission site, rendering at the
//! layer — means emit sites carry no string-formatting noise.
//!
//! # The subprocess chokepoint: [`CommandTrace`]
//!
//! Every subprocess command record (`cmd=…`) is emitted by exactly one type:
//! [`CommandTrace`]. The underlying `command_completed` / `command_errored`
//! writers are private to this module, so the *only* way to produce a command
//! record is to construct a `CommandTrace` and resolve it with `complete` /
//! `fail`. This is deliberate: subprocesses are spawned from many places
//! (`shell_exec::Cmd::{run, stream, delayed_stream, pipe_into}`, the
//! concurrent-command runner, pipeline steps, `wt step tether`, the fsmonitor
//! daemon launch), and before this chokepoint each path emitted — or, more
//! often, silently *forgot* to emit — its own record. Routing them all through
//! one guard means a path either traces or doesn't compile a record at all,
//! rather than skipping tracing by omission.
//!
//! `CommandTrace` is `#[must_use]` and asserts on drop that it was resolved
//! (debug builds only), so a future spawn site that constructs a guard but
//! forgets to call `complete`/`fail` trips in tests rather than leaving an
//! unattributed gap in the timeline.
//!
//! # Timing
//!
//! In-process spans (everything that isn't a subprocess) use [`Span`], an
//! RAII guard that captures `ts` at construction and emits the completed
//! record on drop with the elapsed duration. Use it to attribute time spent
//! in code paths subprocess records can't see (config load, repo open,
//! template render).
//!
//! Subprocess timing lives in [`CommandTrace`], which captures
//! `Instant::now()` just before the spawn — `tracing` spans can't carry this
//! across the sync subprocess wait, so the timing stays manual. The duration
//! is snapshotted at the moment `complete`/`fail` is called (the command's
//! resolution point), so a streamed command whose duration isn't known until
//! the stream finishes still records the correct spawn → wait span.
//!
//! # Routing
//!
//! Events emit at `tracing::DEBUG`, so `-vv` or `RUST_LOG=debug` makes them
//! visible. Subprocess stdout/stderr continuations route through separate
//! targets: the full output goes to `subprocess.log`, and a bounded preview
//! shares the routing of all other records — `trace.log` at `-vv`, stderr
//! otherwise — so raw bodies don't spam `-vv`. Each block in `subprocess.log`
//! is prefixed with a `$ cmd … seq=N` header (see `shell_exec::log_output`)
//! so it stays segmentable and joins back to this record via `seq`.

use std::borrow::Cow;
use std::fmt::Display;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Tracing target the `trace.log` layer keys on to render the `[wt-trace]`
/// grammar. Events under any other target fall through to the layer's
/// default message-passing format.
pub const WT_TRACE_TARGET: &str = "worktrunk::wt_trace";

/// Monotonic epoch for trace timestamps. All `ts` fields are microseconds
/// since this point. `Instant` is monotonic even if the system clock steps.
static TRACE_EPOCH: OnceLock<Instant> = OnceLock::new();

/// The monotonic epoch all trace timestamps are relative to.
pub fn trace_epoch() -> Instant {
    *TRACE_EPOCH.get_or_init(Instant::now)
}

/// Microseconds since [`trace_epoch`]. Use as the `ts` field for records.
pub fn now_us() -> u64 {
    Instant::now().duration_since(trace_epoch()).as_micros() as u64
}

/// Numeric thread id, extracted from `ThreadId`'s `Debug` representation.
/// `ThreadId` debug format is `ThreadId(N)`.
pub fn thread_id() -> u64 {
    let thread_id = std::thread::current().id();
    let debug_str = format!("{:?}", thread_id);
    debug_str
        .strip_prefix("ThreadId(")
        .and_then(|s| s.strip_suffix(")"))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Process-global monotonic command counter. Each [`CommandTrace`] claims the
/// next value at construction; the same `seq` is recorded in the `trace.jsonl`
/// record and the per-command header in `subprocess.log`, so a raw output block
/// can be joined back to its command record. Claimed at
/// construction (outside any verbosity gate) so the sequence is dense
/// regardless of whether the file layers are active. Starts at 1.
static CMD_SEQ: AtomicU64 = AtomicU64::new(1);

/// Emit a completed-command record (`ok=true`/`ok=false`).
///
/// Private and taking `&CommandTrace`: every per-command field but the
/// resolution-time `dur_us`/`ok` already lives on the guard, so the writer reads
/// them off it rather than restating the grammar's columns as parameters. The
/// sole caller is [`CommandTrace::complete`]; keeping the writer module-private
/// makes [`CommandTrace`] the only way to emit a command record — see the
/// module-level "subprocess chokepoint" note.
fn command_completed(trace: &CommandTrace, dur_us: u64, ok: bool) {
    let cmd = trace.cmd.as_str();
    let (ts, tid, seq, stdin) = (trace.start_ts_us, trace.tid, trace.seq, trace.reads_stdin);
    match trace.context.as_deref() {
        Some(ctx) => tracing::debug!(
            target: WT_TRACE_TARGET,
            kind = "cmd_completed",
            ts,
            tid,
            seq,
            context = ctx,
            cmd,
            dur_us,
            ok,
            stdin,
        ),
        None => tracing::debug!(
            target: WT_TRACE_TARGET,
            kind = "cmd_completed",
            ts,
            tid,
            seq,
            cmd,
            dur_us,
            ok,
            stdin,
        ),
    }
}

/// Emit a failed-command record (the command didn't run to completion).
///
/// Takes `&CommandTrace` for the same reason as [`command_completed`]. The sole
/// caller is [`CommandTrace::fail`].
fn command_errored(trace: &CommandTrace, dur_us: u64, err: impl Display) {
    let err = err.to_string();
    let cmd = trace.cmd.as_str();
    let (ts, tid, seq, stdin) = (trace.start_ts_us, trace.tid, trace.seq, trace.reads_stdin);
    match trace.context.as_deref() {
        Some(ctx) => tracing::debug!(
            target: WT_TRACE_TARGET,
            kind = "cmd_errored",
            ts,
            tid,
            seq,
            context = ctx,
            cmd,
            dur_us,
            err = %err,
            stdin,
        ),
        None => tracing::debug!(
            target: WT_TRACE_TARGET,
            kind = "cmd_errored",
            ts,
            tid,
            seq,
            cmd,
            dur_us,
            err = %err,
            stdin,
        ),
    }
}

/// The single emitter for subprocess command records — every `[wt-trace]`
/// `cmd=…` line comes from here.
///
/// Construct one immediately before spawning a child, then call
/// [`complete`](Self::complete) (the child exited — `ok=` reflects its status)
/// or [`fail`](Self::fail) (the command never ran to completion — spawn error,
/// wait error) exactly once. The record is emitted at that call; the duration
/// is the elapsed time from construction to resolution, so it brackets the
/// real spawn → wait span even for streamed commands whose duration isn't
/// known until the stream finishes.
///
/// # Why a guard rather than a free function
///
/// Subprocesses are spawned from many code paths with incompatible I/O shapes
/// (capture, inherit, buffer-then-stream, two-stage pipe, N concurrent
/// children). They can't share one spawn primitive, but they can share one
/// *trace* primitive. Centralizing emission here — with the writers private to
/// this module — means a path either resolves a `CommandTrace` or emits no
/// record at all, instead of each path re-deriving (and routinely forgetting)
/// the emit logic.
///
/// The `#[must_use]` and the debug-build drop assertion catch the remaining
/// failure mode: a guard that is constructed but never resolved (a spawn site
/// that forgot to wire up `complete`/`fail`) trips a test-time assertion rather
/// than silently producing an unattributed gap in the timeline. The assertion
/// is suppressed while the thread is panicking so an unrelated panic between
/// construction and resolution unwinds cleanly instead of aborting.
#[must_use = "a CommandTrace must be resolved with complete() or fail() to emit its record"]
pub struct CommandTrace {
    context: Option<String>,
    cmd: String,
    start_ts_us: u64,
    start: Instant,
    tid: u64,
    seq: u64,
    /// Whether the command consumed stdin not captured in its command string
    /// (a `stdin_bytes` buffer or an upstream pipe). Marks the record so the
    /// cache analysis never treats it as a duplicate — its real input isn't in
    /// `cmd`, so two runs with identical `cmd` may be entirely different work.
    reads_stdin: bool,
    resolved: bool,
}

impl CommandTrace {
    /// Begin tracing a command. Call immediately before spawning the child so
    /// the captured start time brackets the subprocess.
    pub fn new(context: Option<&str>, cmd: &str) -> Self {
        Self {
            context: context.map(ToOwned::to_owned),
            cmd: cmd.to_owned(),
            start_ts_us: now_us(),
            start: Instant::now(),
            tid: thread_id(),
            seq: CMD_SEQ.fetch_add(1, Ordering::Relaxed),
            reads_stdin: false,
            resolved: false,
        }
    }

    /// Mark this command as consuming stdin not captured in its command string.
    /// Set by spawn sites that feed a `stdin_bytes` buffer or pipe an upstream
    /// command's output in — the record then carries `stdin=true`, and the
    /// cache analysis excludes it from duplicate detection.
    pub fn reads_stdin(mut self, yes: bool) -> Self {
        self.reads_stdin = yes;
        self
    }

    /// The command's monotonic sequence number — the key shared by this
    /// command's `trace.jsonl` record and its raw output block
    /// (`subprocess.log`).
    pub(crate) fn seq(&self) -> u64 {
        self.seq
    }

    /// The thread that ran the command.
    pub(crate) fn tid(&self) -> u64 {
        self.tid
    }

    /// The command string (e.g. `git status --porcelain`).
    pub(crate) fn cmd(&self) -> &str {
        &self.cmd
    }

    /// The command's context label (typically a worktree name), if any.
    pub(crate) fn context(&self) -> Option<&str> {
        self.context.as_deref()
    }

    /// The child ran to completion; `success` is its exit status. Emits an
    /// `ok=true`/`ok=false` record with the elapsed duration.
    pub fn complete(&mut self, success: bool) {
        let dur_us = self.start.elapsed().as_micros() as u64;
        command_completed(self, dur_us, success);
        self.resolved = true;
    }

    /// Emit a failed record for a command that never produced a child to hold
    /// a guard against — a precondition failure before spawn, where there is
    /// nothing to time. Equivalent to `new` immediately followed by `fail`.
    /// `reads_stdin` records the same stdin shape a successful run would have, so
    /// a precondition-failed stdin command is excluded from dedup like any other.
    pub fn record_failed(context: Option<&str>, cmd: &str, reads_stdin: bool, err: impl Display) {
        let mut trace = Self::new(context, cmd).reads_stdin(reads_stdin);
        trace.fail(err);
    }

    /// The command never ran to completion (spawn failure, wait failure).
    /// Emits an `err=…` record with the elapsed duration.
    pub fn fail(&mut self, err: impl Display) {
        let dur_us = self.start.elapsed().as_micros() as u64;
        command_errored(self, dur_us, err);
        self.resolved = true;
    }
}

impl Drop for CommandTrace {
    fn drop(&mut self) {
        // A guard reaching drop unresolved means a spawn path forgot to call
        // complete()/fail() — catch it in tests. Suppress while panicking so an
        // unrelated panic between construction and resolution doesn't abort by
        // double-panicking during unwind.
        debug_assert!(
            self.resolved || std::thread::panicking(),
            "CommandTrace for `{}` dropped without complete()/fail(); \
             every spawn must resolve its trace (see src/trace/emit.rs)",
            self.cmd
        );
    }
}

/// Emit an instant (milestone) event with no duration. Computes `ts` and
/// `tid` internally — use for one-off markers inside a thread's execution.
///
/// Instant events appear as vertical lines in Chrome Trace Format tools
/// (chrome://tracing, Perfetto).
pub fn instant(event: &str) {
    tracing::debug!(
        target: WT_TRACE_TARGET,
        kind = "instant",
        ts = now_us(),
        tid = thread_id(),
        event,
    );
}

/// Emit a completed in-process span (a named region of code that ran).
///
/// Spans are the in-process counterpart to `command_completed`: subprocess
/// records cover work in child processes; spans cover everything between and
/// around them (config load, repo open, template render, etc.).
pub fn span_completed(name: &str, ts: u64, tid: u64, dur_us: u64) {
    tracing::debug!(
        target: WT_TRACE_TARGET,
        kind = "span",
        ts,
        tid,
        span = name,
        dur_us,
    );
}

/// RAII guard that times its enclosing scope and emits a span record on drop.
///
/// Construct at the top of a block — `let _span = Span::new("config_load");` —
/// and the span fires when `_span` goes out of scope.
///
/// `name` accepts anything that converts into `Cow<'static, str>`: string
/// literals stay borrowed (allocation-free), and `String` becomes owned —
/// useful when the span name carries dynamic context, e.g.
/// `Span::new(format!("prepare_steps:{}", alias))`.
///
/// The `tracing::enabled!` check happens on drop, not construction. A span
/// constructed before the subscriber is installed (e.g. wrapping the logger
/// init itself) still fires correctly as long as the subscriber is up by the
/// time the span goes out of scope. Construction always pays two
/// `Instant::now()` calls; they're vDSO-fast and the overhead is below noise.
pub struct Span {
    name: Cow<'static, str>,
    start_ts_us: u64,
    start: Instant,
}

impl Span {
    pub fn new(name: impl Into<Cow<'static, str>>) -> Self {
        Self {
            name: name.into(),
            start_ts_us: now_us(),
            start: Instant::now(),
        }
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        if !tracing::enabled!(tracing::Level::DEBUG) {
            return;
        }
        let dur_us = self.start.elapsed().as_micros() as u64;
        span_completed(&self.name, self.start_ts_us, thread_id(), dur_us);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Resolution (complete/fail) marks the guard so its drop is a no-op. No
    // tracing subscriber is installed, so the records themselves are dropped —
    // the point is that the drop-time tripwire does not fire on a resolved
    // guard. (The wire grammar is locked separately by
    // `logging::tests::format_wt_trace_renders_each_kind`.)
    #[test]
    fn resolved_trace_drops_cleanly() {
        let mut completed = CommandTrace::new(Some("ctx"), "git status");
        completed.complete(true);
        drop(completed);

        let mut failed = CommandTrace::new(None, "git nope");
        failed.fail(std::io::Error::other("boom"));
        drop(failed);

        CommandTrace::record_failed(None, "git nope", false, "precondition");
    }

    // A guard that reaches drop without complete()/fail() is a spawn site that
    // forgot to resolve its trace — the debug-build tripwire turns that into a
    // test failure rather than a silent unattributed gap.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "dropped without complete()/fail()")]
    fn unresolved_trace_panics_on_drop() {
        let _trace = CommandTrace::new(None, "git unresolved");
        // No complete()/fail() — drop here trips the assertion.
    }
}
