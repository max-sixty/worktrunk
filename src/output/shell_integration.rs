//! Shell integration warnings and prompts.
//!
//! # Shell Integration Warning Messages (Complete Spec)
//!
//! When shell integration is not active, warn that cd won't happen.
//!
//! ## Switch to Existing Worktree (`wt switch X` where X exists)
//!
//! | Condition | Warning | Hint |
//! |-----------|---------|------|
//! | Not installed | `Worktree for X @ path, but cannot change directory — shell integration not installed` | `To enable automatic cd, run wt config shell install` |
//! | Needs restart | `Worktree for X @ path, but cannot change directory — shell requires restart` | `Restart shell to activate shell integration` |
//! | Explicit path | `Worktree for X @ path, but cannot change directory — ran ./wt; shell integration wraps wt` | (none) |
//! | Git subcommand | `Worktree for X @ path, but cannot change directory — ran git wt; running through git prevents cd` | `Use git-wt directly (via shell function) for automatic cd` |
//!
//! ## Switch to New Worktree (`wt switch --create X`)
//!
//! Success message shown first, then warning if shell won't cd:
//!
//! | Condition | Success | Warning | Hint |
//! |-----------|---------|---------|------|
//! | Shell active | `Created new worktree for X from base @ path` | (none) | (none) |
//! | Not installed | `Created new worktree for X from base @ path` | `Cannot change directory — shell integration not installed` | `To enable automatic cd, run wt config shell install` |
//! | Git subcommand | `Created new worktree for X from base @ path` | `Cannot change directory — ran git wt; running through git prevents cd` | `Use git-wt directly (via shell function) for automatic cd` |
//!
//! ## After Merge/Remove (switching to main worktree)
//!
//! | Condition | Warning | Hint |
//! |-----------|---------|------|
//! | Shell active | (info) `Switched to worktree for main @ path` | (none) |
//! | Git subcommand | `Cannot change directory — ran git wt; running through git prevents cd` | `Use git-wt directly (via shell function) for automatic cd` |
//! | Other | `Cannot change directory — {reason}` | `To enable automatic cd, run wt config shell install` |
//!
//! ## Prompt Decision Flow
//!
//! When shell integration is not active, [`prompt_shell_integration`] decides what to show:
//!
//! | Condition | Action |
//! |-----------|--------|
//! | Git subcommand | Return early (warning already shown) |
//! | Unsupported shell | Hint: `Shell integration not yet supported for <shell>` |
//! | $SHELL not set | Hint: `To enable automatic cd, run wt config shell install` |
//! | Current shell already installed | Hint: `Restart shell to activate shell integration` |
//! | `skip-shell-integration-prompt` / Non-TTY | Hint: `To enable automatic cd, run wt config shell install` |
//! | TTY | Prompt: `Install shell integration? [y/N/?]` |
//!
//! # Reason Values
//!
//! The [`compute_shell_warning_reason`] function returns these reason strings:
//!
//! | Reason | Meaning |
//! |--------|---------|
//! | `shell integration not installed` | Shell config doesn't have the `eval` line |
//! | `shell requires restart` | Shell config has `eval` line but wrapper not active |
//! | `ran X; shell integration wraps Y` | Invoked with explicit path (e.g., `./target/debug/wt`) |
//!
//! Note: The git subcommand case (`ran git wt; ...`) is handled separately via [`crate::is_git_subcommand`].

use color_print::cformat;

use worktrunk::config::WorktrunkConfig;
use worktrunk::path::format_path_for_display;
use worktrunk::shell::{Shell, extract_filename_from_path};
use worktrunk::styling::hint_message;

/// Shell integration install hint message.
// TODO(hints-count): After showing this hint 5+ times, suggest `wt config show` for diagnostics.
// This requires changing the hints infrastructure to track counts rather than booleans.
// See `Repository::mark_hint_shown()` and `list_shown_hints()` in src/git/repository/mod.rs.
pub(crate) fn shell_integration_hint() -> String {
    cformat!("To enable automatic cd, run <bright-black>wt config shell install</>")
}

/// Hint when shell integration is installed but shell needs restart.
pub(crate) fn shell_restart_hint() -> &'static str {
    "Restart shell to activate shell integration"
}

/// Shell integration hint for unknown/unsupported shell.
fn shell_integration_unsupported_shell(shell_path: &str) -> String {
    // Extract shell name from path, handling both Unix and Windows paths
    // e.g., "/bin/tcsh" -> "tcsh", "C:\...\tcsh.exe" -> "tcsh"
    let shell_name = extract_filename_from_path(shell_path).unwrap_or(shell_path);
    format!(
        "Shell integration not yet supported for {shell_name} (supports bash, zsh, fish, PowerShell)"
    )
}

/// Warning message when running as git subcommand (cd cannot work).
pub(crate) fn git_subcommand_warning() -> String {
    cformat!("Use <bright-black>git-wt</> directly (via shell function) for automatic cd")
}

/// Compute the shell warning reason for display in messages.
///
/// Returns a reason string explaining why shell integration isn't working.
/// See the module documentation for the complete spec of warning messages.
pub(crate) fn compute_shell_warning_reason() -> String {
    let is_configured = Shell::is_integration_configured(&crate::binary_name())
        .ok()
        .flatten()
        .is_some();
    let explicit_path = crate::was_invoked_with_explicit_path();
    let invoked = crate::invocation_path();
    let wraps = crate::binary_name();

    compute_shell_warning_reason_inner(is_configured, explicit_path, &invoked, &wraps)
}

/// Inner logic for computing shell warning reason.
/// Separated for testability - the outer function handles environment queries.
fn compute_shell_warning_reason_inner(
    is_configured: bool,
    explicit_path: bool,
    invoked: &str,
    wraps: &str,
) -> String {
    if is_configured {
        if explicit_path {
            // Invoked with explicit path - shell wrapper won't intercept this binary.
            let invoked_name = std::path::Path::new(invoked)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(invoked);

            // Check if the only difference is .exe suffix (case-insensitive for Windows)
            let invoked_lower = invoked_name.to_lowercase();
            let wraps_lower = wraps.to_lowercase();
            if invoked_lower == format!("{wraps_lower}.exe") {
                // Windows .exe mismatch - give targeted advice
                cformat!(
                    "ran <bold>{invoked_name}</>; use <bold>{wraps}</> (without .exe) for auto-cd"
                )
            } else if invoked_name == wraps {
                // Filename matches but full path differs - show full path for clarity
                // (e.g., "./target/debug/wt" vs "wt" - the path IS the useful info)
                cformat!("ran <bold>{invoked}</>; shell integration wraps <bold>{wraps}</>")
            } else {
                // Different binary name - show just the filename
                cformat!("ran <bold>{invoked_name}</>; shell integration wraps <bold>{wraps}</>")
            }
        } else {
            "shell requires restart".to_string()
        }
    } else {
        "shell integration not installed".to_string()
    }
}

/// Print skipped shells (config file not found).
pub fn print_skipped_shells(
    skipped: &[(worktrunk::shell::Shell, std::path::PathBuf)],
) -> anyhow::Result<()> {
    for (shell, path) in skipped {
        let path = format_path_for_display(path);
        super::print(hint_message(cformat!(
            "Skipped <bright-black>{shell}</>; <bright-black>{path}</> not found"
        )))?;
    }
    Ok(())
}

/// Print the result of shell integration installation.
///
/// Shows:
/// - Configured shells with their paths
/// - Completion results (for fish)
/// - Skipped shells
/// - Summary count
/// - Zsh compinit warning if needed
/// - Restart hint for current shell
///
/// Used by both `wt config shell install` and the interactive prompt after `wt switch`.
pub fn print_shell_install_result(
    scan_result: &crate::commands::configure_shell::ScanResult,
) -> anyhow::Result<()> {
    use crate::commands::configure_shell::ConfigAction;
    use worktrunk::styling::{
        format_bash_with_gutter, info_message, success_message, warning_message,
    };

    // Count shells that became (more) configured
    let shells_configured_count = scan_result
        .configured
        .iter()
        .filter(|ext_result| {
            let ext_changed = !matches!(ext_result.action, ConfigAction::AlreadyExists);
            let comp_changed = scan_result
                .completion_results
                .iter()
                .find(|c| c.shell == ext_result.shell)
                .is_some_and(|c| !matches!(c.action, ConfigAction::AlreadyExists));
            ext_changed || comp_changed
        })
        .count();

    // Show configured shells grouped with their completions
    for result in &scan_result.configured {
        let shell = result.shell;
        let path = format_path_for_display(&result.path);
        // For bash/zsh, completions are inline in the init script
        let what = if matches!(shell, Shell::Bash | Shell::Zsh) {
            "shell extension & completions"
        } else {
            "shell extension"
        };
        let message = cformat!(
            "{} {what} for <bold>{shell}</> @ <bold>{path}</>",
            result.action.description()
        );

        match result.action {
            ConfigAction::Added | ConfigAction::Created => {
                super::print(success_message(message))?;
            }
            ConfigAction::AlreadyExists => {
                super::print(info_message(message))?;
            }
            ConfigAction::WouldAdd | ConfigAction::WouldCreate => {
                unreachable!("Preview actions handled by confirmation prompt")
            }
        }

        // Show completion result for this shell (fish has separate completion files)
        if let Some(comp_result) = scan_result
            .completion_results
            .iter()
            .find(|r| r.shell == shell)
        {
            let comp_path = format_path_for_display(&comp_result.path);
            let comp_message = cformat!(
                "{} completions for <bold>{shell}</> @ <bold>{comp_path}</>",
                comp_result.action.description()
            );
            match comp_result.action {
                ConfigAction::Added | ConfigAction::Created => {
                    super::print(success_message(comp_message))?;
                }
                ConfigAction::AlreadyExists => {
                    super::print(info_message(comp_message))?;
                }
                ConfigAction::WouldAdd | ConfigAction::WouldCreate => {
                    unreachable!("Preview actions handled by confirmation prompt")
                }
            }
        }
    }

    // Show skipped shells
    print_skipped_shells(&scan_result.skipped)?;

    // Summary
    if shells_configured_count > 0 {
        super::blank()?;
        let plural = if shells_configured_count == 1 {
            ""
        } else {
            "s"
        };
        super::print(success_message(format!(
            "Configured {shells_configured_count} shell{plural}"
        )))?;
    } else {
        super::print(success_message("All shells already configured"))?;
    }

    // Zsh compinit advisory
    if scan_result.zsh_needs_compinit {
        super::print(warning_message(
            "Completions require compinit; add to ~/.zshrc before the wt line:",
        ))?;
        super::print(format_bash_with_gutter("autoload -Uz compinit && compinit"))?;
    }

    // Restart hint for current shell
    if shells_configured_count > 0 {
        let current_shell = std::env::var("SHELL")
            .ok()
            .and_then(|s| extract_filename_from_path(&s).map(String::from));

        let current_shell_result = current_shell.as_ref().and_then(|shell_name| {
            scan_result
                .configured
                .iter()
                .filter(|r| !matches!(r.action, ConfigAction::AlreadyExists))
                .find(|r| r.shell.to_string().eq_ignore_ascii_case(shell_name))
        });

        if current_shell_result.is_some() {
            super::print(hint_message(shell_restart_hint()))?;
        }
    }

    Ok(())
}

/// Handle shell integration prompt/hint after switch when shell integration is not active.
///
/// Called after `wt switch` when shell integration is NOT active.
/// See module documentation for the complete decision flow.
///
/// Returns `Ok(true)` if installed, `Ok(false)` otherwise.
pub fn prompt_shell_integration(
    config: &mut WorktrunkConfig,
    binary_name: &str,
    skip_prompt: bool,
) -> anyhow::Result<bool> {
    use crate::commands::configure_shell::{
        ConfigAction, handle_configure_shell, prompt_for_install, scan_shell_configs,
    };
    use std::io::IsTerminal;
    use worktrunk::shell::current_shell;

    // Skip when running as git subcommand - shell integration can't help there
    // (running through git prevents cd, so the shell wrapper won't intercept)
    // The git subcommand warning is already shown by the caller
    if crate::is_git_subcommand() {
        return Ok(false);
    }

    let is_tty = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();

    // Check the current shell (from $SHELL)
    // Only prompt if current shell is supported (so they benefit immediately)
    let shell_env = std::env::var("SHELL").ok();
    if current_shell().is_none() {
        let msg = match &shell_env {
            Some(path) => shell_integration_unsupported_shell(path),
            // $SHELL not set: could be Windows PowerShell, or unusual Unix setup
            // Point them to manual installation
            None => shell_integration_hint(),
        };
        super::print(hint_message(msg))?;
        return Ok(false);
    };

    // Scan ALL shells (same as `wt config shell install`)
    // Only includes shells where config files already exist
    let scan = scan_shell_configs(None, true, binary_name)
        .map_err(|e| anyhow::anyhow!("Failed to scan shell configs: {e}"))?;

    // No config files exist - show install hint
    if scan.configured.is_empty() {
        super::print(hint_message(shell_integration_hint()))?;
        return Ok(false);
    }

    // Check if current shell is already configured (user just needs to restart)
    let current_shell_installed = scan
        .configured
        .iter()
        .filter(|r| Some(r.shell) == current_shell())
        .any(|r| matches!(r.action, ConfigAction::AlreadyExists));

    if current_shell_installed {
        // Shell integration is configured but not active for this invocation
        if !crate::was_invoked_with_explicit_path() {
            // Invoked via PATH but wrapper isn't active - needs shell restart
            super::print(hint_message(shell_restart_hint()))?;
        }
        // For explicit paths: no hint needed - handle_switch_output() warning already explains
        return Ok(false);
    }

    // Can't or shouldn't prompt - show install hint
    if config.skip_shell_integration_prompt || !is_tty || skip_prompt {
        super::print(hint_message(shell_integration_hint()))?;
        return Ok(false);
    }

    // TTY + first time: Show interactive prompt
    // Accepting installs for all shells with config files (same as `wt config shell install`)
    super::blank()?;
    let confirmed = prompt_for_install(
        &scan.configured,
        &scan.completion_results,
        binary_name,
        "Install shell integration?",
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;

    if !confirmed {
        // Only skip future prompts after explicit decline (not Ctrl+C)
        let _ = config.set_skip_shell_integration_prompt(None);
        super::print(hint_message(shell_integration_hint()))?;
        return Ok(false);
    }

    // Install for all shells with config files (same as `wt config shell install`)
    let install_result = handle_configure_shell(None, true, binary_name.to_string())
        .map_err(|e| anyhow::anyhow!("Failed to configure shell integration: {e}"))?;

    print_shell_install_result(&install_result)?;

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_integration_hint() {
        let hint = shell_integration_hint();
        assert!(hint.contains("wt config shell install"));
    }

    #[test]
    fn test_git_subcommand_warning() {
        let warning = git_subcommand_warning();
        assert!(warning.contains("git-wt"));
        assert!(warning.contains("shell function"));
    }

    #[test]
    fn test_compute_shell_warning_reason_not_installed() {
        // Shell integration not configured -> "not installed"
        let reason = compute_shell_warning_reason_inner(false, false, "wt", "wt");
        assert_eq!(reason, "shell integration not installed");
    }

    #[test]
    fn test_compute_shell_warning_reason_explicit_path_same_name() {
        // When filename matches wraps, show full path (the path IS the useful info)
        let reason = compute_shell_warning_reason_inner(true, true, "./target/debug/wt", "wt");
        assert!(reason.contains("./target/debug/wt"));
        assert!(reason.contains("wraps"));
    }

    #[test]
    fn test_compute_shell_warning_reason_explicit_path_different_binary() {
        // When invoked binary differs from wrapped binary, show both
        let reason = compute_shell_warning_reason_inner(true, true, "/usr/local/bin/git-wt", "wt");
        assert!(reason.contains("git-wt"));
        assert!(reason.contains("wt"));
        assert!(reason.contains("wraps"));
    }

    #[test]
    #[cfg(windows)]
    fn test_compute_shell_warning_reason_windows_exe_suffix() {
        // Windows: invoked as git-wt.exe, wraps git-wt -> targeted .exe message
        let reason = compute_shell_warning_reason_inner(
            true,
            true,
            r"C:\Users\user\AppData\Local\Microsoft\WinGet\Packages\git-wt.exe",
            "git-wt",
        );
        // Should extract filename and give targeted advice
        assert!(reason.contains("git-wt.exe"));
        assert!(reason.contains("without .exe"));
        assert!(!reason.contains(r"C:\Users")); // No full path
    }

    #[test]
    #[cfg(windows)]
    fn test_compute_shell_warning_reason_windows_exe_case_insensitive() {
        // Windows paths are case-insensitive
        let reason = compute_shell_warning_reason_inner(true, true, r"C:\path\to\WT.EXE", "wt");
        assert!(reason.contains("without .exe"));
    }

    #[test]
    fn test_compute_shell_warning_reason_needs_restart() {
        // Shell integration configured + NOT explicit path -> "shell requires restart"
        let reason = compute_shell_warning_reason_inner(true, false, "wt", "wt");
        assert_eq!(reason, "shell requires restart");
    }
}
