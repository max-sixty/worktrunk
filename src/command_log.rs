//! Always-on logging for configured external commands.
//!
//! Logs hook execution and LLM commands to `.git/wt-logs/commands.jsonl` as JSONL.
//! Provides an audit trail for debugging without requiring `-vv`.
//!
//! # Growth control
//!
//! Before each write, the file size is checked. If >1MB, the current file is
//! renamed to `commands.jsonl.old` and a fresh file is started. This bounds
//! storage to ~2MB worst case.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// Maximum log file size before rotation (1MB).
const MAX_LOG_SIZE: u64 = 1_048_576;

/// Maximum command string length in log entries.
const MAX_CMD_LENGTH: usize = 2000;

static COMMAND_LOG: OnceLock<Mutex<Option<CommandLog>>> = OnceLock::new();

struct CommandLog {
    log_path: PathBuf,
    file: Option<File>,
    wt_command: String,
}

/// Initialize the command log.
///
/// Call once at startup after determining the repository's log directory.
/// The log file and directory are created lazily on first write.
pub fn init(log_dir: &Path, wt_command: &str) {
    let logger = CommandLog {
        log_path: log_dir.join("commands.jsonl"),
        file: None,
        wt_command: wt_command.to_string(),
    };

    // OnceLock::set fails if already initialized — that's fine, ignore the error
    let _ = COMMAND_LOG.set(Mutex::new(Some(logger)));
}

/// Log an external command execution.
///
/// - `label`: identifies what triggered this command (e.g., "pre-merge user:lint", "commit.generation")
/// - `command`: the shell command that was executed (truncated to 2000 chars)
/// - `exit_code`: `None` for background commands where outcome is unknown
/// - `duration`: `None` for background commands
pub fn log_command(label: &str, command: &str, exit_code: Option<i32>, duration: Option<Duration>) {
    let mutex = match COMMAND_LOG.get() {
        Some(m) => m,
        None => return,
    };

    let mut guard = match mutex.lock() {
        Ok(g) => g,
        Err(_) => return,
    };

    let logger = match guard.as_mut() {
        Some(l) => l,
        None => return,
    };

    // Rotate if needed
    if let Ok(metadata) = fs::metadata(&logger.log_path)
        && metadata.len() > MAX_LOG_SIZE
    {
        let old_path = logger.log_path.with_extension("jsonl.old");
        let _ = fs::rename(&logger.log_path, &old_path);
        logger.file = None; // Force re-open after rotation
    }

    // Lazily open the file on first write (or after rotation)
    if logger.file.is_none() {
        if let Some(parent) = logger.log_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        logger.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&logger.log_path)
            .ok();
    }

    let file = match logger.file.as_mut() {
        Some(f) => f,
        None => return,
    };

    let cmd_display = truncate_cmd(command);

    let ts = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let entry = serde_json::json!({
        "ts": ts,
        "wt": logger.wt_command,
        "label": label,
        "cmd": cmd_display,
        "exit": exit_code,
        "dur_ms": duration.map(|d| d.as_millis() as u64),
    });

    // Single write_all to avoid interleaving with concurrent wt processes
    let mut buf = entry.to_string();
    buf.push('\n');
    let _ = file.write_all(buf.as_bytes());
}

/// Truncate a command string to `MAX_CMD_LENGTH` characters, appending `…` if truncated.
/// Uses char_indices to find the byte boundary in a single scan.
fn truncate_cmd(command: &str) -> String {
    match command.char_indices().nth(MAX_CMD_LENGTH) {
        Some((byte_idx, _)) => {
            let mut s = command[..byte_idx].to_string();
            s.push('…');
            s
        }
        None => command.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_truncation_ascii() {
        let long_cmd = "x".repeat(MAX_CMD_LENGTH + 100);
        let truncated = truncate_cmd(&long_cmd);
        assert_eq!(truncated.chars().count(), MAX_CMD_LENGTH + 1);
        assert!(truncated.ends_with('…'));
    }

    #[test]
    fn test_command_truncation_multibyte() {
        let long_cmd = "é".repeat(MAX_CMD_LENGTH + 100);
        let truncated = truncate_cmd(&long_cmd);
        assert_eq!(truncated.chars().count(), MAX_CMD_LENGTH + 1);
        assert!(truncated.ends_with('…'));
    }

    #[test]
    fn test_command_no_truncation_when_short() {
        let short_cmd = "echo hello";
        let result = truncate_cmd(short_cmd);
        assert_eq!(result, "echo hello");
    }

    #[test]
    fn test_log_command_without_init() {
        // Should silently do nothing when not initialized
        log_command(
            "test",
            "echo hello",
            Some(0),
            Some(Duration::from_millis(100)),
        );
    }

    #[test]
    fn test_json_format() {
        let entry = serde_json::json!({
            "ts": "2026-02-17T10:00:00Z",
            "wt": "wt hook pre-merge --yes",
            "label": "pre-merge user:lint",
            "cmd": "pre-commit run --all-files",
            "exit": 0,
            "dur_ms": 12345_u64,
        });

        let line = entry.to_string();
        let parsed: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed["label"], "pre-merge user:lint");
        assert_eq!(parsed["cmd"], "pre-commit run --all-files");
        assert_eq!(parsed["exit"], 0);
        assert_eq!(parsed["dur_ms"], 12345);
    }

    #[test]
    fn test_null_values_for_background() {
        let entry = serde_json::json!({
            "ts": "2026-02-17T10:00:00Z",
            "wt": "wt switch",
            "label": "post-start user:server",
            "cmd": "npm run dev",
            "exit": null,
            "dur_ms": null,
        });

        let line = entry.to_string();
        let parsed: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert!(parsed["exit"].is_null());
        assert!(parsed["dur_ms"].is_null());
    }

    #[test]
    fn test_special_chars_in_command() {
        // serde_json handles escaping automatically
        let entry = serde_json::json!({
            "cmd": "echo \"hello\nworld\"",
        });
        let line = entry.to_string();
        let parsed: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed["cmd"], "echo \"hello\nworld\"");
    }
}
