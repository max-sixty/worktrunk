//! Pager detection and execution.
//!
//! Handles detection and use of diff pagers (delta, bat, etc.) for preview windows.

use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use worktrunk::config::WorktrunkConfig;
use worktrunk::shell::extract_filename_from_path;

use crate::pager::{git_config_pager, parse_pager_value};

/// Cached pager command, detected once at startup.
///
/// None means no pager should be used (empty config or "cat").
/// We cache this to avoid running `git config` on every preview render.
pub(super) static CACHED_PAGER: OnceLock<Option<String>> = OnceLock::new();

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
            if let Ok(config) = WorktrunkConfig::load()
                && let Some(select_config) = config.select
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
pub(super) fn has_explicit_pager_config() -> bool {
    WorktrunkConfig::load()
        .ok()
        .and_then(|config| config.select)
        .and_then(|select| select.pager)
        .is_some_and(|p| !p.trim().is_empty())
}

/// Run git diff piped directly through the pager as a streaming pipeline.
///
/// Runs `git <args> | pager` as a single shell command, avoiding intermediate
/// buffering. Returns None if pipeline fails or times out (caller should fall back to raw diff).
///
/// When `[select] pager` is not configured, automatically appends `--paging=never` for
/// delta/bat/batcat pagers to prevent hangs. To override this behavior, set an explicit
/// pager command in config: `[select] pager = "delta"` (or with custom flags).
pub(super) fn run_git_diff_with_pager(git_args: &[&str], pager_cmd: &str) -> Option<String> {
    // Note: pager_cmd is expected to be valid shell code (like git's core.pager).
    // Users with paths containing special chars must quote them in their config.

    // Apply auto-detection only when user hasn't set explicit config
    // If config is set, use the value as-is (user has full control)
    let pager_with_args = if !has_explicit_pager_config() && pager_needs_paging_disabled(pager_cmd)
    {
        format!("{} --paging=never", pager_cmd)
    } else {
        pager_cmd.to_string()
    };

    // Build shell pipeline: git <args> | pager
    // Shell-escape args to handle paths with spaces
    let escaped_args: Vec<String> = git_args
        .iter()
        .map(|arg| shlex::try_quote(arg).unwrap_or((*arg).into()).into_owned())
        .collect();
    let pipeline = format!("git {} | {}", escaped_args.join(" "), pager_with_args);

    log::debug!("Running pager pipeline: {}", pipeline);

    // Spawn pipeline
    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(&pipeline)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        // Prevent subprocesses from writing to the directive file
        .env_remove(worktrunk::shell_exec::DIRECTIVE_FILE_ENV_VAR)
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            log::debug!("Failed to spawn pager pipeline: {}", e);
            return None;
        }
    };

    // Read output in a thread to avoid blocking
    let stdout = child.stdout.take()?;
    let reader_thread = std::thread::spawn(move || {
        use std::io::Read;
        let mut stdout = stdout;
        let mut output = Vec::new();
        let _ = stdout.read_to_end(&mut output);
        output
    });

    // Wait for pipeline with timeout
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = reader_thread.join().ok()?;
                if status.success() {
                    return String::from_utf8(output).ok();
                } else {
                    log::debug!("Pager pipeline exited with status: {}", status);
                    return None;
                }
            }
            Ok(None) => {
                if start.elapsed() > PAGER_TIMEOUT {
                    log::debug!("Pager pipeline timed out after {:?}", PAGER_TIMEOUT);
                    let _ = child.kill();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => {
                log::debug!("Failed to wait for pager pipeline: {}", e);
                let _ = child.kill();
                return None;
            }
        }
    }
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
}
