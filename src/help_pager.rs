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

use std::io::{IsTerminal, Write};
use std::process::{Command, Stdio};

fn validate_pager(s: &str) -> Option<String> {
    let trimmed = s.trim();
    (!trimmed.is_empty() && trimmed != "cat").then(|| trimmed.to_string())
}

/// Detect pager for help output, following git's pager precedence.
///
/// Checks in order: GIT_PAGER → git config core.pager → PAGER → "less"
fn detect_help_pager() -> Option<String> {
    // Check environment variables in git's precedence order
    std::env::var("GIT_PAGER")
        .ok()
        .and_then(|s| validate_pager(&s))
        .or_else(|| {
            // Try git config core.pager
            Command::new("git")
                .args(["config", "--get", "core.pager"])
                .output()
                .inspect_err(|e| log::debug!("Failed to run git config: {}", e))
                .ok()
                .and_then(|output| {
                    if !output.status.success() {
                        log::debug!("git config exited with status: {}", output.status);
                    }
                    String::from_utf8(output.stdout)
                        .inspect_err(|e| log::debug!("git config output not UTF-8: {}", e))
                        .ok()
                        .and_then(|s| validate_pager(&s))
                })
        })
        .or_else(|| std::env::var("PAGER").ok().and_then(|s| validate_pager(&s)))
        .or_else(|| Some("less".to_string())) // Default fallback (may fail if less not installed)
}

/// Show help text through a pager with TTY access for interactive scrolling.
///
/// Only uses pager when stdout is a terminal. Falls back to direct output if:
/// - No pager configured (prints to stderr)
/// - stdout is not a TTY (prints to stderr)
/// - Pager spawn fails (prints to stderr)
///
/// Note: All fallbacks output to stderr for consistency with pager behavior
/// (which sends output to stderr via `>&2`). This ensures `config show --internal`
/// works correctly in directive mode where stdout is reserved for shell directives.
pub fn show_help_in_pager(help_text: &str) -> std::io::Result<()> {
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
    #[cfg(unix)]
    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(&final_cmd)
        .stdin(Stdio::piped())
        .env("LESS", &less_flags)
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            log::debug!("Failed to spawn pager '{}': {}", pager_cmd, e);
            eprint!("{}", help_text);
            return Ok(());
        }
    };

    #[cfg(windows)]
    let mut child = match Command::new("cmd")
        .arg("/C")
        .arg(&final_cmd)
        .stdin(Stdio::piped())
        .env("LESS", &less_flags)
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            log::debug!("Failed to spawn pager '{}': {}", pager_cmd, e);
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
    #[test]
    fn test_validate_excludes_cat() {
        assert_eq!(super::validate_pager("cat"), None);
        assert_eq!(super::validate_pager("  cat  "), None);
        assert_eq!(super::validate_pager(""), None);
        assert_eq!(super::validate_pager("  "), None);
    }

    #[test]
    fn test_validate_accepts_valid_pagers() {
        assert_eq!(super::validate_pager("less"), Some("less".to_string()));
        assert_eq!(super::validate_pager("  less  "), Some("less".to_string()));
        assert_eq!(super::validate_pager("delta"), Some("delta".to_string()));
        assert_eq!(
            super::validate_pager("less -R"),
            Some("less -R".to_string())
        );
    }
}
