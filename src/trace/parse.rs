//! Parse wt-trace log lines into structured entries.
//!
//! Trace lines are emitted by `shell_exec::Cmd` with this format:
//! ```text
//! [wt-trace] ts=1234567 tid=3 context=worktree cmd="git status" dur_us=12300 ok=true
//! [wt-trace] ts=1234567 tid=3 cmd="gh pr list" dur_us=45200 ok=false
//! [wt-trace] ts=1234567 tid=3 context=main cmd="git merge-base" dur_us=100000 err="fatal: ..."
//! ```
//!
//! Instant events (milestones without duration) use this format:
//! ```text
//! [wt-trace] ts=1234567 tid=3 event="Showed skeleton"
//! ```
//!
//! The `ts` (timestamp in microseconds since trace epoch) and `tid` (thread ID) fields
//! enable concurrency analysis and Chrome Trace Format export for visualizing
//! thread utilization in tools like chrome://tracing or Perfetto.
//!
//! Both `dur_us` (microseconds, preferred) and `dur` (milliseconds, legacy) are supported.

use std::time::Duration;

/// The kind of trace entry: command execution or instant event.
#[derive(Debug, Clone, PartialEq)]
pub enum TraceEntryKind {
    /// A command execution with duration and result
    Command {
        /// Full command string (e.g., "git status --porcelain")
        command: String,
        /// Command duration
        duration: Duration,
        /// Command result
        result: TraceResult,
    },
    /// An instant event (milestone marker with no duration)
    Instant {
        /// Event name (e.g., "Showed skeleton")
        name: String,
    },
}

/// A parsed trace entry from a wt-trace log line.
#[derive(Debug, Clone, PartialEq)]
pub struct TraceEntry {
    /// Optional context (typically worktree name for git commands)
    pub context: Option<String>,
    /// The kind of trace entry
    pub kind: TraceEntryKind,
    /// Start timestamp in microseconds since Unix epoch (for Chrome Trace Format)
    pub start_time_us: Option<u64>,
    /// Thread ID that executed this command (for concurrency analysis)
    pub thread_id: Option<u64>,
}

/// Result of a traced command.
#[derive(Debug, Clone, PartialEq)]
pub enum TraceResult {
    /// Command completed (ok=true or ok=false)
    Completed { success: bool },
    /// Command failed with error (err="...")
    Error { message: String },
}

impl TraceEntry {
    /// Returns true if the command succeeded.
    /// Instant events always return true.
    pub fn is_success(&self) -> bool {
        match &self.kind {
            TraceEntryKind::Command { result, .. } => {
                matches!(result, TraceResult::Completed { success: true })
            }
            TraceEntryKind::Instant { .. } => true,
        }
    }
}

/// Parse a single trace line.
///
/// Returns `None` if the line doesn't match the expected format.
/// The `[wt-trace]` marker can appear anywhere in the line (to handle log prefixes).
///
/// Supports two formats:
/// - Command events: `cmd="..." dur=...ms ok=true/false` or `err="..."`
/// - Instant events: `event="..."`
fn parse_line(line: &str) -> Option<TraceEntry> {
    // Find the [wt-trace] marker anywhere in the line
    let marker = "[wt-trace] ";
    let marker_pos = line.find(marker)?;
    let rest = &line[marker_pos + marker.len()..];

    // Parse key=value pairs
    let mut context = None;
    let mut command = None;
    let mut event = None;
    let mut duration = None;
    let mut result = None;
    let mut start_time_us = None;
    let mut thread_id = None;

    let mut remaining = rest;

    while !remaining.is_empty() {
        remaining = remaining.trim_start();
        if remaining.is_empty() {
            break;
        }

        // Find key=
        let eq_pos = remaining.find('=')?;
        let key = &remaining[..eq_pos];
        remaining = &remaining[eq_pos + 1..];

        // Parse value (quoted or unquoted)
        let value = if remaining.starts_with('"') {
            // Quoted value - find closing quote
            remaining = &remaining[1..];
            let end_quote = remaining.find('"')?;
            let val = &remaining[..end_quote];
            remaining = &remaining[end_quote + 1..];
            val
        } else {
            // Unquoted value - ends at space or end
            let end = remaining.find(' ').unwrap_or(remaining.len());
            let val = &remaining[..end];
            remaining = &remaining[end..];
            val
        };

        match key {
            "context" => context = Some(value.to_string()),
            "cmd" => command = Some(value.to_string()),
            "event" => event = Some(value.to_string()),
            "dur" => {
                // Parse "123.4ms" (legacy format)
                let ms_str = value.strip_suffix("ms")?;
                let ms: f64 = ms_str.parse().ok()?;
                duration = Some(Duration::from_secs_f64(ms / 1000.0));
            }
            "dur_us" => {
                // Parse microseconds (new format, no precision loss)
                let us: u64 = value.parse().ok()?;
                duration = Some(Duration::from_micros(us));
            }
            "ok" => {
                let success = value == "true";
                result = Some(TraceResult::Completed { success });
            }
            "err" => {
                result = Some(TraceResult::Error {
                    message: value.to_string(),
                });
            }
            "ts" => {
                start_time_us = value.parse().ok();
            }
            "tid" => {
                thread_id = value.parse().ok();
            }
            _ => {} // Ignore unknown keys for forward compatibility
        }
    }

    // Determine the entry kind based on what was parsed
    let kind = if let Some(event_name) = event {
        // Instant event
        TraceEntryKind::Instant { name: event_name }
    } else {
        // Command event - requires cmd, dur, and result
        TraceEntryKind::Command {
            command: command?,
            duration: duration?,
            result: result?,
        }
    };

    Some(TraceEntry {
        context,
        kind,
        start_time_us,
        thread_id,
    })
}

/// Parse multiple lines, filtering to only valid trace entries.
pub fn parse_lines(input: &str) -> Vec<TraceEntry> {
    input.lines().filter_map(parse_line).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_basic() {
        let line = r#"[wt-trace] cmd="git status" dur=12.3ms ok=true"#;
        let entry = parse_line(line).unwrap();

        assert_eq!(entry.context, None);
        let TraceEntryKind::Command {
            command, duration, ..
        } = &entry.kind
        else {
            panic!("expected command");
        };
        assert_eq!(command, "git status");
        assert_eq!(*duration, Duration::from_secs_f64(0.0123));
        assert!(entry.is_success());
    }

    #[test]
    fn test_parse_with_context() {
        let line =
            r#"[wt-trace] context=main cmd="git merge-base HEAD origin/main" dur=45.2ms ok=true"#;
        let entry = parse_line(line).unwrap();

        assert_eq!(entry.context, Some("main".to_string()));
        let TraceEntryKind::Command { command, .. } = &entry.kind else {
            panic!("expected command");
        };
        assert_eq!(command, "git merge-base HEAD origin/main");
    }

    #[test]
    fn test_parse_error() {
        let line = r#"[wt-trace] cmd="git rev-list" dur=100.0ms err="fatal: bad revision""#;
        let entry = parse_line(line).unwrap();

        assert!(!entry.is_success());
        assert!(matches!(
            &entry.kind,
            TraceEntryKind::Command { result: TraceResult::Error { message }, .. } if message == "fatal: bad revision"
        ));
    }

    #[test]
    fn test_parse_ok_false() {
        let line = r#"[wt-trace] cmd="git diff" dur=5.0ms ok=false"#;
        let entry = parse_line(line).unwrap();

        assert!(!entry.is_success());
        assert!(matches!(
            &entry.kind,
            TraceEntryKind::Command {
                result: TraceResult::Completed { success: false },
                ..
            }
        ));
    }

    #[test]
    fn test_parse_non_trace_line() {
        assert!(parse_line("some random log line").is_none());
        assert!(parse_line("[other-tag] something").is_none());
    }

    #[test]
    fn test_parse_with_log_prefix() {
        // Real output has thread ID prefix like "[a] "
        let line = r#"[a] [wt-trace] cmd="git status" dur=5.0ms ok=true"#;
        let entry = parse_line(line).unwrap();
        assert!(matches!(
            &entry.kind,
            TraceEntryKind::Command { command, .. } if command == "git status"
        ));
    }

    #[test]
    fn test_parse_unknown_keys_ignored() {
        // Unknown keys should be ignored for forward compatibility
        let line =
            r#"[wt-trace] future_field=xyz cmd="git status" dur=5.0ms ok=true extra=ignored"#;
        let entry = parse_line(line).unwrap();
        assert!(matches!(
            &entry.kind,
            TraceEntryKind::Command { command, .. } if command == "git status"
        ));
        assert!(entry.is_success());
    }

    #[test]
    fn test_parse_trailing_whitespace() {
        // Trailing whitespace should be handled (exercises trim_start + break)
        let line = "[wt-trace] cmd=\"git status\" dur=5.0ms ok=true   ";
        let entry = parse_line(line).unwrap();
        assert!(matches!(
            &entry.kind,
            TraceEntryKind::Command { command, .. } if command == "git status"
        ));
    }

    #[test]
    fn test_parse_lines() {
        let input = r#"
DEBUG some other log
[wt-trace] cmd="git status" dur=10.0ms ok=true
more noise
[wt-trace] cmd="git diff" dur=20.0ms ok=true
"#;
        let entries = parse_lines(input);
        assert_eq!(entries.len(), 2);
        assert!(matches!(
            &entries[0].kind,
            TraceEntryKind::Command { command, .. } if command == "git status"
        ));
        assert!(matches!(
            &entries[1].kind,
            TraceEntryKind::Command { command, .. } if command == "git diff"
        ));
    }

    #[test]
    fn test_parse_with_timestamp_and_thread_id() {
        let line = r#"[wt-trace] ts=1736600000000000 tid=5 context=feature cmd="git status" dur=12.3ms ok=true"#;
        let entry = parse_line(line).unwrap();

        assert_eq!(entry.start_time_us, Some(1736600000000000));
        assert_eq!(entry.thread_id, Some(5));
        assert_eq!(entry.context, Some("feature".to_string()));
        assert!(matches!(
            &entry.kind,
            TraceEntryKind::Command { command, .. } if command == "git status"
        ));
        assert!(entry.is_success());
    }

    #[test]
    fn test_parse_without_timestamp_and_thread_id() {
        // Old format traces (without ts/tid) should still parse with None values
        let line = r#"[wt-trace] cmd="git status" dur=12.3ms ok=true"#;
        let entry = parse_line(line).unwrap();

        assert_eq!(entry.start_time_us, None);
        assert_eq!(entry.thread_id, None);
        assert!(matches!(
            &entry.kind,
            TraceEntryKind::Command { command, .. } if command == "git status"
        ));
    }

    #[test]
    fn test_parse_partial_new_fields() {
        // Only ts provided, no tid
        let line = r#"[wt-trace] ts=1736600000000000 cmd="git status" dur=12.3ms ok=true"#;
        let entry = parse_line(line).unwrap();

        assert_eq!(entry.start_time_us, Some(1736600000000000));
        assert_eq!(entry.thread_id, None);
    }

    #[test]
    fn test_parse_dur_us_format() {
        // New format with microseconds (no precision loss)
        let line = r#"[wt-trace] ts=1234567 tid=3 cmd="git status" dur_us=12345 ok=true"#;
        let entry = parse_line(line).unwrap();

        let TraceEntryKind::Command { duration, .. } = &entry.kind else {
            panic!("expected command");
        };
        assert_eq!(*duration, Duration::from_micros(12345));
    }

    // ========================================================================
    // Instant event tests
    // ========================================================================

    #[test]
    fn test_parse_instant_event() {
        let line = r#"[wt-trace] ts=1736600000000000 tid=3 event="Showed skeleton""#;
        let entry = parse_line(line).unwrap();

        assert_eq!(entry.start_time_us, Some(1736600000000000));
        assert_eq!(entry.thread_id, Some(3));
        let TraceEntryKind::Instant { name } = &entry.kind else {
            panic!("expected instant event");
        };
        assert_eq!(name, "Showed skeleton");
        assert!(entry.is_success()); // Instant events are always "successful"
    }

    #[test]
    fn test_parse_instant_event_with_context() {
        let line = r#"[wt-trace] ts=1736600000000000 tid=3 context=main event="Skeleton rendered""#;
        let entry = parse_line(line).unwrap();

        assert_eq!(entry.context, Some("main".to_string()));
        assert!(matches!(
            &entry.kind,
            TraceEntryKind::Instant { name } if name == "Skeleton rendered"
        ));
    }

    #[test]
    fn test_parse_instant_event_minimal() {
        // Instant event with only the required field
        let line = r#"[wt-trace] event="Started""#;
        let entry = parse_line(line).unwrap();

        assert!(matches!(
            &entry.kind,
            TraceEntryKind::Instant { name } if name == "Started"
        ));
        assert_eq!(entry.start_time_us, None);
        assert_eq!(entry.thread_id, None);
    }

    #[test]
    fn test_parse_lines_mixed() {
        let input = r#"
[wt-trace] event="Started"
[wt-trace] cmd="git status" dur=10.0ms ok=true
[wt-trace] event="Showed skeleton"
[wt-trace] cmd="git diff" dur=20.0ms ok=true
[wt-trace] event="Done"
"#;
        let entries = parse_lines(input);
        assert_eq!(entries.len(), 5);
        assert!(matches!(&entries[0].kind, TraceEntryKind::Instant { name } if name == "Started"));
        assert!(
            matches!(&entries[1].kind, TraceEntryKind::Command { command, .. } if command == "git status")
        );
        assert!(
            matches!(&entries[2].kind, TraceEntryKind::Instant { name } if name == "Showed skeleton")
        );
        assert!(
            matches!(&entries[3].kind, TraceEntryKind::Command { command, .. } if command == "git diff")
        );
        assert!(matches!(&entries[4].kind, TraceEntryKind::Instant { name } if name == "Done"));
    }
}
