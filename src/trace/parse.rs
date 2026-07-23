//! Parse `trace.jsonl` records into structured entries.
//!
//! `trace.jsonl` is the machine-readable trace sink (`-vv` writes it alongside
//! the human `trace.log`). Each line is one JSON object — a `[wt-trace]` record
//! serialized by `src/logging.rs::TraceJsonlFormat`. The objects dispatch on a
//! `kind` field:
//!
//! ```text
//! {"kind":"cmd_completed","ts":1234567,"tid":3,"seq":1,"context":"worktree","cmd":"git status","dur_us":12300,"ok":true}
//! {"kind":"cmd_errored","ts":1234567,"tid":3,"seq":3,"context":"main","cmd":"git merge-base","dur_us":100000,"err":"fatal: ..."}
//! {"kind":"instant","ts":1234567,"tid":3,"event":"Showed skeleton"}
//! {"kind":"span","ts":1234567,"tid":3,"span":"build_hook_context","dur_us":8200}
//! ```
//!
//! `seq` (a per-command counter, command records only) is ignored here — no
//! consumer needs it; it correlates a record with its raw output block in
//! `subprocess.log`. `stdin` (bool, command records, omitted → `false`) flags a
//! command that consumed stdin the `cmd` string doesn't capture; the cache
//! analysis skips such commands from dedup.
//!
//! `trace.jsonl` also carries free-form `log::*` / `tracing::*` lines as
//! `{"message":...}` (no `kind`); those — and the `$ cmd` start echoes — are
//! skipped, since they aren't trace records.
//!
//! The `ts` (microseconds since trace epoch) and `tid` (thread ID) fields enable
//! concurrency analysis and Chrome Trace Format export for visualizing thread
//! utilization in tools like chrome://tracing or Perfetto. Duration is `dur_us`,
//! in microseconds.

use std::time::Duration;

/// The kind of trace entry: command execution, instant event, or in-process span.
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
        /// Whether the command consumed stdin the `command` string doesn't
        /// capture (a `stdin_bytes` buffer or an upstream pipe). The cache
        /// analysis skips these — two runs with identical `(command, context)`
        /// may be entirely different work. From the `stdin` JSON field
        /// (omitted → `false`).
        reads_stdin: bool,
    },
    /// An instant event (milestone marker with no duration)
    Instant {
        /// Event name (e.g., "Showed skeleton")
        name: String,
    },
    /// An in-process span — a named region of code with a duration.
    /// Distinct from `Command` so consumers can render subprocess vs.
    /// in-process work differently.
    Span {
        /// Span name (e.g., "build_hook_context")
        name: String,
        /// Span duration
        duration: Duration,
    },
}

/// A parsed trace entry from a `trace.jsonl` record.
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
    ///
    /// Instant events and spans always return true — they record completion of
    /// in-process work, not subprocess success/failure.
    pub fn is_success(&self) -> bool {
        match &self.kind {
            TraceEntryKind::Command { result, .. } => {
                matches!(result, TraceResult::Completed { success: true })
            }
            TraceEntryKind::Instant { .. } | TraceEntryKind::Span { .. } => true,
        }
    }
}

/// Parse a single `trace.jsonl` line into a [`TraceEntry`].
///
/// Returns `None` when the line isn't a JSON object, carries no recognized
/// `kind`, or is missing a field that `kind` requires. Non-record lines
/// (free-form `{"message":...}`, the `$ cmd` echoes) have no `kind` and are
/// skipped this way.
fn parse_line(line: &str) -> Option<TraceEntry> {
    let line = line.trim();
    // Cheap reject before handing the line to serde — most non-record lines
    // (start echoes, blank lines) don't even start with `{`.
    if !line.starts_with('{') {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let obj = value.as_object()?;

    let context = obj
        .get("context")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let start_time_us = obj.get("ts").and_then(serde_json::Value::as_u64);
    let thread_id = obj.get("tid").and_then(serde_json::Value::as_u64);

    let dur = || -> Option<Duration> {
        obj.get("dur_us")
            .and_then(serde_json::Value::as_u64)
            .map(Duration::from_micros)
    };

    // `stdin` is omitted from records that didn't read it → `false`.
    let reads_stdin = obj
        .get("stdin")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    let kind = match obj.get("kind")?.as_str()? {
        "cmd_completed" => TraceEntryKind::Command {
            command: obj.get("cmd")?.as_str()?.to_string(),
            duration: dur()?,
            result: TraceResult::Completed {
                success: obj.get("ok")?.as_bool()?,
            },
            reads_stdin,
        },
        "cmd_errored" => TraceEntryKind::Command {
            command: obj.get("cmd")?.as_str()?.to_string(),
            duration: dur()?,
            result: TraceResult::Error {
                message: obj
                    .get("err")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
            },
            reads_stdin,
        },
        "instant" => TraceEntryKind::Instant {
            name: obj.get("event")?.as_str()?.to_string(),
        },
        "span" => TraceEntryKind::Span {
            name: obj.get("span")?.as_str()?.to_string(),
            duration: dur()?,
        },
        _ => return None, // Unknown kind — forward-compatible skip
    };

    Some(TraceEntry {
        context,
        kind,
        start_time_us,
        thread_id,
    })
}

/// Parse multiple lines, filtering to only valid trace records.
pub fn parse_lines(input: &str) -> Vec<TraceEntry> {
    input.lines().filter_map(parse_line).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_basic() {
        let line = r#"{"kind":"cmd_completed","cmd":"git status","dur_us":12300,"ok":true}"#;
        let entry = parse_line(line).unwrap();

        assert_eq!(entry.context, None);
        let TraceEntryKind::Command {
            command, duration, ..
        } = &entry.kind
        else {
            panic!("expected command");
        };
        assert_eq!(command, "git status");
        assert_eq!(*duration, Duration::from_micros(12300));
        assert!(entry.is_success());
    }

    #[test]
    fn test_parse_with_context() {
        let line = r#"{"kind":"cmd_completed","context":"main","cmd":"git merge-base HEAD origin/main","dur_us":45200,"ok":true}"#;
        let entry = parse_line(line).unwrap();

        assert_eq!(entry.context, Some("main".to_string()));
        let TraceEntryKind::Command { command, .. } = &entry.kind else {
            panic!("expected command");
        };
        assert_eq!(command, "git merge-base HEAD origin/main");
    }

    #[test]
    fn test_parse_error() {
        let line = r#"{"kind":"cmd_errored","cmd":"git rev-list","dur_us":100000,"err":"fatal: bad revision"}"#;
        let entry = parse_line(line).unwrap();

        assert!(!entry.is_success());
        assert!(matches!(
            &entry.kind,
            TraceEntryKind::Command { result: TraceResult::Error { message }, .. } if message == "fatal: bad revision"
        ));
    }

    #[test]
    fn test_parse_ok_false() {
        let line = r#"{"kind":"cmd_completed","cmd":"git diff","dur_us":5000,"ok":false}"#;
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
    fn test_parse_reads_stdin() {
        // `stdin:true` is carried onto the Command; omitted → false.
        let with = parse_line(
            r#"{"kind":"cmd_completed","cmd":"git patch-id","dur_us":640,"ok":true,"stdin":true}"#,
        )
        .unwrap();
        assert!(matches!(
            with.kind,
            TraceEntryKind::Command {
                reads_stdin: true,
                ..
            }
        ));
        let without =
            parse_line(r#"{"kind":"cmd_completed","cmd":"git status","dur_us":10,"ok":true}"#)
                .unwrap();
        assert!(matches!(
            without.kind,
            TraceEntryKind::Command {
                reads_stdin: false,
                ..
            }
        ));
    }

    #[test]
    fn test_parse_embedded_quote_in_command() {
        // The escaping win JSON buys us: a literal `"` in the command, where
        // the old `cmd="value"` text grammar would truncate the parse.
        let line = r#"{"kind":"cmd_completed","cmd":"git commit -m \"wip\"","dur_us":1,"ok":true}"#;
        let entry = parse_line(line).unwrap();
        let TraceEntryKind::Command { command, .. } = &entry.kind else {
            panic!("expected command");
        };
        assert_eq!(command, r#"git commit -m "wip""#);
    }

    #[test]
    fn test_parse_non_record_lines() {
        // Not JSON at all (a `$ cmd` echo, a blank line).
        assert!(parse_line("$ git status [main]").is_none());
        assert!(parse_line("").is_none());
        // JSON, but a free-form `log::*` message with no `kind`.
        assert!(parse_line(r#"{"message":"fsmonitor: skipping","level":"warn"}"#).is_none());
        // JSON with an unknown future kind — skipped, not an error.
        assert!(parse_line(r#"{"kind":"future_kind","ts":1}"#).is_none());
    }

    #[test]
    fn test_parse_missing_required_field() {
        // `cmd_completed` without `dur_us` / `ok` can't form a Command.
        assert!(parse_line(r#"{"kind":"cmd_completed","cmd":"git status"}"#).is_none());
        // `span` without `dur_us`.
        assert!(parse_line(r#"{"kind":"span","span":"config_load"}"#).is_none());
    }

    #[test]
    fn test_parse_lines() {
        let input = r#"
DEBUG some other log
{"kind":"cmd_completed","cmd":"git status","dur_us":10000,"ok":true}
{"message":"noise","level":"debug"}
{"kind":"cmd_completed","cmd":"git diff","dur_us":20000,"ok":true}
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
        let line = r#"{"kind":"cmd_completed","ts":1736600000000000,"tid":5,"context":"feature","cmd":"git status","dur_us":12300,"ok":true}"#;
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
        // Records without ts/tid parse with None values.
        let line = r#"{"kind":"cmd_completed","cmd":"git status","dur_us":12300,"ok":true}"#;
        let entry = parse_line(line).unwrap();

        assert_eq!(entry.start_time_us, None);
        assert_eq!(entry.thread_id, None);
        assert!(matches!(
            &entry.kind,
            TraceEntryKind::Command { command, .. } if command == "git status"
        ));
    }

    // ========================================================================
    // Instant event tests
    // ========================================================================

    #[test]
    fn test_parse_instant_event() {
        let line = r#"{"kind":"instant","ts":1736600000000000,"tid":3,"event":"Showed skeleton"}"#;
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
        let line = r#"{"kind":"instant","ts":1736600000000000,"tid":3,"context":"main","event":"Skeleton rendered"}"#;
        let entry = parse_line(line).unwrap();

        assert_eq!(entry.context, Some("main".to_string()));
        assert!(matches!(
            &entry.kind,
            TraceEntryKind::Instant { name } if name == "Skeleton rendered"
        ));
    }

    // ========================================================================
    // Span event tests
    // ========================================================================

    #[test]
    fn test_parse_span_event() {
        let line =
            r#"{"kind":"span","ts":1736600000000000,"tid":3,"span":"config_load","dur_us":8200}"#;
        let entry = parse_line(line).unwrap();

        assert_eq!(entry.start_time_us, Some(1736600000000000));
        assert_eq!(entry.thread_id, Some(3));
        let TraceEntryKind::Span { name, duration } = &entry.kind else {
            panic!("expected span event, got {:?}", entry.kind);
        };
        assert_eq!(name, "config_load");
        assert_eq!(*duration, Duration::from_micros(8200));
        assert!(entry.is_success());
    }

    #[test]
    fn test_parse_lines_mixed() {
        let input = r#"
{"kind":"instant","event":"Started"}
{"kind":"cmd_completed","cmd":"git status","dur_us":10000,"ok":true}
{"kind":"instant","event":"Showed skeleton"}
{"kind":"cmd_completed","cmd":"git diff","dur_us":20000,"ok":true}
{"kind":"span","span":"repo_open","dur_us":1500}
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
        assert!(
            matches!(&entries[4].kind, TraceEntryKind::Span { name, .. } if name == "repo_open")
        );
    }
}
