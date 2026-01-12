//! Trace log parsing and performance analysis.
//!
//! This module provides tools for analyzing `wt-trace` log output to understand
//! where time is spent during command execution.
//!
//! # Features
//!
//! - **Performance analysis**: Command breakdown, histograms, timeout impact
//! - **Concurrency visualization**: Chrome Trace Format export for chrome://tracing or Perfetto
//!
//! The trace log format includes timestamps (`ts`) and thread IDs (`tid`) to enable
//! concurrency analysis and visualization of thread utilization.
//!
//! # Usage
//!
//! ```ignore
//! use worktrunk::trace::{parse_lines, analyze, render, to_chrome_trace};
//!
//! let entries = parse_lines(&log_output);
//!
//! // Human-readable analysis
//! let analysis = analyze(&entries);
//! println!("{}", render(&analysis));
//!
//! // Chrome Trace Format (for chrome://tracing or Perfetto)
//! let json = to_chrome_trace(&entries);
//! std::fs::write("trace.json", json)?;
//! ```

pub mod analyze;
pub mod chrome;
pub mod display;
pub mod parse;

// Re-export main types for convenience
pub use analyze::{CommandStats, TraceAnalysis, analyze};
pub use chrome::to_chrome_trace;
pub use display::render;
pub use parse::{TraceEntry, TraceResult, parse_line, parse_lines};
