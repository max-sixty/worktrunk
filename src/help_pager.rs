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

use crate::pager::{git_config_pager, parse_pager_value};

/// Detect pager for help output, following git's pager precedence.
///
/// Checks in order: GIT_PAGER → git config core.pager → PAGER → "less"
///
/// On Windows without Git Bash, returns None if only `less` would be selected
/// (since `less` isn't available without Git for Windows).
fn detect_help_pager() -> Option<String> {
    let shell = ShellConfig::get();

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
/// Only uses pager when stdout or stderr is a terminal. Falls back to direct output if:
/// - No pager configured (prints to stderr)
/// - Neither stdout nor stderr is a TTY (prints to stderr)
/// - Pager spawn fails (prints to stderr)
///
/// Note: All fallbacks output to stderr for consistency with pager behavior
/// (which sends output to stderr via `>&2`). This ensures `config show`
/// works correctly since stdout is reserved for data output.
pub(crate) fn show_help_in_pager(help_text: &str) -> std::io::Result<()> {
    let Some(pager_cmd) = detect_help_pager() else {
        log::debug!("No pager configured, printing help directly to stderr");
        eprint!("{}", help_text);
        return Ok(());
    };

    // Check if stdout OR stderr is a TTY
    // stdout check: direct invocation (cargo run -- --help)
    // stderr check: shell wrapper (wt --help) redirects stdout but preserves stderr
    let is_tty = std::io::stdout().is_terminal() || std::io::stderr().is_terminal();

    if !is_tty {
        log::debug!("Neither stdout nor stderr is a TTY, skipping pager");
        eprint!("{}", help_text);
        return Ok(());
    }

    log::debug!("Invoking pager: {}", pager_cmd);

    // LESS flags: F=quit if one screen, R=allow colors, X=no termcap init
    let less_flags = std::env::var("LESS").unwrap_or_else(|_| "FRX".to_string());

    // Always send pager output to stderr (standard for help text, like git)
    // This works in all cases: direct invocation, shell wrapper, piping, etc.
    // Note: pager_cmd is expected to be valid shell code (like git's core.pager).
    // Users with paths containing special chars must quote them in their config.
    let final_cmd = format!("{} >&2", pager_cmd);

    // Spawn pager with TTY access (interactive, unlike detached diff renderer)
    // Falls back to direct output if pager unavailable (e.g., less not installed)
    let shell = ShellConfig::get();
    let mut cmd = shell.command(&final_cmd);
    // Prevent subprocesses from writing to the directive file
    cmd.env_remove(worktrunk::shell_exec::DIRECTIVE_FILE_ENV_VAR);
    let mut child = match cmd.stdin(Stdio::piped()).env("LESS", &less_flags).spawn() {
        Ok(child) => child,
        Err(e) => {
            log::debug!(
                "Failed to spawn pager '{}' with {}: {}",
                pager_cmd,
                shell.name,
                e
            );
            eprint!("{}", help_text);
            return Ok(());
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(help_text.as_bytes())?;
    }

    child.wait()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::pager::parse_pager_value;

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
}
