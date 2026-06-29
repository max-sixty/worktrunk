//! Trace log parsing and Chrome Trace Format export.
//!
//! This module provides tools for analyzing `wt-trace` log output to understand
//! where time is spent during command execution.
//!
//! # Features
//!
//! - **Trace parsing**: Parse `wt-trace` log lines into structured entries
//! - **Chrome Trace Format**: Export for chrome://tracing or Perfetto visualization
//! - **SQL analysis**: Use Perfetto's trace_processor for queries
//!
//! # Usage
//!
//! ```bash
//! # Text timeline of one wt invocation
//! cargo run -p wt-perf -- timeline -- list --progressive
//!
//! # Chrome Trace Format JSON for Perfetto/chrome://tracing
//! # (--progressive forces TTY-gated events like `Skeleton rendered` to
//! # fire even though wt-perf pipes wt's stdout to /dev/null)
//! cargo run -p wt-perf -- timeline --chrome -- list --progressive > trace.json
//!
//! # From a trace.jsonl already captured to disk
//! cargo run -p wt-perf -- trace .git/wt/logs/trace.jsonl > trace.json
//!
//! # Analyze with SQL (requires: curl -LO https://get.perfetto.dev/trace_processor)
//! trace_processor trace.json -Q 'SELECT name, COUNT(*), SUM(dur)/1e6 as ms FROM slice GROUP BY name'
//!
//! # Find milestone events (instant events have dur=0)
//! trace_processor trace.json -Q 'SELECT name, ts/1e6 as ms FROM slice WHERE dur = 0'
//!
//! # Time from start to skeleton render
//! trace_processor trace.json -Q "
//!   SELECT (skeleton.ts - start.ts)/1e6 as skeleton_ms
//!   FROM slice start, slice skeleton
//!   WHERE start.name = 'List collect started'
//!     AND skeleton.name = 'Skeleton rendered'"
//! ```

pub mod chrome;
pub mod emit;
pub mod parse;
pub mod profile;

// Re-export main types for convenience
pub use chrome::to_chrome_trace;
pub use emit::{CommandTrace, Span, WT_TRACE_TARGET, instant, now_us, thread_id};
pub use parse::{TraceEntry, TraceEntryKind, TraceResult, parse_lines};
pub use profile::{CacheReport, Profile};
