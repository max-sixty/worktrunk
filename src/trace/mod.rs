//! Trace log parsing and analysis.
//!
//! Tools for analyzing the `trace.jsonl` a `-vv` run captures (to
//! `.git/wt/logs/`), to understand where time went during a `wt` invocation:
//!
//! - [`emit`] — the authoritative `[wt-trace]` record emitter
//! - [`parse`] — `trace.jsonl` lines → structured [`TraceEntry`] values
//! - [`profile`] — the aggregate report behind `wt config state logs profile`:
//!   where time goes (by command type / by context / slowest), parallelism,
//!   and same-context cache misses
//! - [`timeline`] — the per-record view behind `wt-perf timeline`
//! - [`chrome`] — Chrome Trace Format export for visual critical-path
//!   inspection in <https://ui.perfetto.dev> or chrome://tracing
//!
//! Capture with `wt -vv <cmd>`, or let the `wt-perf timeline` helper run the
//! capture and render in one step (`cargo run -p wt-perf -- timeline -- list
//! --progressive`).

pub mod chrome;
pub mod emit;
pub mod parse;
pub mod profile;
pub mod timeline;

// Re-export main types for convenience
pub use chrome::to_chrome_trace;
pub use emit::{CommandTrace, Span, WT_TRACE_TARGET, instant, now_us, thread_id};
pub use parse::{TraceEntry, TraceEntryKind, TraceResult, parse_lines};
pub use profile::{CacheReport, Profile};
pub use timeline::render_timeline;
