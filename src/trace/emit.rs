//! Authoritative emitter for the `[wt-trace]` log grammar.
//!
//! `[wt-trace]` records are structured single-line `key=value` text emitted on
//! top of the `log` crate and parsed downstream by [`super::parse`] and the
//! `wt-perf` binary. This module is the single source of truth for the
//! grammar — any field or formatting change happens here and in `parse.rs`
//! together.
//!
//! # Format
//!
//! ```text
//! [wt-trace] ts=1234567 tid=3 context=worktree cmd="git status" dur_us=12300 ok=true
//! [wt-trace] ts=1234567 tid=3 cmd="gh pr list" dur_us=45200 ok=false
//! [wt-trace] ts=1234567 tid=3 context=main cmd="git merge-base" dur_us=100000 err="fatal: ..."
//! [wt-trace] ts=1234567 tid=3 event="Showed skeleton"
//! ```
//!
//! Records are emitted at `log::debug!`, so `-vv` or `RUST_LOG=debug` makes
//! them visible. Subprocess stdout/stderr continuations are emitted separately
//! — full output at `log::trace!` (`-vvv`) and a bounded preview at
//! `log::debug!` — so raw bodies don't spam `-vv`.

use std::fmt::Display;
use std::sync::OnceLock;
use std::time::Instant;

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

/// Emit a completed-command record (`ok=true`/`ok=false`).
pub fn command_completed(
    context: Option<&str>,
    cmd: &str,
    ts: u64,
    tid: u64,
    dur_us: u64,
    ok: bool,
) {
    match context {
        Some(ctx) => log::debug!(
            r#"[wt-trace] ts={} tid={} context={} cmd="{}" dur_us={} ok={}"#,
            ts,
            tid,
            ctx,
            cmd,
            dur_us,
            ok
        ),
        None => log::debug!(
            r#"[wt-trace] ts={} tid={} cmd="{}" dur_us={} ok={}"#,
            ts,
            tid,
            cmd,
            dur_us,
            ok
        ),
    }
}

/// Emit a failed-command record (the command didn't run to completion).
pub fn command_errored(
    context: Option<&str>,
    cmd: &str,
    ts: u64,
    tid: u64,
    dur_us: u64,
    err: impl Display,
) {
    match context {
        Some(ctx) => log::debug!(
            r#"[wt-trace] ts={} tid={} context={} cmd="{}" dur_us={} err="{}""#,
            ts,
            tid,
            ctx,
            cmd,
            dur_us,
            err
        ),
        None => log::debug!(
            r#"[wt-trace] ts={} tid={} cmd="{}" dur_us={} err="{}""#,
            ts,
            tid,
            cmd,
            dur_us,
            err
        ),
    }
}

/// Emit an instant (milestone) event with no duration. Computes `ts` and
/// `tid` internally — use for one-off markers inside a thread's execution.
///
/// Instant events appear as vertical lines in Chrome Trace Format tools
/// (chrome://tracing, Perfetto).
pub fn instant(event: &str) {
    log::debug!(
        r#"[wt-trace] ts={} tid={} event="{}""#,
        now_us(),
        thread_id(),
        event
    );
}
