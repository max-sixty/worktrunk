//! Parse wt-trace log lines into structured entries.
//!
//! Trace lines are emitted by `shell_exec::run()` with this format:
//! ```text
//! [wt-trace] context=worktree cmd="git status" dur=12.3ms ok=true
//! [wt-trace] cmd="gh pr list" dur=45.2ms ok=false
//! [wt-trace] context=main cmd="git merge-base" dur=100.0ms err="fatal: ..."
//! ```

use std::time::Duration;

/// A parsed trace entry from a wt-trace log line.
#[derive(Debug, Clone, PartialEq)]
pub struct TraceEntry {
    /// Optional context (typically worktree name for git commands)
    pub context: Option<String>,
    /// Full command string (e.g., "git status --porcelain")
    pub command: String,
    /// Command duration
    pub duration: Duration,
    /// Command result
    pub result: TraceResult,
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
    pub fn program(&self) -> &str {
        self.command.split_whitespace().next().unwrap_or("")
    }

    /// Extract git subcommand if this is a git command.
    /// Returns None if not a git command.
    pub fn git_subcommand(&self) -> Option<&str> {
        let mut parts = self.command.split_whitespace();
        let program = parts.next()?;
        if program == "git" { parts.next() } else { None }
    }

    /// Returns true if the command succeeded.
    pub fn is_success(&self) -> bool {
        matches!(self.result, TraceResult::Completed { success: true })
    }
}

/// Parse a single trace line.
///
/// Returns `None` if the line doesn't match the expected format.
/// The `[wt-trace]` marker can appear anywhere in the line (to handle log prefixes).
pub fn parse_line(line: &str) -> Option<TraceEntry> {
    // Find the [wt-trace] marker anywhere in the line
    let marker = "[wt-trace] ";
    let marker_pos = line.find(marker)?;
    let rest = &line[marker_pos + marker.len()..];

    // Parse key=value pairs
    let mut context = None;
    let mut command = None;
    let mut duration = None;
    let mut result = None;

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
            _ => {} // Ignore unknown keys for forward compatibility
        }
    }

    Some(TraceEntry {
        context,
        command: command?,
        duration: duration?,
        result: result?,
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
        assert_eq!(entry.command, "git status");
        assert_eq!(entry.duration, Duration::from_secs_f64(0.0123));
        assert!(entry.is_success());
    }

    #[test]
    fn test_parse_with_context() {
        let line =
            r#"[wt-trace] context=main cmd="git merge-base HEAD origin/main" dur=45.2ms ok=true"#;
        let entry = parse_line(line).unwrap();

        assert_eq!(entry.context, Some("main".to_string()));
        assert_eq!(entry.command, "git merge-base HEAD origin/main");
        assert_eq!(entry.git_subcommand(), Some("merge-base"));
    }

    #[test]
    fn test_parse_error() {
        let line = r#"[wt-trace] cmd="git rev-list" dur=100.0ms err="fatal: bad revision""#;
        let entry = parse_line(line).unwrap();

        assert!(!entry.is_success());
        assert!(matches!(
            entry.result,
            TraceResult::Error { message } if message == "fatal: bad revision"
        ));
    }

    #[test]
    fn test_parse_ok_false() {
        let line = r#"[wt-trace] cmd="git diff" dur=5.0ms ok=false"#;
        let entry = parse_line(line).unwrap();

        assert!(!entry.is_success());
        assert!(matches!(
            entry.result,
            TraceResult::Completed { success: false }
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
        assert_eq!(entry.command, "git status");
    }

    #[test]
    fn test_parse_unknown_keys_ignored() {
        // Unknown keys should be ignored for forward compatibility
        let line =
            r#"[wt-trace] future_field=xyz cmd="git status" dur=5.0ms ok=true extra=ignored"#;
        let entry = parse_line(line).unwrap();
        assert_eq!(entry.command, "git status");
        assert!(entry.is_success());
    }

    #[test]
    fn test_parse_trailing_whitespace() {
        // Trailing whitespace should be handled (exercises trim_start + break)
        let line = "[wt-trace] cmd=\"git status\" dur=5.0ms ok=true   ";
        let entry = parse_line(line).unwrap();
        assert_eq!(entry.command, "git status");
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
        assert_eq!(entries[0].command, "git status");
        assert_eq!(entries[1].command, "git diff");
    }
}
