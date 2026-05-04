//! Help text pager integration for CLI help output.
//!
//! Provides pager support for `--help` output, following git's pager precedence:
//! GIT_PAGER → core.pager config → PAGER environment variable → "less" default.
//!
//! # Difference from diff pager
//!
//! This pager is INTERACTIVE (spawned with TTY access) unlike the diff renderer
//! in src/git/repository/mod.rs which is DETACHED (spawned via setsid). This is
//! intentional:
//!
//! - Help pager: Top-level user command, needs TTY for interactive scrolling
//! - Diff renderer: Used in TUI contexts (skim preview), must be detached to
//!   prevent hangs from TTY access
//!
//! Both follow git's pager detection but spawn differently based on their usage context.
//!
//! # Cross-Platform Support
//!
//! On Windows, Git Bash (if available) enables standard pagers like `less`.
//! Without Git Bash, we only use a pager if the configured command works under
//! the PowerShell fallback; otherwise we print directly.

use std::io::{IsTerminal, Write};
use std::process::Stdio;
use worktrunk::shell_exec::ShellConfig;
use worktrunk::styling::print;

use crate::pager::{git_config_pager, parse_pager_value};

/// Detect pager for help output, following git's pager precedence.
///
/// Checks in order: GIT_PAGER → git config core.pager → PAGER → "less"
///
/// On Windows without Git Bash, returns None if only `less` would be selected
/// (since `less` isn't available without Git for Windows).
fn detect_help_pager() -> Option<String> {
    let shell = ShellConfig::get().ok()?;

    // Check environment variables in git's precedence order
    let pager = std::env::var("GIT_PAGER")
        .ok()
        .and_then(|s| parse_pager_value(&s))
        .or_else(git_config_pager)
        .or_else(|| {
            std::env::var("PAGER")
                .ok()
                .and_then(|s| parse_pager_value(&s))
        });

    // If user explicitly configured a pager, use it
    if pager.is_some() {
        return pager;
    }

    // Default to "less" only if we have a POSIX shell (Unix or Git Bash on Windows)
    // Without Git Bash, less isn't typically available on Windows
    if shell.is_posix() {
        Some("less".to_string())
    } else {
        log::debug!("No POSIX shell available, skipping pager (less not available)");
        None
    }
}

/// Show help text through a pager with TTY access for interactive scrolling.
///
/// The `use_pager` flag controls whether to attempt pager display:
/// - `true` (--help): Uses pager when available and terminal is detected
/// - `false` (-h): Always prints directly to stdout, never uses pager
///
/// This follows git's convention where `-h` never opens a pager (muscle-memory safe)
/// while `--help` uses a pager for longer content.
///
/// Even when `use_pager=true`, falls back to direct output if:
/// - No pager configured (prints to stdout)
/// - stdout is not a TTY (prints to stdout)
/// - Pager spawn fails (prints to stdout)
/// - Pager closes stdin early or wait fails (prints to stdout)
///
/// Help text goes to stdout — POSIX convention (`wt --help | less` should work
/// without redirection), matching `cargo`, `curl`, `python`, `git <cmd> -h`,
/// and `--version` (see #2072).
pub(crate) fn show_help_in_pager(help_text: &str, use_pager: bool) {
    // Short help (-h) never uses a pager
    if !use_pager {
        log::debug!("Short help (-h) requested, printing directly to stdout");
        print!("{}", help_text);
        return;
    }

    let Some(pager_cmd) = detect_help_pager() else {
        log::debug!("No pager configured, printing help directly to stdout");
        print!("{}", help_text);
        return;
    };

    // Only page when our output destination is a terminal.
    // If stdout is piped/redirected (e.g., `wt --help | grep foo`), print directly.
    if !std::io::stdout().is_terminal() {
        log::debug!("stdout is not a TTY, skipping pager");
        print!("{}", help_text);
        return;
    }

    log::debug!("Invoking pager: {}", pager_cmd);
    if let Err(e) = pipe_through_pager(&pager_cmd, help_text) {
        log::debug!("Pager failed, falling back to stdout: {}", e);
        print!("{}", help_text);
    }
}

/// Pipe `help_text` to the given pager command and wait for it to exit.
///
/// Spawn / write / wait failures all surface as `Err` so the caller can fall back
/// to direct stdout. Extracted from `show_help_in_pager` so the failure paths can
/// be exercised by unit tests with stand-in commands (`true`, `cat > /dev/null`)
/// that don't depend on a real TTY.
fn pipe_through_pager(pager_cmd: &str, help_text: &str) -> std::io::Result<()> {
    let less_flags = compute_less_flags(std::env::var("LESS").ok().as_deref());
    let shell = ShellConfig::get().map_err(std::io::Error::other)?;
    log::debug!("$ {} (pager)", pager_cmd);
    let mut cmd = shell.command(pager_cmd);
    worktrunk::shell_exec::scrub_directive_env_vars(&mut cmd);
    let mut child = cmd.stdin(Stdio::piped()).env("LESS", &less_flags).spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(help_text.as_bytes())?;
    }
    child.wait()?;
    Ok(())
}

/// Compute LESS flags by appending our required flags to user's existing LESS setting.
///
/// Returns flags suitable for setting LESS env var when spawning less.
/// Ensures F (quit if one screen), R (colors), X (no termcap init) are always active.
fn compute_less_flags(user_less: Option<&str>) -> String {
    format!("{} -FRX", user_less.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::{compute_less_flags, parse_pager_value};

    /// `cat > /dev/null` exercises the full spawn → write → wait path with a
    /// stand-in pager that always succeeds. Covers every line of
    /// `pipe_through_pager`'s body; the `?` propagations are line-hit even on
    /// the Ok branch.
    #[cfg(unix)]
    #[test]
    fn test_pipe_through_pager_pipes_to_real_command() {
        super::pipe_through_pager("cat > /dev/null", "help text").expect("cat should succeed");
    }

    #[test]
    fn test_validate_excludes_cat() {
        assert_eq!(parse_pager_value("cat"), None);
        assert_eq!(parse_pager_value("  cat  "), None);
        assert_eq!(parse_pager_value(""), None);
        assert_eq!(parse_pager_value("  "), None);
    }

    #[test]
    fn test_validate_accepts_valid_pagers() {
        assert_eq!(parse_pager_value("less"), Some("less".to_string()));
        assert_eq!(parse_pager_value("  less  "), Some("less".to_string()));
        assert_eq!(parse_pager_value("delta"), Some("delta".to_string()));
        assert_eq!(parse_pager_value("less -R"), Some("less -R".to_string()));
    }

    #[test]
    fn test_compute_less_flags_empty() {
        // Leading space is fine - less ignores it
        assert_eq!(compute_less_flags(None), " -FRX");
        assert_eq!(compute_less_flags(Some("")), " -FRX");
    }

    #[test]
    fn test_compute_less_flags_short_options() {
        // Common case: user has -R (oh-my-zsh default)
        assert_eq!(compute_less_flags(Some("-R")), "-R -FRX");
        // User has multiple short flags
        assert_eq!(compute_less_flags(Some("-iMRS")), "-iMRS -FRX");
    }

    #[test]
    fn test_compute_less_flags_long_options() {
        // Issue #594: --mouse must not become --mouseFRX
        assert_eq!(compute_less_flags(Some("--mouse")), "--mouse -FRX");
        // Multiple long options
        assert_eq!(
            compute_less_flags(Some("--mouse --shift=4")),
            "--mouse --shift=4 -FRX"
        );
    }

    #[test]
    fn test_compute_less_flags_mixed() {
        assert_eq!(compute_less_flags(Some("-R --mouse")), "-R --mouse -FRX");
    }
}
