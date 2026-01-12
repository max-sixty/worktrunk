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
//! # Generate Chrome Trace Format
//! RUST_LOG=debug wt list 2>&1 | grep wt-trace | analyze-trace --format=chrome > trace.json
//!
//! # Visualize: open trace.json in chrome://tracing or https://ui.perfetto.dev
//!
//! # Analyze with SQL (requires: curl -LO https://get.perfetto.dev/trace_processor)
//! trace_processor trace.json -Q 'SELECT name, COUNT(*), SUM(dur)/1e6 as ms FROM slice GROUP BY name'
//! ```

pub mod chrome;
pub mod parse;

// Re-export main types for convenience
pub use chrome::to_chrome_trace;
pub use parse::{TraceEntry, TraceResult, parse_line, parse_lines};
