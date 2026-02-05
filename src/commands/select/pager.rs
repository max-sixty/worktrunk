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

/// Cached pager command, detected once at startup.
///
/// None means no pager should be used (empty config or "cat").
/// We cache this to avoid running `git config` on every preview render.
pub(super) static CACHED_PAGER: OnceLock<Option<String>> = OnceLock::new();

/// Cached flag for whether user has explicit pager config.
/// Avoids reloading config on every preview render.
static HAS_EXPLICIT_PAGER_CONFIG: OnceLock<bool> = OnceLock::new();

/// Maximum time to wait for pager to complete.
///
/// Pager blocking can freeze skim's event loop, making the UI unresponsive.
/// If the pager takes longer than this, kill it and fall back to raw diff.
pub(super) const PAGER_TIMEOUT: Duration = Duration::from_millis(2000);

/// Get the cached pager command, initializing if needed.
///
/// Precedence (highest to lowest):
/// 1. `[select] pager` in user config (explicit override, used as-is)
/// 2. `GIT_PAGER` environment variable (with auto-detection applied)
/// 3. `core.pager` git config (with auto-detection applied)
pub(super) fn get_diff_pager() -> Option<&'static String> {
    CACHED_PAGER
        .get_or_init(|| {
            // Check user config first for explicit pager override
            // When set, use exactly as specified (no auto-detection)
            if let Ok(config) = UserConfig::load()
                && let Some(select_config) = config.configs.select
                && let Some(pager) = select_config.pager
                && !pager.trim().is_empty()
            {
                return Some(pager);
            }

            // GIT_PAGER takes precedence over core.pager
            if let Ok(pager) = std::env::var("GIT_PAGER") {
                return parse_pager_value(&pager);
            }

            // Fall back to core.pager config
            git_config_pager()
        })
        .as_ref()
}

/// Check if the pager spawns its own internal pager (e.g., less).
///
/// Some pagers like delta and bat spawn `less` by default, which hangs in
/// non-TTY contexts like skim's preview panel. These need `--paging=never`.
///
/// Used only when user hasn't set `[select] pager` config explicitly.
/// When config is set, that value is used as-is without modification.
pub(super) fn pager_needs_paging_disabled(pager_cmd: &str) -> bool {
    // Split on whitespace to get the command name, then extract basename
    // Uses extract_filename_from_path for consistent handling of Windows paths and .exe
    pager_cmd
        .split_whitespace()
        .next()
        .and_then(extract_filename_from_path)
        // bat is called "batcat" on Debian/Ubuntu
        // Case-insensitive for Windows where commands might be Delta.exe, BAT.EXE, etc.
        .is_some_and(|basename| {
            basename.eq_ignore_ascii_case("delta")
                || basename.eq_ignore_ascii_case("bat")
                || basename.eq_ignore_ascii_case("batcat")
        })
}

/// Check if user has explicitly configured a select-specific pager.
/// Result is cached to avoid reloading config on every preview render.
pub(super) fn has_explicit_pager_config() -> bool {
    *HAS_EXPLICIT_PAGER_CONFIG.get_or_init(|| {
        UserConfig::load()
            .ok()
            .and_then(|config| config.configs.select)
            .and_then(|select| select.pager)
            .is_some_and(|p| !p.trim().is_empty())
    })
}

/// Pipe text through the configured pager for display.
///
/// Takes raw diff/text output and pipes it through the user's pager (delta, bat, etc.)
/// for syntax highlighting and formatting. Returns the paged output, or the original
/// text if the pager fails or times out.
///
/// Sets `COLUMNS` environment variable to the preview width, allowing pagers to detect
/// the correct width. For pagers like delta with side-by-side mode, users can reference
/// this in their config: `[select] pager = "delta --width=$COLUMNS"`.
///
/// When `[select] pager` is not configured, automatically appends `--paging=never` for
/// delta/bat/batcat pagers to prevent hangs.
pub(super) fn pipe_through_pager(text: &str, pager_cmd: &str, width: usize) -> String {
    // Apply auto-detection only when user hasn't set explicit config
    let pager_with_args = if !has_explicit_pager_config() && pager_needs_paging_disabled(pager_cmd)
    {
        format!("{} --paging=never", pager_cmd)
    } else {
        pager_cmd.to_string()
    };

    log::debug!("Piping through pager: {}", pager_with_args);

    // Spawn pager with stdin piped
    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(&pager_with_args)
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
    fn test_pager_needs_paging_disabled() {
        // delta - plain command name
        assert!(pager_needs_paging_disabled("delta"));
        // delta - with arguments
        assert!(pager_needs_paging_disabled("delta --side-by-side"));
        assert!(pager_needs_paging_disabled("delta --paging=always"));
        // delta - full path
        assert!(pager_needs_paging_disabled("/usr/bin/delta"));
        assert!(pager_needs_paging_disabled(
            "/opt/homebrew/bin/delta --line-numbers"
        ));
        // bat - also spawns less by default
        assert!(pager_needs_paging_disabled("bat"));
        assert!(pager_needs_paging_disabled("/usr/bin/bat"));
        assert!(pager_needs_paging_disabled("bat --style=plain"));
        // Pagers that don't spawn sub-pagers
        assert!(!pager_needs_paging_disabled("less"));
        assert!(!pager_needs_paging_disabled("diff-so-fancy"));
        assert!(!pager_needs_paging_disabled("colordiff"));
        // Edge cases - similar names but not delta/bat
        assert!(!pager_needs_paging_disabled("delta-preview"));
        assert!(!pager_needs_paging_disabled("/path/to/delta-preview"));
        assert!(pager_needs_paging_disabled("batcat")); // Debian's bat package name

        // Case-insensitive matching (Windows command names)
        assert!(pager_needs_paging_disabled("Delta"));
        assert!(pager_needs_paging_disabled("DELTA"));
        assert!(pager_needs_paging_disabled("BAT"));
        assert!(pager_needs_paging_disabled("Bat"));
        assert!(pager_needs_paging_disabled("BatCat"));
        assert!(pager_needs_paging_disabled("delta.exe"));
        assert!(pager_needs_paging_disabled("Delta.EXE"));
    }

    #[test]
    fn test_has_explicit_pager_config() {
        // This function loads real config, so we just test that it doesn't panic
        // The behavior is covered by integration tests that set actual config
        let _ = has_explicit_pager_config();
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
