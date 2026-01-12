//! Chrome Trace Format output for concurrency visualization.
//!
//! Converts trace entries to Chrome Trace Event Format JSON, which can be
//! visualized in chrome://tracing or <https://ui.perfetto.dev>.
//!
//! # Event Types
//!
//! - **Complete events** (`ph: "X"`): Command executions with duration
//! - **Instant events** (`ph: "I"`): Milestones without duration (e.g., "Showed skeleton")
//!
//! # Usage
//!
//! ```bash
//! RUST_LOG=debug wt list 2>&1 | grep wt-trace | analyze-trace > trace.json
//! # Then open trace.json in chrome://tracing or https://ui.perfetto.dev
//! ```
//!
//! # Format Reference
//!
//! - [Trace Event Format](https://docs.google.com/document/d/1CvAClvFfyA5R-PhYUmn5OOQtYMH4h6I0nSsKchNAySU/)
//! - [Perfetto UI](https://ui.perfetto.dev)

use serde::Serialize;

use super::{TraceEntry, TraceEntryKind};

/// A Chrome Trace Event in the Trace Event Format.
///
/// Uses `#[serde(skip_serializing_if)]` to emit the correct fields for each phase:
/// - Complete events ("X"): have `dur`, no `s`
/// - Instant events ("I"): have `s` (scope), no `dur`
#[derive(Debug, Serialize)]
struct TraceEvent {
    /// Event name (displayed in the UI)
    name: String,
    /// Phase: "X" for complete events, "I" for instant events
    ph: &'static str,
    /// Timestamp in microseconds since epoch
    ts: u64,
    /// Duration in microseconds (for "X" phase events only)
    #[serde(skip_serializing_if = "Option::is_none")]
    dur: Option<u64>,
    /// Scope for instant events: "g" (global), "p" (process), or "t" (thread)
    #[serde(skip_serializing_if = "Option::is_none")]
    s: Option<&'static str>,
    /// Process ID (we use 1 for all events since wt is single-process)
    pid: u32,
    /// Thread ID
    tid: u64,
    /// Category (optional, for filtering in the UI)
    #[serde(skip_serializing_if = "Option::is_none")]
    cat: Option<String>,
    /// Custom arguments (shown when event is selected in UI)
    #[serde(skip_serializing_if = "Option::is_none")]
    args: Option<TraceEventArgs>,
}

/// Custom arguments attached to trace events.
#[derive(Debug, Serialize)]
struct TraceEventArgs {
    /// Worktree context (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<String>,
    /// Whether the command succeeded (always true for instant events)
    success: bool,
    /// Duration in milliseconds (human-readable, only for command events)
    #[serde(rename = "duration_ms", skip_serializing_if = "Option::is_none")]
    duration_ms: Option<f64>,
}

/// The top-level Chrome Trace Format structure.
#[derive(Debug, Serialize)]
struct ChromeTrace {
    /// Array of trace events
    #[serde(rename = "traceEvents")]
    trace_events: Vec<TraceEvent>,
    /// Display time unit preference
    #[serde(rename = "displayTimeUnit")]
    display_time_unit: &'static str,
    /// Metadata: tool that generated the trace
    #[serde(rename = "meta_generator")]
    meta_generator: &'static str,
}

/// Convert trace entries to Chrome Trace Format JSON.
///
/// Returns pretty-printed JSON suitable for chrome://tracing or Perfetto.
/// Entries without timestamp/thread data use 0 as fallback.
///
/// # Event Types
///
/// - Command entries become Complete events (`"ph": "X"`) with duration
/// - Instant entries become Instant events (`"ph": "I"`) with global scope
pub fn to_chrome_trace(entries: &[TraceEntry]) -> String {
    let trace_events: Vec<TraceEvent> = entries
        .iter()
        .map(|entry| {
            let ts = entry.start_time_us.unwrap_or(0);
            let tid = entry.thread_id.unwrap_or(0);

            match &entry.kind {
                TraceEntryKind::Command {
                    command, duration, ..
                } => {
                    // Categorize by program type
                    let cat = if command.starts_with("git ") {
                        Some("git".to_string())
                    } else if command.starts_with("gh ") || command.starts_with("glab ") {
                        Some("network".to_string())
                    } else {
                        None
                    };

                    TraceEvent {
                        name: command.clone(),
                        ph: "X", // Complete event (has duration)
                        ts,
                        dur: Some(duration.as_micros() as u64),
                        s: None,
                        pid: 1,
                        tid,
                        cat,
                        args: Some(TraceEventArgs {
                            context: entry.context.clone(),
                            success: entry.is_success(),
                            duration_ms: Some(duration.as_secs_f64() * 1000.0),
                        }),
                    }
                }
                TraceEntryKind::Instant { name } => {
                    TraceEvent {
                        name: name.clone(),
                        ph: "I", // Instant event (no duration)
                        ts,
                        dur: None,
                        s: Some("g"), // Global scope - shows across all threads
                        pid: 1,
                        tid,
                        cat: Some("milestone".to_string()),
                        args: Some(TraceEventArgs {
                            context: entry.context.clone(),
                            success: true,
                            duration_ms: None,
                        }),
                    }
                }
            }
        })
        .collect();

    let chrome_trace = ChromeTrace {
        trace_events,
        display_time_unit: "ms",
        meta_generator: "worktrunk analyze-trace",
    };

    serde_json::to_string_pretty(&chrome_trace).expect("Failed to serialize trace to JSON")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::trace::{TraceEntry, TraceResult};

    fn make_command_entry(
        command: &str,
        duration_ms: u64,
        start_time_us: Option<u64>,
        thread_id: Option<u64>,
    ) -> TraceEntry {
        TraceEntry {
            context: Some("feature".to_string()),
            kind: TraceEntryKind::Command {
                command: command.to_string(),
                duration: Duration::from_millis(duration_ms),
                result: TraceResult::Completed { success: true },
            },
            start_time_us,
            thread_id,
        }
    }

    fn make_instant_entry(
        name: &str,
        start_time_us: Option<u64>,
        thread_id: Option<u64>,
    ) -> TraceEntry {
        TraceEntry {
            context: None,
            kind: TraceEntryKind::Instant {
                name: name.to_string(),
            },
            start_time_us,
            thread_id,
        }
    }

    #[test]
    fn test_to_chrome_trace_with_timestamps() {
        let entries = vec![
            make_command_entry("git status", 10, Some(1000000), Some(1)),
            make_command_entry("git diff", 20, Some(1000000), Some(2)),
            make_command_entry("git log", 15, Some(1010000), Some(1)),
        ];

        let json = to_chrome_trace(&entries);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["displayTimeUnit"], "ms");
        assert_eq!(parsed["meta_generator"], "worktrunk analyze-trace");

        let events = parsed["traceEvents"].as_array().unwrap();
        assert_eq!(events.len(), 3);

        // First event
        assert_eq!(events[0]["name"], "git status");
        assert_eq!(events[0]["ph"], "X");
        assert_eq!(events[0]["ts"], 1000000);
        assert_eq!(events[0]["dur"], 10000); // 10ms = 10000µs
        assert_eq!(events[0]["tid"], 1);
        assert_eq!(events[0]["cat"], "git");

        // Second event (parallel on thread 2)
        assert_eq!(events[1]["tid"], 2);
        assert_eq!(events[1]["ts"], 1000000); // Same start time = parallel
    }

    #[test]
    fn test_to_chrome_trace_without_timestamps() {
        // Old format entries without ts/tid get 0 as fallback
        let entries = vec![
            make_command_entry("git status", 10, None, None),
            make_command_entry("git diff", 20, None, None),
        ];

        let json = to_chrome_trace(&entries);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        let events = parsed["traceEvents"].as_array().unwrap();

        // Fallback: both get ts=0 and tid=0
        assert_eq!(events[0]["ts"], 0);
        assert_eq!(events[1]["ts"], 0);
        assert_eq!(events[0]["tid"], 0);
        assert_eq!(events[1]["tid"], 0);
    }

    #[test]
    fn test_category_assignment() {
        let entries = vec![
            make_command_entry("git status", 10, Some(0), Some(1)),
            make_command_entry("gh pr list", 100, Some(0), Some(2)),
            make_command_entry("glab mr list", 100, Some(0), Some(3)),
            make_command_entry("echo hello", 1, Some(0), Some(4)),
        ];

        let json = to_chrome_trace(&entries);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let events = parsed["traceEvents"].as_array().unwrap();

        assert_eq!(events[0]["cat"], "git");
        assert_eq!(events[1]["cat"], "network");
        assert_eq!(events[2]["cat"], "network");
        assert!(events[3]["cat"].is_null()); // No category for other commands
    }

    #[test]
    fn test_args_include_context() {
        let entries = vec![make_command_entry("git status", 10, Some(0), Some(1))];

        let json = to_chrome_trace(&entries);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let events = parsed["traceEvents"].as_array().unwrap();

        assert_eq!(events[0]["args"]["context"], "feature");
        assert_eq!(events[0]["args"]["success"], true);
        assert_eq!(events[0]["args"]["duration_ms"], 10.0);
    }

    // ========================================================================
    // Instant event tests
    // ========================================================================

    #[test]
    fn test_instant_event() {
        let entries = vec![make_instant_entry(
            "Showed skeleton",
            Some(1000000),
            Some(1),
        )];

        let json = to_chrome_trace(&entries);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let events = parsed["traceEvents"].as_array().unwrap();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["name"], "Showed skeleton");
        assert_eq!(events[0]["ph"], "I"); // Instant event
        assert_eq!(events[0]["ts"], 1000000);
        assert_eq!(events[0]["s"], "g"); // Global scope
        assert_eq!(events[0]["cat"], "milestone");
        assert!(events[0]["dur"].is_null()); // No duration for instant events
        assert_eq!(events[0]["args"]["success"], true);
        assert!(events[0]["args"]["duration_ms"].is_null());
    }

    #[test]
    fn test_mixed_events() {
        let entries = vec![
            make_instant_entry("Started", Some(1000000), Some(1)),
            make_command_entry("git status", 10, Some(1000100), Some(1)),
            make_instant_entry("Showed skeleton", Some(1010000), Some(1)),
            make_command_entry("git diff", 20, Some(1010100), Some(2)),
            make_instant_entry("Done", Some(1030000), Some(1)),
        ];

        let json = to_chrome_trace(&entries);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let events = parsed["traceEvents"].as_array().unwrap();

        assert_eq!(events.len(), 5);

        // Check instant events
        assert_eq!(events[0]["ph"], "I");
        assert_eq!(events[0]["name"], "Started");

        assert_eq!(events[2]["ph"], "I");
        assert_eq!(events[2]["name"], "Showed skeleton");

        assert_eq!(events[4]["ph"], "I");
        assert_eq!(events[4]["name"], "Done");

        // Check command events
        assert_eq!(events[1]["ph"], "X");
        assert_eq!(events[1]["name"], "git status");

        assert_eq!(events[3]["ph"], "X");
        assert_eq!(events[3]["name"], "git diff");
    }
}
