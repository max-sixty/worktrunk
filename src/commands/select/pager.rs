//! Pager detection and execution.
//!
//! Handles detection and use of diff pagers (delta, bat, etc.) for preview windows.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use worktrunk::config::UserConfig;
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
pub(super) fn has_explicit_pager_config() -> bool {
    UserConfig::load()
        .ok()
        .and_then(|config| config.configs.select)
        .and_then(|select| select.pager)
        .is_some_and(|p| !p.trim().is_empty())
}

/// Build the pager command with appropriate flags for width and paging.
///
/// When user hasn't set explicit config and the pager is delta/bat/batcat,
/// automatically adds --paging=never and --width flags.
fn build_pager_command(pager_cmd: &str, width: usize) -> String {
    if !has_explicit_pager_config() && pager_needs_paging_disabled(pager_cmd) {
        // Add both --paging=never and --width for delta/bat to use full preview space
        format!("{} --paging=never --width={}", pager_cmd, width)
    } else {
        pager_cmd.to_string()
    }
}

/// Run git diff piped directly through the pager as a streaming pipeline.
///
/// Runs `git <args> | pager` as a single shell command, avoiding intermediate
/// buffering. Returns None if pipeline fails or times out (caller should fall back to raw diff).
///
/// When `[select] pager` is not configured, automatically appends `--paging=never` and
/// `--width` for delta/bat/batcat pagers to prevent hangs and ensure proper width.
/// To override this behavior, set an explicit pager command in config: `[select] pager = "delta"`.
pub(super) fn run_git_diff_with_pager(
    git_args: &[&str],
    pager_cmd: &str,
    width: usize,
) -> Option<String> {
    // Note: pager_cmd is expected to be valid shell code (like git's core.pager).
    // Users with paths containing special chars must quote them in their config.

    // Apply auto-detection only when user hasn't set explicit config
    // If config is set, use the value as-is (user has full control)
    let pager_with_args = build_pager_command(pager_cmd, width);

    // Build shell pipeline: git <args> | pager
    // Shell-escape args to handle paths with spaces
    let escaped_args: Vec<String> = git_args
        .iter()
        .map(|arg| shlex::try_quote(arg).unwrap_or((*arg).into()).into_owned())
        .collect();
    let pipeline = format!("git {} | {}", escaped_args.join(" "), pager_with_args);

    log::debug!("Running pager pipeline: {}", pipeline);

    // Spawn pipeline with COLUMNS set to preview width
    // This ensures pagers that don't support --width can still detect the correct width
    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(&pipeline)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        // Set COLUMNS so pagers can detect preview width
        .env("COLUMNS", width.to_string())
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

    #[test]
    fn test_build_pager_command_with_delta() {
        // Note: This test assumes no explicit [select] pager config is set
        // If user has config, the behavior changes (returns pager_cmd as-is)

        // delta should get --paging=never and --width
        let result = build_pager_command("delta", 120);
        if !has_explicit_pager_config() {
            assert_eq!(result, "delta --paging=never --width=120");
        }

        // delta with existing args should append flags
        let result = build_pager_command("delta --side-by-side", 90);
        if !has_explicit_pager_config() {
            assert_eq!(result, "delta --side-by-side --paging=never --width=90");
        }

        // Full path to delta
        let result = build_pager_command("/usr/bin/delta", 100);
        if !has_explicit_pager_config() {
            assert_eq!(result, "/usr/bin/delta --paging=never --width=100");
        }
    }

    #[test]
    fn test_build_pager_command_with_bat() {
        // bat should get --paging=never and --width
        let result = build_pager_command("bat", 80);
        if !has_explicit_pager_config() {
            assert_eq!(result, "bat --paging=never --width=80");
        }

        // batcat (Debian package name)
        let result = build_pager_command("batcat", 110);
        if !has_explicit_pager_config() {
            assert_eq!(result, "batcat --paging=never --width=110");
        }

        // bat with existing args
        let result = build_pager_command("bat --style=plain", 95);
        if !has_explicit_pager_config() {
            assert_eq!(result, "bat --style=plain --paging=never --width=95");
        }
    }

    #[test]
    fn test_build_pager_command_with_other_pagers() {
        // Pagers that don't need special handling should be returned as-is

        // less - no modifications
        let result = build_pager_command("less -R", 120);
        assert_eq!(result, "less -R");

        // diff-so-fancy - no modifications
        let result = build_pager_command("diff-so-fancy", 100);
        assert_eq!(result, "diff-so-fancy");

        // colordiff - no modifications
        let result = build_pager_command("colordiff | less", 90);
        assert_eq!(result, "colordiff | less");
    }

    #[test]
    fn test_build_pager_command_with_various_widths() {
        // Test different width values
        if !has_explicit_pager_config() {
            // Small width
            let result = build_pager_command("delta", 40);
            assert_eq!(result, "delta --paging=never --width=40");

            // Medium width
            let result = build_pager_command("bat", 80);
            assert_eq!(result, "bat --paging=never --width=80");

            // Large width
            let result = build_pager_command("delta", 200);
            assert_eq!(result, "delta --paging=never --width=200");

            // Very small width (edge case)
            let result = build_pager_command("bat", 1);
            assert_eq!(result, "bat --paging=never --width=1");
        }
    }

    #[test]
    fn test_build_pager_command_case_insensitive() {
        // Windows-style command names should also get width flags
        if !has_explicit_pager_config() {
            let result = build_pager_command("Delta", 100);
            assert_eq!(result, "Delta --paging=never --width=100");

            let result = build_pager_command("BAT", 100);
            assert_eq!(result, "BAT --paging=never --width=100");

            let result = build_pager_command("delta.exe", 100);
            assert_eq!(result, "delta.exe --paging=never --width=100");
        }
    }
}
