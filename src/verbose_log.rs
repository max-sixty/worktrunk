//! Verbose log file management for diagnostics.
//!
//! When `--verbose` is passed, logs are written to both stderr AND
//! `.git/wt-logs/verbose.log`. This file can be included in diagnostic
//! reports to help debug issues.
//!
//! # Usage
//!
//! 1. Call `init()` early in main() after parsing CLI args but before logging
//! 2. Call `write_line()` from the log format function
//! 3. The diagnostic module reads the log file via `log_file_path()`

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// Global state for verbose logging to file.
static VERBOSE_LOG: OnceLock<Mutex<Option<VerboseLog>>> = OnceLock::new();

struct VerboseLog {
    path: PathBuf,
    file: File,
}

/// Initialize verbose log file writing.
///
/// Should be called early in main() when `--verbose` is set.
/// Tries to find a git repo and create the log file.
pub fn init() {
    let mutex = VERBOSE_LOG.get_or_init(|| Mutex::new(None));
    let Ok(mut guard) = mutex.lock() else { return };

    // Try to find the repo and create the log file
    if let Some((path, file)) = try_create_log_file() {
        *guard = Some(VerboseLog { path, file });
    }
}

/// Write a line to the verbose log file (if initialized).
///
/// Call this from the log format function. The line should be
/// plain text (no ANSI codes) for readability in issue reports.
pub fn write_line(line: &str) {
    if let Some(mutex) = VERBOSE_LOG.get()
        && let Ok(mut guard) = mutex.lock()
        && let Some(log) = guard.as_mut()
    {
        // Ignore write errors - logging shouldn't break the command
        let _ = writeln!(log.file, "{}", line);
        let _ = log.file.flush();
    }
}

/// Get the path to the verbose log file, if it was created.
///
/// Used by the diagnostic module to include log contents.
pub fn log_file_path() -> Option<PathBuf> {
    VERBOSE_LOG.get().and_then(|mutex| {
        mutex
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(|log| log.path.clone()))
    })
}

/// Try to create the verbose log file in the repo's wt-logs directory.
///
/// TODO: This uses current_dir() which ignores the `-C` flag. If a user runs
/// `wt -C /other/repo list --verbose` from outside the repo, the log file
/// will be created in the wrong location (or not at all). To fix this,
/// verbose_log::init() would need to run after set_base_path(), or accept
/// a repository parameter.
fn try_create_log_file() -> Option<(PathBuf, File)> {
    // Find the git repo from current directory
    let cwd = std::env::current_dir().ok()?;
    let repo = worktrunk::git::Repository::at(cwd);

    // Get the wt-logs directory (creates it if needed)
    let log_dir = repo.wt_logs_dir().ok()?;
    std::fs::create_dir_all(&log_dir).ok()?;

    let path = log_dir.join("verbose.log");

    // Truncate/create the file
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .ok()?;

    Some((path, file))
}
