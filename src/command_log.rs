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

static COMMAND_LOG: OnceLock<Mutex<CommandLog>> = OnceLock::new();

struct CommandLog {
    log_path: PathBuf,
    file: Option<File>,
    wt_command: String,
}

impl CommandLog {
    fn new(log_dir: &Path, wt_command: &str) -> Self {
        Self {
            log_path: log_dir.join("commands.jsonl"),
            file: None,
            wt_command: wt_command.to_string(),
        }
    }

    fn write(
        &mut self,
        label: &str,
        command: &str,
        exit_code: Option<i32>,
        duration: Option<Duration>,
    ) {
        // Rotate if needed
        if let Ok(metadata) = fs::metadata(&self.log_path)
            && metadata.len() > MAX_LOG_SIZE
        {
            let old_path = self.log_path.with_extension("jsonl.old");
            let _ = fs::rename(&self.log_path, &old_path);
            self.file = None; // Force re-open after rotation
        }

        // Lazily open the file on first write (or after rotation)
        if self.file.is_none() {
            if let Some(parent) = self.log_path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            self.file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.log_path)
                .ok();
        }

        let cmd_display = truncate_cmd(command);
        let ts = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let entry = serde_json::json!({
            "ts": ts,
            "wt": self.wt_command,
            "label": label,
            "cmd": cmd_display,
            "exit": exit_code,
            "dur_ms": duration.map(|d| d.as_millis() as u64),
        });

        // Single write_all so each JSON line is written atomically
        let mut buf = entry.to_string();
        buf.push('\n');

        let Some(file) = self.file.as_mut() else {
            return;
        };
        let _ = file.write_all(buf.as_bytes());
    }
}

/// Initialize the command log.
///
/// Call once at startup after determining the repository's log directory.
/// The log file and directory are created lazily on first write.
pub fn init(log_dir: &Path, wt_command: &str) {
    let logger = CommandLog::new(log_dir, wt_command);

    // OnceLock::set fails if already initialized — that's fine, ignore the error
    let _ = COMMAND_LOG.set(Mutex::new(logger));
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

    let Ok(mut logger) = mutex.lock() else {
        return;
    };

    logger.write(label, command, exit_code, duration);
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

    #[test]
    fn test_write_creates_file_lazily() {
        let dir = tempfile::tempdir().unwrap();
        let mut logger = CommandLog::new(dir.path(), "wt test");

        assert!(!dir.path().join("commands.jsonl").exists());
        logger.write("test", "echo hi", Some(0), Some(Duration::from_millis(10)));
        assert!(dir.path().join("commands.jsonl").exists());

        let content = fs::read_to_string(dir.path().join("commands.jsonl")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(parsed["label"], "test");
        assert_eq!(parsed["cmd"], "echo hi");
        assert_eq!(parsed["exit"], 0);
        assert_eq!(parsed["wt"], "wt test");
    }

    #[test]
    fn test_write_appends_multiple_entries() {
        let dir = tempfile::tempdir().unwrap();
        let mut logger = CommandLog::new(dir.path(), "wt test");

        logger.write("a", "cmd-a", Some(0), Some(Duration::from_millis(1)));
        logger.write("b", "cmd-b", Some(1), Some(Duration::from_millis(2)));

        let content = fs::read_to_string(dir.path().join("commands.jsonl")).unwrap();
        let lines: Vec<&str> = content.trim().lines().collect();
        assert_eq!(lines.len(), 2);

        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(first["label"], "a");
        assert_eq!(second["label"], "b");
    }

    #[test]
    fn test_rotation_at_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("commands.jsonl");

        // Write a file just over MAX_LOG_SIZE
        let filler = "x".repeat(MAX_LOG_SIZE as usize + 1);
        fs::write(&log_path, &filler).unwrap();

        let mut logger = CommandLog::new(dir.path(), "wt test");
        // Open the existing oversized file so the logger has a handle
        logger.file = OpenOptions::new().append(true).open(&log_path).ok();

        logger.write(
            "rotated",
            "echo rotated",
            Some(0),
            Some(Duration::from_millis(5)),
        );

        // Old file should exist with the filler content
        let old_path = dir.path().join("commands.jsonl.old");
        assert!(old_path.exists());
        assert_eq!(fs::read_to_string(&old_path).unwrap(), filler);

        // New file should have just the one rotated entry
        let content = fs::read_to_string(&log_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(parsed["label"], "rotated");
    }
}
