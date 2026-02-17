//! Pager detection and execution.
//!
//! Handles detection and use of diff pagers (delta, bat, etc.) for preview windows.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

use wait_timeout::ChildExt;

use worktrunk::config::UserConfig;
use worktrunk::shell::extract_filename_from_path;

use crate::pager::{git_config_pager, parse_pager_value};

/// Cached pager command, ready to use. None means no pager.
static CACHED_PAGER: OnceLock<Option<String>> = OnceLock::new();

/// Maximum time to wait for pager to complete.
///
/// Pager blocking can freeze skim's event loop, making the UI unresponsive.
/// If the pager takes longer than this, kill it and fall back to raw diff.
pub(super) const PAGER_TIMEOUT: Duration = Duration::from_millis(2000);

/// Check if a pager spawns its own internal pager (e.g., less).
/// Delta and bat spawn `less` by default, which hangs in non-TTY contexts.
fn needs_paging_disabled(pager_cmd: &str) -> bool {
    pager_cmd
        .split_whitespace()
        .next()
        .and_then(extract_filename_from_path)
        .is_some_and(|basename| {
            basename.eq_ignore_ascii_case("delta")
                || basename.eq_ignore_ascii_case("bat")
                || basename.eq_ignore_ascii_case("batcat")
        })
}

/// Get the cached pager command, ready to use.
///
/// Returns the pager command with any necessary flags (like `--paging=never`)
/// already appended. Precedence:
/// 1. `[switch.picker] pager` in user config (used as-is)
/// 2. `[select] pager` in user config (deprecated, used as-is)
/// 3. `GIT_PAGER` environment variable (auto-detection applied)
/// 4. `core.pager` git config (auto-detection applied)
pub(super) fn get_diff_pager() -> Option<&'static String> {
    CACHED_PAGER
        .get_or_init(|| {
            // Check user config first - use exactly as specified (no auto-detection)
            // Uses switch_picker() accessor which handles [switch.picker] → [select] fallback
            if let Ok(config) = UserConfig::load()
                && let Some(pager) = config.switch_picker(None).pager
                && !pager.trim().is_empty()
            {
                return Some(pager);
            }

            // GIT_PAGER or core.pager - apply auto-detection for delta/bat
            let pager = if let Ok(p) = std::env::var("GIT_PAGER") {
                parse_pager_value(&p)
            } else {
                git_config_pager()
            };

            pager.map(|p| {
                if needs_paging_disabled(&p) {
                    format!("{} --paging=never", p)
                } else {
                    p
                }
            })
        })
        .as_ref()
}

/// Pipe text through the configured pager for display.
///
/// Returns the paged output, or the original text if the pager fails or times out.
/// Sets `COLUMNS` environment variable for pagers like delta with side-by-side mode.
pub(super) fn pipe_through_pager(text: &str, pager_cmd: &str, width: usize) -> String {
    log::debug!("Piping through pager: {}", pager_cmd);

    // Spawn pager with stdin piped
    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(pager_cmd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("COLUMNS", width.to_string())
        .env_remove(worktrunk::shell_exec::DIRECTIVE_FILE_ENV_VAR)
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            log::debug!("Failed to spawn pager: {}", e);
            return text.to_string();
        }
    };

    // Write input to stdin in a thread to avoid deadlock.
    // Thread will unblock when: (a) write completes, or (b) pipe breaks (pager exits/killed).
    let stdin = child.stdin.take();
    let input = text.to_string();
    let writer_thread = std::thread::spawn(move || {
        if let Some(mut stdin) = stdin {
            use std::io::Write;
            let _ = stdin.write_all(input.as_bytes());
        }
    });

    // Read output in a thread to avoid deadlock (can't read stdout after stdin fills)
    let stdout = child.stdout.take();
    let reader_thread = std::thread::spawn(move || {
        stdout.map(|mut stdout| {
            let mut output = Vec::new();
            let _ = stdout.read_to_end(&mut output);
            output
        })
    });

    // Wait for pager with timeout
    match child.wait_timeout(PAGER_TIMEOUT) {
        Ok(Some(status)) => {
            // Pager exited within timeout
            let _ = writer_thread.join();
            if let Ok(Some(output)) = reader_thread.join()
                && status.success()
                && let Ok(s) = String::from_utf8(output)
            {
                return s;
            }
            log::debug!("Pager exited with status: {}", status);
        }
        Ok(None) => {
            // Timed out - kill pager and clean up
            log::debug!("Pager timed out after {:?}", PAGER_TIMEOUT);
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader_thread.join();
        }
        Err(e) => {
            log::debug!("Failed to wait for pager: {}", e);
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader_thread.join();
        }
    }

    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_needs_paging_disabled() {
        // delta - plain command name
        assert!(needs_paging_disabled("delta"));
        // delta - with arguments
        assert!(needs_paging_disabled("delta --side-by-side"));
        assert!(needs_paging_disabled("delta --paging=always"));
        // delta - full path
        assert!(needs_paging_disabled("/usr/bin/delta"));
        assert!(needs_paging_disabled(
            "/opt/homebrew/bin/delta --line-numbers"
        ));
        // bat - also spawns less by default
        assert!(needs_paging_disabled("bat"));
        assert!(needs_paging_disabled("/usr/bin/bat"));
        assert!(needs_paging_disabled("bat --style=plain"));
        // Pagers that don't spawn sub-pagers
        assert!(!needs_paging_disabled("less"));
        assert!(!needs_paging_disabled("diff-so-fancy"));
        assert!(!needs_paging_disabled("colordiff"));
        // Edge cases - similar names but not delta/bat
        assert!(!needs_paging_disabled("delta-preview"));
        assert!(!needs_paging_disabled("/path/to/delta-preview"));
        assert!(needs_paging_disabled("batcat")); // Debian's bat package name

        // Case-insensitive matching (Windows command names)
        assert!(needs_paging_disabled("Delta"));
        assert!(needs_paging_disabled("DELTA"));
        assert!(needs_paging_disabled("BAT"));
        assert!(needs_paging_disabled("Bat"));
        assert!(needs_paging_disabled("BatCat"));
        assert!(needs_paging_disabled("delta.exe"));
        assert!(needs_paging_disabled("Delta.EXE"));
    }

    #[test]
    fn test_get_diff_pager_initializes() {
        // Exercise the config initialization path
        // Returns None or Some depending on user's pager config
        let _ = get_diff_pager();
    }

    #[test]
    fn test_pipe_through_pager_passthrough() {
        // Use cat as a simple pager that passes through input unchanged
        let input = "line 1\nline 2\nline 3";
        let result = pipe_through_pager(input, "cat", 80);
        assert_eq!(result, input);
    }

    #[test]
    fn test_pipe_through_pager_with_transform() {
        // Use tr to transform input (proves pager is actually being invoked)
        let input = "hello world";
        let result = pipe_through_pager(input, "tr 'a-z' 'A-Z'", 80);
        assert_eq!(result, "HELLO WORLD");
    }

    #[test]
    fn test_pipe_through_pager_invalid_command() {
        // Invalid pager command should return original text
        let input = "original text";
        let result = pipe_through_pager(input, "nonexistent-command-xyz", 80);
        assert_eq!(result, input);
    }

    #[test]
    fn test_pipe_through_pager_failing_command() {
        // Pager that exits with error should return original text
        let input = "original text";
        let result = pipe_through_pager(input, "false", 80);
        assert_eq!(result, input);
    }
}
