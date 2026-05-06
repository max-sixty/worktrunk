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
//! [wt-trace] ts=1234567 tid=3 span="build_hook_context" dur_us=8200
//! ```
//!
//! In-process spans (everything that isn't a subprocess) use [`Span`], an
//! RAII guard that captures `ts` at construction and emits the completed
//! record on drop with the elapsed duration. Use it to attribute time spent
//! in code paths subprocess records can't see (config load, repo open,
//! template render).
//!
//! Records are emitted at `log::debug!`, so `-vv` or `RUST_LOG=debug` makes
//! them visible. Subprocess stdout/stderr continuations are emitted via
//! separate log targets: the full output goes to `output.log`, and a bounded
//! preview goes to stderr + `trace.log` — so raw bodies don't spam `-vv`.

use std::borrow::Cow;
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

/// Emit a completed in-process span (a named region of code that ran).
///
/// Spans are the in-process counterpart to `command_completed`: subprocess
/// records cover work in child processes; spans cover everything between and
/// around them (config load, repo open, template render, etc.).
pub fn span_completed(name: &str, ts: u64, tid: u64, dur_us: u64) {
    log::debug!(
        r#"[wt-trace] ts={} tid={} span="{}" dur_us={}"#,
        ts,
        tid,
        name,
        dur_us
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
/// The `log_enabled!` check happens on drop, not construction. A span
/// constructed before `init_logging` (e.g. wrapping the logger init itself)
/// still fires correctly as long as the logger is up by the time the span
/// goes out of scope. Construction always pays two `Instant::now()` calls;
/// they're vDSO-fast and the overhead is below noise.
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
        if !log::log_enabled!(log::Level::Debug) {
            return;
        }
        let dur_us = self.start.elapsed().as_micros() as u64;
        span_completed(&self.name, self.start_ts_us, thread_id(), dur_us);
    }
}
