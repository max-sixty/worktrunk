//! Parse wt-trace log lines into structured entries.
//!
//! Trace lines are emitted by `shell_exec::run()` with this format:
//! ```text
//! [wt-trace] ts=1234567890 tid=3 context=worktree cmd="git status" dur=12.3ms ok=true
//! [wt-trace] ts=1234567890 tid=3 cmd="gh pr list" dur=45.2ms ok=false
//! [wt-trace] ts=1234567890 tid=3 context=main cmd="git merge-base" dur=100.0ms err="fatal: ..."
//! ```
//!
//! Instant events (milestones without duration) use this format:
//! ```text
//! [wt-trace] ts=1234567890 tid=3 event="Showed skeleton"
//! ```
//!
//! The `ts` (timestamp in microseconds since epoch) and `tid` (thread ID) fields
//! enable concurrency analysis and Chrome Trace Format export for visualizing
//! thread utilization in tools like chrome://tracing or Perfetto.

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
    /// Extract the program name (first word of command).
    /// Returns empty string for instant events.
    pub fn program(&self) -> &str {
        match &self.kind {
            TraceEntryKind::Command { command, .. } => {
                command.split_whitespace().next().unwrap_or("")
            }
            TraceEntryKind::Instant { .. } => "",
        }
    }

    /// Extract git subcommand if this is a git command.
    /// Returns None if not a git command or if this is an instant event.
    pub fn git_subcommand(&self) -> Option<&str> {
        match &self.kind {
            TraceEntryKind::Command { command, .. } => {
                let mut parts = command.split_whitespace();
                let program = parts.next()?;
                if program == "git" { parts.next() } else { None }
            }
            TraceEntryKind::Instant { .. } => None,
        }
    }

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

    /// Returns the command string if this is a command entry.
    pub fn command(&self) -> Option<&str> {
        match &self.kind {
            TraceEntryKind::Command { command, .. } => Some(command),
            TraceEntryKind::Instant { .. } => None,
        }
    }

    /// Returns the event name if this is an instant event.
    pub fn event_name(&self) -> Option<&str> {
        match &self.kind {
            TraceEntryKind::Command { .. } => None,
            TraceEntryKind::Instant { name } => Some(name),
        }
    }

    /// Returns the duration if this is a command entry.
    pub fn duration(&self) -> Option<Duration> {
        match &self.kind {
            TraceEntryKind::Command { duration, .. } => Some(*duration),
            TraceEntryKind::Instant { .. } => None,
        }
    }

    /// Returns the display name for this entry.
    pub fn display_name(&self) -> &str {
        match &self.kind {
            TraceEntryKind::Command { command, .. } => command,
            TraceEntryKind::Instant { name } => name,
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
pub fn parse_line(line: &str) -> Option<TraceEntry> {
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
                // Parse "123.4ms"
                let ms_str = value.strip_suffix("ms")?;
                let ms: f64 = ms_str.parse().ok()?;
                duration = Some(Duration::from_secs_f64(ms / 1000.0));
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
        assert_eq!(entry.command(), Some("git status"));
        assert_eq!(entry.duration(), Some(Duration::from_secs_f64(0.0123)));
        assert!(entry.is_success());
    }

    #[test]
    fn test_parse_with_context() {
        let line =
            r#"[wt-trace] context=main cmd="git merge-base HEAD origin/main" dur=45.2ms ok=true"#;
        let entry = parse_line(line).unwrap();

        assert_eq!(entry.context, Some("main".to_string()));
        assert_eq!(entry.command(), Some("git merge-base HEAD origin/main"));
        assert_eq!(entry.git_subcommand(), Some("merge-base"));
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
    fn test_program_extraction() {
        let line = r#"[wt-trace] cmd="gh pr list --limit 10" dur=200.0ms ok=true"#;
        let entry = parse_line(line).unwrap();

        assert_eq!(entry.program(), "gh");
        assert_eq!(entry.git_subcommand(), None);
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
        assert_eq!(entry.command(), Some("git status"));
    }

    #[test]
    fn test_parse_unknown_keys_ignored() {
        // Unknown keys should be ignored for forward compatibility
        let line =
            r#"[wt-trace] future_field=xyz cmd="git status" dur=5.0ms ok=true extra=ignored"#;
        let entry = parse_line(line).unwrap();
        assert_eq!(entry.command(), Some("git status"));
        assert!(entry.is_success());
    }

    #[test]
    fn test_parse_trailing_whitespace() {
        // Trailing whitespace should be handled (exercises trim_start + break)
        let line = "[wt-trace] cmd=\"git status\" dur=5.0ms ok=true   ";
        let entry = parse_line(line).unwrap();
        assert_eq!(entry.command(), Some("git status"));
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
        assert_eq!(entries[0].command(), Some("git status"));
        assert_eq!(entries[1].command(), Some("git diff"));
    }

    #[test]
    fn test_parse_with_timestamp_and_thread_id() {
        let line = r#"[wt-trace] ts=1736600000000000 tid=5 context=feature cmd="git status" dur=12.3ms ok=true"#;
        let entry = parse_line(line).unwrap();

        assert_eq!(entry.start_time_us, Some(1736600000000000));
        assert_eq!(entry.thread_id, Some(5));
        assert_eq!(entry.context, Some("feature".to_string()));
        assert_eq!(entry.command(), Some("git status"));
        assert!(entry.is_success());
    }

    #[test]
    fn test_parse_without_timestamp_and_thread_id() {
        // Old format traces (without ts/tid) should still parse with None values
        let line = r#"[wt-trace] cmd="git status" dur=12.3ms ok=true"#;
        let entry = parse_line(line).unwrap();

        assert_eq!(entry.start_time_us, None);
        assert_eq!(entry.thread_id, None);
        assert_eq!(entry.command(), Some("git status"));
    }

    #[test]
    fn test_parse_partial_new_fields() {
        // Only ts provided, no tid
        let line = r#"[wt-trace] ts=1736600000000000 cmd="git status" dur=12.3ms ok=true"#;
        let entry = parse_line(line).unwrap();

        assert_eq!(entry.start_time_us, Some(1736600000000000));
        assert_eq!(entry.thread_id, None);
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
        assert_eq!(entry.event_name(), Some("Showed skeleton"));
        assert_eq!(entry.command(), None);
        assert_eq!(entry.duration(), None);
        assert!(entry.is_success()); // Instant events are always "successful"
    }

    #[test]
    fn test_parse_instant_event_with_context() {
        let line = r#"[wt-trace] ts=1736600000000000 tid=3 context=main event="Skeleton rendered""#;
        let entry = parse_line(line).unwrap();

        assert_eq!(entry.context, Some("main".to_string()));
        assert_eq!(entry.event_name(), Some("Skeleton rendered"));
    }

    #[test]
    fn test_parse_instant_event_minimal() {
        // Instant event with only the required field
        let line = r#"[wt-trace] event="Started""#;
        let entry = parse_line(line).unwrap();

        assert_eq!(entry.event_name(), Some("Started"));
        assert_eq!(entry.start_time_us, None);
        assert_eq!(entry.thread_id, None);
    }

    #[test]
    fn test_display_name() {
        // Command entry
        let cmd_line = r#"[wt-trace] cmd="git status" dur=5.0ms ok=true"#;
        let cmd_entry = parse_line(cmd_line).unwrap();
        assert_eq!(cmd_entry.display_name(), "git status");

        // Instant event
        let event_line = r#"[wt-trace] event="Showed skeleton""#;
        let event_entry = parse_line(event_line).unwrap();
        assert_eq!(event_entry.display_name(), "Showed skeleton");
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
        assert_eq!(entries[0].event_name(), Some("Started"));
        assert_eq!(entries[1].command(), Some("git status"));
        assert_eq!(entries[2].event_name(), Some("Showed skeleton"));
        assert_eq!(entries[3].command(), Some("git diff"));
        assert_eq!(entries[4].event_name(), Some("Done"));
    }
}
