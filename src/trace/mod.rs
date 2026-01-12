//! Trace log parsing and performance analysis.
//!
//! This module provides tools for analyzing `wt-trace` log output to understand
//! where time is spent during command execution.
//!
//! **Note:** This module is new and minimal. We're very open to adding more
//! metrics, visualizations, or analysis capabilities as needs emerge.
//!
//! To support wall-time analysis, thread utilization, or concurrency visualization,
//! we'd need to add timestamps to the trace log format (currently only duration is logged).
//!
//! # Usage
//!
//! ```ignore
//! use worktrunk::trace::{parse, analyze, display};
//!
//! let entries = parse::parse_lines(&log_output);
//! let analysis = analyze::analyze(&entries);
//! println!("{}", display::render(&analysis));
//! ```

pub mod analyze;
pub mod display;
pub mod parse;

// Re-export main types for convenience
pub use analyze::{CommandStats, TraceAnalysis, analyze};
pub use display::render;
pub use parse::{TraceEntry, TraceResult, parse_line, parse_lines};
