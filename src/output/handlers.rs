//! Output handlers for worktree operations using the global output context

use color_print::cformat;
use std::path::Path;

use crate::commands::command_executor::CommandContext;
use crate::commands::execute_pre_remove_commands;
use crate::commands::process::spawn_detached;
use crate::commands::worktree::{BranchDeletionMode, RemoveResult, SwitchBranchInfo, SwitchResult};
use worktrunk::config::WorktrunkConfig;
use worktrunk::git::GitError;
use worktrunk::git::IntegrationReason;
use worktrunk::git::Repository;
use worktrunk::git::path_dir_name;
use worktrunk::path::format_path_for_display;
use worktrunk::shell::Shell;
use worktrunk::styling::{
    FormattedMessage, error_message, format_with_gutter, hint_message, info_message,
    progress_message, success_message, suggest_command, warning_message,
};

/// Format a switch message with a consistent location phrase
///
/// Both modes (with and without shell integration) use the human-friendly
/// `"Created new worktree for {branch} from {base} @ {path}"` wording so
/// users see the same message regardless of how worktrunk is invoked.
///
/// Returns unstyled text - callers wrap in success_message() for Created
/// or info_message() for Switched (existing worktree).
fn format_switch_message(
    branch: &str,
    path: &Path,
    created_branch: bool,
    base_branch: Option<&str>,
    from_remote: Option<&str>,
) -> String {
    // Determine action and source based on how the worktree was created
    // Priority: explicit --create > DWIM from remote > existing local branch
    let (action, source) = if created_branch {
        ("Created new worktree for", base_branch)
    } else if let Some(remote) = from_remote {
        ("Created worktree for", Some(remote))
    } else {
        ("Switched to worktree for", None)
    };

    match source {
        Some(src) => cformat!(
            "{action} <bold>{branch}</> from <bold>{src}</> @ <bold>{}</>",
            format_path_for_display(path)
        ),
        None => cformat!(
            "{action} <bold>{branch}</> @ <bold>{}</>",
            format_path_for_display(path)
        ),
    }
}

/// Result of an integration check, including which target was used.
struct IntegrationResult {
    reason: Option<IntegrationReason>,
    /// The target that was actually checked against (may be upstream if ahead of local)
    effective_target: String,
}

/// Check if a branch's content has been integrated into the target.
///
/// Returns the reason if the branch is safe to delete (ordered by check cost):
/// - `SameCommit`: Branch HEAD is literally the same commit as target
/// - `NoAddedChanges`: Branch has no file changes beyond merge-base (empty three-dot diff)
/// - `TreesMatch`: The branch's tree SHA matches the target's tree SHA (squash merge/rebase)
/// - `MergeAddsNothing`: Merge simulation shows branch would add nothing (squash + target advanced)
///
/// Also returns the effective target used (may be upstream if it's ahead of local).
///
/// Returns None reason if no condition is met, or if an error occurs (e.g., invalid refs).
/// This fail-safe default prevents accidental branch deletion when integration cannot
/// be determined.
fn get_integration_reason(repo: &Repository, branch_name: &str, target: &str) -> IntegrationResult {
    let effective_target = repo.effective_integration_target(target);

    let reason = check_integration_against(repo, branch_name, &effective_target);

    IntegrationResult {
        reason,
        effective_target,
    }
}

/// Check integration against a specific target ref.
fn check_integration_against(
    repo: &Repository,
    branch_name: &str,
    target: &str,
) -> Option<IntegrationReason> {
    // Use lazy provider for short-circuit evaluation.
    // Expensive checks (would_merge_add) are skipped if cheaper ones succeed.
    let mut provider = worktrunk::git::LazyGitIntegration::new(repo, branch_name, target);
    worktrunk::git::check_integration(&mut provider)
}

/// Outcome of a branch deletion attempt.
enum BranchDeletionOutcome {
    /// Branch was not deleted (not integrated and not forced)
    NotDeleted,
    /// Branch was force-deleted without integration check
    ForceDeleted,
    /// Branch was deleted because it was integrated
    Integrated(IntegrationReason),
}

/// Result of a branch deletion attempt.
struct BranchDeletionResult {
    outcome: BranchDeletionOutcome,
    /// The target that was actually checked against (may be upstream if ahead of local)
    effective_target: String,
}

/// Attempt to delete a branch if it's integrated or force_delete is set.
///
/// Returns `BranchDeletionResult` with:
/// - `outcome`: Whether/why deletion occurred
/// - `effective_target`: The ref checked against (may be upstream if ahead of local)
fn delete_branch_if_safe(
    repo: &Repository,
    branch_name: &str,
    target: &str,
    force_delete: bool,
) -> anyhow::Result<BranchDeletionResult> {
    let IntegrationResult {
        reason,
        effective_target,
    } = get_integration_reason(repo, branch_name, target);

    // Determine outcome based on integration and force flag
    let outcome = match (reason, force_delete) {
        (Some(r), _) => {
            repo.run_command(&["branch", "-D", branch_name])?;
            BranchDeletionOutcome::Integrated(r)
        }
        (None, true) => {
            repo.run_command(&["branch", "-D", branch_name])?;
            BranchDeletionOutcome::ForceDeleted
        }
        (None, false) => BranchDeletionOutcome::NotDeleted,
    };

    Ok(BranchDeletionResult {
        outcome,
        effective_target,
    })
}

/// Handle the result of a branch deletion attempt.
///
/// Shows appropriate messages for non-deleted branches:
/// - `NotDeleted`: We checked and chose not to delete (not integrated) - show info
/// - `Err(e)`: Git command failed - show warning with actual error
///
/// Returns (result, needs_hint) where needs_hint indicates the caller should print
/// the unmerged branch hint after any success message.
///
/// When `defer_output` is true, info and hint are suppressed (caller will handle).
fn handle_branch_deletion_result(
    result: anyhow::Result<BranchDeletionResult>,
    branch_name: &str,
    defer_output: bool,
) -> anyhow::Result<(BranchDeletionResult, bool)> {
    match result {
        Ok(r) if !matches!(r.outcome, BranchDeletionOutcome::NotDeleted) => Ok((r, false)),
        Ok(r) => {
            // Branch not integrated - we chose not to delete (not a failure)
            if !defer_output {
                super::print(info_message(cformat!(
                    "Branch <bold>{branch_name}</> retained; has unmerged changes"
                )))?;
                let cmd = suggest_command("remove", &[branch_name], &["-D"]);
                super::print(hint_message(cformat!(
                    "To delete the unmerged branch, run <bright-black>{cmd}</>"
                )))?;
            }
            Ok((r, defer_output))
        }
        Err(e) => {
            // Git command failed - this is an error (we decided to delete but couldn't)
            super::print(error_message(cformat!(
                "Failed to delete branch <bold>{branch_name}</>"
            )))?;
            super::print(format_with_gutter(&e.to_string(), None))?;
            Err(e)
        }
    }
}

// ============================================================================
// FlagNote: Workaround for cformat! being compile-time only
// ============================================================================
//
// We want to parameterize the color (cyan/green) but can't because cformat!
// parses color tags at compile time before generic substitution. So we have
// duplicate methods (after_cyan, after_green) instead of after(color).
//
// This is ugly but unavoidable. Keep it encapsulated here.
// ============================================================================

struct FlagNote {
    text: String,
    symbol: Option<String>,
    suffix: String,
}

impl FlagNote {
    fn empty() -> Self {
        Self {
            text: String::new(),
            symbol: None,
            suffix: String::new(),
        }
    }

    fn text_only(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            symbol: None,
            suffix: String::new(),
        }
    }

    fn with_symbol(
        text: impl Into<String>,
        symbol: impl Into<String>,
        suffix: impl Into<String>,
    ) -> Self {
        Self {
            text: text.into(),
            symbol: Some(symbol.into()),
            suffix: suffix.into(),
        }
    }

    fn after_cyan(&self) -> String {
        match &self.symbol {
            Some(s) => cformat!("{}<cyan>{}</>", s, self.suffix),
            None => String::new(),
        }
    }

    fn after_green(&self) -> String {
        match &self.symbol {
            Some(s) => cformat!("{}<green>{}</>", s, self.suffix),
            None => String::new(),
        }
    }
}

// ============================================================================

/// Get flag acknowledgment note for remove messages
///
/// `target_branch`: The branch we checked integration against (shown in reason)
fn get_flag_note(
    deletion_mode: BranchDeletionMode,
    outcome: &BranchDeletionOutcome,
    target_branch: Option<&str>,
) -> FlagNote {
    if deletion_mode.should_keep() {
        return FlagNote::text_only(" (--no-delete-branch)");
    }

    match outcome {
        BranchDeletionOutcome::NotDeleted => FlagNote::empty(),
        BranchDeletionOutcome::ForceDeleted => FlagNote::text_only(" (--force-delete)"),
        BranchDeletionOutcome::Integrated(reason) => {
            let Some(target) = target_branch else {
                return FlagNote::empty();
            };
            let symbol = reason.symbol();
            let desc = reason.description();
            FlagNote::with_symbol(
                cformat!(" ({desc} <bold>{target}</>,"),
                cformat!(" <dim>{symbol}</>"),
                ")",
            )
        }
    }
}

/// Shell integration hint message
fn shell_integration_hint() -> String {
    cformat!("To enable automatic cd, run <bright-black>wt config shell install</>")
}

/// Shell integration hint for unknown/unsupported shell
fn shell_integration_unsupported_shell(shell_path: &str) -> String {
    // Extract shell name from path (e.g., "/bin/tcsh" -> "tcsh")
    let shell_name = shell_path.rsplit('/').next().unwrap_or(shell_path);
    format!(
        "Shell integration not yet supported for {shell_name} (supports bash, zsh, fish, PowerShell)"
    )
}

/// Print skipped shells (config file not found)
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
    use worktrunk::styling::format_bash_with_gutter;

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
            .and_then(|s| s.rsplit('/').next().map(String::from));

        let current_shell_result = current_shell.as_ref().and_then(|shell_name| {
            scan_result
                .configured
                .iter()
                .filter(|r| !matches!(r.action, ConfigAction::AlreadyExists))
                .find(|r| r.shell.to_string().eq_ignore_ascii_case(shell_name))
        });

        if current_shell_result.is_some() {
            super::print(hint_message("Restart shell to activate shell integration"))?;
        }
    }

    Ok(())
}

/// Handle shell integration prompt/hint after switch when shell integration is not active.
///
/// Called after `wt switch` when shell integration is NOT active.
///
/// ## Decision Flow (checked in order)
///
/// | Condition | Action |
/// |-----------|--------|
/// | Unsupported shell | Hint: `Shell integration not yet supported for <shell> (supports bash, zsh, fish, PowerShell)` |
/// | $SHELL not set | Hint: `To enable automatic cd, run wt config shell install` |
/// | Current shell already installed | Hint: `Restart shell to activate shell integration` |
/// | `skip-shell-integration-prompt` / Non-TTY / `--execute` | Hint: `To enable automatic cd, run wt config shell install` |
/// | TTY | Prompt: `❯ Install shell integration? [y/N/?]` |
///
/// When prompting:
/// 1. Show prompt with preview of ALL shells that would be configured
/// 2. On accept, install for ALL shells (equivalent to `wt config shell install`)
/// 3. On decline, set `skip-shell-integration-prompt = true` and show hint
/// 4. On Ctrl+C, exit without setting skip flag (will prompt again next time)
///
/// ## Rationale
///
/// - **Installs all shells**: Accepting is equivalent to `wt config shell install`, not just
///   the current shell. This sets up the user for switching shells later.
/// - **Skip flag only on explicit decline**: Ctrl+C doesn't set the flag, so users who
///   interrupted can see the prompt again next time.
/// - **First-time users (TTY)**: Get a one-time prompt with option to preview (`?`) before deciding
/// - **Users who declined (TTY)**: Get a reminder hint on each switch
/// - **Users with stale install**: Already have the config line, just need to restart shell
/// - **Non-interactive (non-TTY)**: Always show hint (can't prompt)
/// - **Unknown shell**: Show unsupported message
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
            super::print(hint_message("Restart shell to activate shell integration"))?;
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

/// Show switch message when changing directory after worktree removal
fn print_switch_message_if_changed(
    changed_directory: bool,
    main_path: &Path,
) -> anyhow::Result<()> {
    if changed_directory && let Ok(Some(dest_branch)) = Repository::at(main_path).current_branch() {
        let path_display = format_path_for_display(main_path);
        super::print(info_message(cformat!(
            "Switched to worktree for <bold>{dest_branch}</> @ <bold>{path_display}</>"
        )))?;
    }
    Ok(())
}

/// Handle output for a switch operation
///
/// # Shell Integration Warnings
///
/// Always warn when the shell's directory won't change. Users expect to be in
/// the target worktree after switching.
///
/// **When to warn:** Shell integration is not active (`WORKTRUNK_DIRECTIVE_FILE`
/// not set). This applies to both `Existing` and `Created` results.
///
/// **When NOT to warn:**
/// - `AlreadyAt` — user is already in the target directory
/// - Shell integration IS active — cd will happen automatically
///
/// **Warning reasons:**
/// - "shell requires restart" — shell config has `eval` line but wrapper not active
/// - "shell integration not installed" — shell config doesn't have `eval` line
///
/// **Message order for Created:** Success message first, then warning. Creation
/// is a real accomplishment, but users still need to know they won't cd there.
///
/// # Return Value
///
/// Returns `Some(path)` when post-switch hooks should show "@ path" in their
/// announcements (because the user's shell won't be in that directory). This happens when:
/// - Shell integration is not active (user's shell stays in original directory)
///
/// Returns `None` when the user will be in the worktree directory (shell integration
/// active or already at the worktree), so no path annotation needed.
pub fn handle_switch_output(
    result: &SwitchResult,
    branch_info: &SwitchBranchInfo,
    execute_command: Option<&str>,
) -> anyhow::Result<Option<std::path::PathBuf>> {
    // Set target directory for command execution
    super::change_directory(result.path())?;

    let path = result.path();
    let path_display = format_path_for_display(path);
    let branch = branch_info.branch();

    // Check if shell integration is active (directive file set)
    let is_shell_integration_active = super::is_shell_integration_active();

    // Compute shell warning reason once (only if we'll need it)
    let shell_warning_reason: Option<String> = if is_shell_integration_active {
        None
    } else if Shell::is_integration_configured(&crate::binary_name())
        .ok()
        .flatten()
        .is_some()
    {
        if crate::was_invoked_with_explicit_path() {
            // Invoked with explicit path - shell wrapper won't intercept this binary
            let invoked = crate::invocation_path();
            let wraps = crate::binary_name();
            Some(cformat!(
                "ran <bold>{invoked}</>; shell integration wraps <bold>{wraps}</>"
            ))
        } else {
            Some("shell requires restart".to_string())
        }
    } else {
        Some("shell integration not installed".to_string())
    };

    // Show path mismatch warning after the main message
    let path_mismatch_warning = branch_info.expected_path.as_ref().map(|expected| {
        let expected_display = format_path_for_display(expected);
        warning_message(cformat!(
            "Worktree path doesn't match branch name; expected <bold>{expected_display}</> <red>⚑</>"
        ))
    });

    let display_path_for_hooks = match result {
        SwitchResult::AlreadyAt(_) => {
            // Already in target directory — no shell warning needed
            super::print(info_message(cformat!(
                "Already on worktree for <bold>{branch}</> @ <bold>{path_display}</>"
            )))?;
            if let Some(warning) = path_mismatch_warning {
                super::print(warning)?;
            }
            // User is already there - no path annotation needed
            None
        }
        SwitchResult::Existing(_) => {
            if let Some(reason) = &shell_warning_reason {
                // Shell integration not active — warn that shell won't cd
                if let Some(cmd) = execute_command {
                    // --execute: command will run in target dir, but shell stays put
                    super::print(warning_message(cformat!(
                        "Executing <bold>{cmd}</> @ <bold>{path_display}</>, but shell directory unchanged — {reason}"
                    )))?;
                } else {
                    // No --execute: user expected to cd but won't
                    super::print(warning_message(cformat!(
                        "Worktree for <bold>{branch}</> @ <bold>{path_display}</>, but cannot change directory — {reason}"
                    )))?;
                }
                if let Some(warning) = path_mismatch_warning {
                    super::print(warning)?;
                }
                // User won't be there - show path in hook announcements
                Some(path.clone())
            } else {
                // Shell integration active — cd will happen automatically
                super::print(info_message(format_switch_message(
                    branch, path, false, None, None,
                )))?;
                if let Some(warning) = path_mismatch_warning {
                    super::print(warning)?;
                }
                // cd will happen - no path annotation needed
                None
            }
        }
        SwitchResult::Created {
            created_branch,
            base_branch,
            from_remote,
            ..
        } => {
            // Always show success for creation
            super::print(success_message(format_switch_message(
                branch,
                path,
                *created_branch,
                base_branch.as_deref(),
                from_remote.as_deref(),
            )))?;

            // Warn if shell won't cd to the new worktree
            if let Some(reason) = shell_warning_reason {
                if let Some(cmd) = execute_command {
                    super::print(warning_message(cformat!(
                        "Executing <bold>{cmd}</> @ <bold>{path_display}</>, but shell directory unchanged — {reason}"
                    )))?;
                } else {
                    // Don't repeat "Created worktree" — success message above already said that
                    super::print(warning_message(cformat!(
                        "Cannot change directory — {reason}"
                    )))?;
                }
                // User won't be there - show path in hook announcements
                Some(path.clone())
            } else {
                // cd will happen - no path annotation needed
                None
            }
            // Note: No path_mismatch_warning — created worktrees are always at
            // the expected path (SwitchBranchInfo::expected_path is None)
        }
    };

    super::flush()?;
    Ok(display_path_for_hooks)
}

/// Execute the --execute command after hooks have run
pub fn execute_user_command(command: &str) -> anyhow::Result<()> {
    use worktrunk::styling::format_bash_with_gutter;

    // Show what command is being executed (section header + gutter content)
    super::print(progress_message("Executing (--execute):"))?;
    super::print(format_bash_with_gutter(command))?;

    super::execute(command)?;

    Ok(())
}

/// Build shell command for background worktree removal
///
/// `branch_to_delete` is the branch to delete after removing the worktree.
/// Pass `None` for detached HEAD or when branch should be retained.
/// This decision is computed upfront (checking if branch is merged) before spawning the background process.
///
/// `force_worktree` adds `--force` to `git worktree remove`, allowing removal
/// even when the worktree contains untracked files (like build artifacts).
fn build_remove_command(
    worktree_path: &std::path::Path,
    branch_to_delete: Option<&str>,
    force_worktree: bool,
) -> String {
    use shell_escape::escape;

    let worktree_path_str = worktree_path.to_string_lossy();
    let worktree_escaped = escape(worktree_path_str.as_ref().into());

    // TODO: This delay is a timing-based workaround, not a principled fix.
    // The race: after wt exits, the shell wrapper reads the directive file and
    // runs `cd`. But fish (and other shells) may call getcwd() before the cd
    // completes (e.g., for prompt updates), and if the background removal has
    // already deleted the directory, we get "shell-init: error retrieving current
    // directory". A 1s delay is very conservative (shell cd takes ~1-5ms), but
    // deterministic solutions (shell-spawned background, marker file sync) add
    // significant complexity for marginal benefit.
    let delay = "sleep 1";

    // Stop fsmonitor daemon first (best effort - ignore errors)
    // This prevents zombie daemons from accumulating when using builtin fsmonitor
    let stop_fsmonitor = format!(
        "git -C {} fsmonitor--daemon stop 2>/dev/null || true",
        worktree_escaped
    );

    let force_flag = if force_worktree { " --force" } else { "" };

    match branch_to_delete {
        Some(branch_name) => {
            let branch_escaped = escape(branch_name.into());
            format!(
                "{} && {} && git worktree remove{} {} && git branch -D {}",
                delay, stop_fsmonitor, force_flag, worktree_escaped, branch_escaped
            )
        }
        None => {
            format!(
                "{} && {} && git worktree remove{} {}",
                delay, stop_fsmonitor, force_flag, worktree_escaped
            )
        }
    }
}

/// Handle output for a remove operation
///
/// Approval is handled at the gate (command entry point), not here.
pub fn handle_remove_output(
    result: &RemoveResult,
    background: bool,
    verify: bool,
) -> anyhow::Result<()> {
    match result {
        RemoveResult::RemovedWorktree {
            main_path,
            worktree_path,
            changed_directory,
            branch_name,
            deletion_mode,
            target_branch,
            integration_reason,
            force_worktree,
        } => handle_removed_worktree_output(
            main_path,
            worktree_path,
            *changed_directory,
            branch_name.as_deref(),
            *deletion_mode,
            target_branch.as_deref(),
            *integration_reason,
            *force_worktree,
            background,
            verify,
        ),
        RemoveResult::BranchOnly {
            branch_name,
            deletion_mode,
        } => handle_branch_only_output(branch_name, *deletion_mode),
    }
}

/// Handle output for BranchOnly removal (branch exists but no worktree)
fn handle_branch_only_output(
    branch_name: &str,
    deletion_mode: BranchDeletionMode,
) -> anyhow::Result<()> {
    // Warn that no worktree was found (user asked to remove it)
    super::print(warning_message(cformat!(
        "No worktree found for branch <bold>{branch_name}</>"
    )))?;

    // Attempt branch deletion (unless --no-delete-branch was specified)
    if deletion_mode.should_keep() {
        // User explicitly requested no branch deletion - nothing more to do
        super::flush()?;
        return Ok(());
    }

    let repo = worktrunk::git::Repository::current();

    // Get default branch for integration check and reason display
    // Falls back to HEAD if default branch can't be determined
    let default_branch = repo.default_branch().ok();
    let check_target = default_branch.as_deref().unwrap_or("HEAD");

    let result = delete_branch_if_safe(&repo, branch_name, check_target, deletion_mode.is_force());
    let (deletion, _) = handle_branch_deletion_result(result, branch_name, false)?;

    if !matches!(deletion.outcome, BranchDeletionOutcome::NotDeleted) {
        let flag_note = get_flag_note(
            deletion_mode,
            &deletion.outcome,
            Some(&deletion.effective_target),
        );
        let flag_text = &flag_note.text;
        let flag_after = flag_note.after_green();
        super::print(FormattedMessage::new(cformat!(
            "<green>✓ Removed branch <bold>{branch_name}</>{flag_text}</>{flag_after}"
        )))?;
    }

    super::flush()?;
    Ok(())
}

/// Spawn post-switch hooks in the destination worktree after a directory change.
///
/// Called when removing a worktree causes a cd to the main worktree.
/// Only runs if `verify` is true (hooks approved) and `changed_directory` is true.
fn spawn_post_switch_after_remove(
    main_path: &std::path::Path,
    verify: bool,
    changed_directory: bool,
) -> anyhow::Result<()> {
    if !verify || !changed_directory {
        return Ok(());
    }
    let Ok(config) = WorktrunkConfig::load() else {
        return Ok(());
    };
    let dest_repo = Repository::at(main_path);
    let dest_branch = dest_repo.current_branch()?;
    let repo_root = dest_repo.worktree_base()?;
    let ctx = CommandContext::new(
        &dest_repo,
        &config,
        dest_branch,
        main_path,
        &repo_root,
        false, // force=false for CommandContext
    );
    // No display_path needed - changed_directory is true, so user's shell IS going there
    ctx.spawn_post_switch_commands(None)
}

/// Handle output for RemovedWorktree removal
#[allow(clippy::too_many_arguments)]
fn handle_removed_worktree_output(
    main_path: &std::path::Path,
    worktree_path: &std::path::Path,
    changed_directory: bool,
    branch_name: Option<&str>,
    deletion_mode: BranchDeletionMode,
    target_branch: Option<&str>,
    pre_computed_integration: Option<IntegrationReason>,
    force_worktree: bool,
    background: bool,
    verify: bool,
) -> anyhow::Result<()> {
    // 1. Emit cd directive if needed - shell will execute this immediately
    if changed_directory {
        super::change_directory(main_path)?;
        super::flush()?; // Force flush to ensure shell processes the cd
    }

    let repo = worktrunk::git::Repository::current();

    // Execute pre-remove hooks in the worktree being removed
    // Non-zero exit aborts removal (FailFast strategy)
    // For detached HEAD, {{ branch }} expands to "HEAD" in templates
    if verify && let Ok(config) = WorktrunkConfig::load() {
        let target_repo = Repository::at(worktree_path);
        let ctx = CommandContext::new(
            &target_repo,
            &config,
            branch_name,
            worktree_path,
            main_path,
            false, // force=false for CommandContext (not approval-related)
        );
        execute_pre_remove_commands(&ctx, None)?;
    }

    // Handle detached HEAD case (no branch known)
    let Some(branch_name) = branch_name else {
        // No branch associated - just remove the worktree
        if background {
            super::print(progress_message(
                "Removing worktree in background (detached HEAD, no branch to delete)",
            ))?;
            let remove_command = build_remove_command(worktree_path, None, force_worktree);
            spawn_detached(
                &repo,
                main_path,
                &remove_command,
                "detached",
                "remove",
                None,
            )?;
        } else {
            let target_repo = worktrunk::git::Repository::at(worktree_path);
            let _ = target_repo.run_command(&["fsmonitor--daemon", "stop"]);
            if let Err(err) = repo.remove_worktree(worktree_path, force_worktree) {
                return Err(GitError::WorktreeRemovalFailed {
                    branch: path_dir_name(worktree_path).to_string(),
                    path: worktree_path.to_path_buf(),
                    error: err.to_string(),
                }
                .into());
            }
            super::print(success_message(
                "Removed worktree (detached HEAD, no branch to delete)",
            ))?;
        }
        spawn_post_switch_after_remove(main_path, verify, changed_directory)?;
        super::flush()?;
        return Ok(());
    };

    if background {
        // Background mode: spawn detached process

        // Use pre-computed integration reason to avoid race conditions when removing
        // multiple worktrees (background processes can hold git locks)
        let (outcome, effective_target) = if deletion_mode.should_keep() {
            (
                BranchDeletionOutcome::NotDeleted,
                target_branch.map(String::from),
            )
        } else if deletion_mode.is_force() {
            (
                BranchDeletionOutcome::ForceDeleted,
                target_branch.map(String::from),
            )
        } else {
            // Use pre-computed integration reason
            let outcome = match pre_computed_integration {
                Some(r) => BranchDeletionOutcome::Integrated(r),
                None => BranchDeletionOutcome::NotDeleted,
            };
            (outcome, target_branch.map(String::from))
        };

        let should_delete_branch = matches!(
            outcome,
            BranchDeletionOutcome::ForceDeleted | BranchDeletionOutcome::Integrated(_)
        );

        let flag_note = get_flag_note(deletion_mode, &outcome, effective_target.as_deref());
        let flag_text = &flag_note.text;
        let flag_after = flag_note.after_cyan();

        // Reason in parentheses: user flags shown explicitly, integration reason for automatic cleanup
        // Note: We use FormattedMessage directly instead of progress_message() to control
        // where cyan styling ends. Symbol must be inside the <cyan> block to get proper coloring.
        //
        // Message structure by case:
        // - Branch deleted (integrated/force): "worktree & branch in background (reason)"
        // - Branch kept (any reason): "worktree in background" + hint (if relevant)
        let branch_was_integrated = pre_computed_integration.is_some();

        let action = if should_delete_branch {
            // Branch will be deleted (integrated or force-deleted)
            cformat!(
                "<cyan>◎ Removing <bold>{branch_name}</> worktree & branch in background{flag_text}</>{flag_after}"
            )
        } else {
            // Branch kept: hint will explain why (integrated+flag, unmerged, or unmerged+flag)
            cformat!("<cyan>◎ Removing <bold>{branch_name}</> worktree in background</>")
        };
        super::print(FormattedMessage::new(action))?;

        // Show hints for branch status
        if !should_delete_branch {
            if deletion_mode.should_keep() && branch_was_integrated {
                // User kept an integrated branch - show integration info
                let reason = pre_computed_integration.as_ref().unwrap();
                let target = effective_target.as_deref().unwrap_or("target");
                let desc = reason.description();
                let symbol = reason.symbol();
                super::print(hint_message(cformat!(
                    "Branch integrated ({desc} <bold>{target}</>, <dim>{symbol}</>); retained with <bright-black>--no-delete-branch</>"
                )))?;
            } else if !deletion_mode.should_keep() {
                // Unmerged, no flag - show how to force delete
                let cmd = suggest_command("remove", &[branch_name], &["-D"]);
                super::print(hint_message(cformat!(
                    "Branch unmerged; to delete, run <bright-black>{cmd}</>"
                )))?;
            }
            // else: Unmerged + flag - no hint (flag had no effect)
        }

        print_switch_message_if_changed(changed_directory, main_path)?;

        // Build command with the decision we already made
        let remove_command = build_remove_command(
            worktree_path,
            should_delete_branch.then_some(branch_name),
            force_worktree,
        );

        // Spawn the removal in background - runs from main_path (where we cd'd to)
        spawn_detached(
            &repo,
            main_path,
            &remove_command,
            branch_name,
            "remove",
            None,
        )?;

        spawn_post_switch_after_remove(main_path, verify, changed_directory)?;
        super::flush()?;
        Ok(())
    } else {
        // Synchronous mode: remove immediately and report actual results

        // Stop fsmonitor daemon first (best effort - ignore errors)
        // This prevents zombie daemons from accumulating when using builtin fsmonitor
        let target_repo = worktrunk::git::Repository::at(worktree_path);
        let _ = target_repo.run_command(&["fsmonitor--daemon", "stop"]);

        // Track whether branch was actually deleted (will be computed based on deletion attempt)
        if let Err(err) = repo.remove_worktree(worktree_path, force_worktree) {
            return Err(GitError::WorktreeRemovalFailed {
                branch: branch_name.into(),
                path: worktree_path.to_path_buf(),
                error: err.to_string(),
            }
            .into());
        }

        // Delete the branch (unless --no-delete-branch was specified)
        // Only show effective_target in message if we had a meaningful target (not tautological "HEAD" fallback)
        let branch_was_integrated = pre_computed_integration.is_some();

        let (outcome, effective_target, show_unmerged_hint) = if !deletion_mode.should_keep() {
            let deletion_repo = worktrunk::git::Repository::at(main_path);
            let check_target = target_branch.unwrap_or("HEAD");
            let result = delete_branch_if_safe(
                &deletion_repo,
                branch_name,
                check_target,
                deletion_mode.is_force(),
            );
            let (deletion, needs_hint) = handle_branch_deletion_result(result, branch_name, true)?;
            // Only use effective_target for display if we had a real target (not "HEAD" fallback)
            let display_target = target_branch.map(|_| deletion.effective_target);
            (deletion.outcome, display_target, needs_hint)
        } else {
            (
                BranchDeletionOutcome::NotDeleted,
                target_branch.map(String::from),
                false,
            )
        };

        let branch_deleted = matches!(
            outcome,
            BranchDeletionOutcome::ForceDeleted | BranchDeletionOutcome::Integrated(_)
        );
        // Message structure parallel to background mode:
        // - Branch deleted (integrated/force): "worktree & branch (reason)"
        // - Branch kept (any reason): "worktree" + hint (if relevant)
        let msg = if branch_deleted {
            let flag_note = get_flag_note(deletion_mode, &outcome, effective_target.as_deref());
            let flag_text = &flag_note.text;
            let flag_after = flag_note.after_green();
            cformat!(
                "<green>✓ Removed <bold>{branch_name}</> worktree & branch{flag_text}</>{flag_after}"
            )
        } else {
            // Branch kept: hint will explain why (integrated+flag, unmerged, or unmerged+flag)
            cformat!("<green>✓ Removed <bold>{branch_name}</> worktree</>")
        };
        super::print(FormattedMessage::new(msg))?;

        // Show hints for branch status
        if !branch_deleted {
            if deletion_mode.should_keep() && branch_was_integrated {
                // User kept an integrated branch - show integration info
                let reason = pre_computed_integration.as_ref().unwrap();
                let target = effective_target.as_deref().unwrap_or("target");
                let desc = reason.description();
                let symbol = reason.symbol();
                super::print(hint_message(cformat!(
                    "Branch integrated ({desc} <bold>{target}</>, <dim>{symbol}</>); retained with <bright-black>--no-delete-branch</>"
                )))?;
            } else if show_unmerged_hint {
                // Unmerged, no flag - show how to force delete
                let cmd = suggest_command("remove", &[branch_name], &["-D"]);
                super::print(hint_message(cformat!(
                    "Branch unmerged; to delete, run <bright-black>{cmd}</>"
                )))?;
            }
            // else: Unmerged + flag - no hint (flag had no effect)
        }

        print_switch_message_if_changed(changed_directory, main_path)?;

        spawn_post_switch_after_remove(main_path, verify, changed_directory)?;
        super::flush()?;
        Ok(())
    }
}

/// Execute a command in a worktree directory
///
/// Merges stdout into stderr using shell redirection (1>&2) to ensure deterministic output ordering.
/// Per CLAUDE.md guidelines: child process output goes to stderr, worktrunk output goes to stdout.
///
/// If `stdin_content` is provided, it will be piped to the command's stdin. This is used to pass
/// hook context as JSON to hook commands.
///
/// ## Color Bleeding Prevention
///
/// This function explicitly resets ANSI codes on stderr before executing child commands.
///
/// Root cause: Terminal emulators maintain a single rendering state machine. When stdout
/// and stderr both connect to the same TTY, output from both streams passes through this
/// state machine in arrival order. If stdout writes color codes but stderr's output arrives
/// next, the terminal applies stdout's color state to stderr's text. The flush ensures stdout
/// completes, but doesn't reset the terminal state - hence this explicit reset to stderr.
///
/// We write the reset to stderr (not stdout) because:
/// 1. Child process output goes to stderr (per CLAUDE.md guidelines)
/// 2. The reset must reach the terminal before child output
/// 3. Writing to stdout could arrive after stderr due to buffering
///
pub fn execute_command_in_worktree(
    worktree_path: &std::path::Path,
    command: &str,
    stdin_content: Option<&str>,
) -> anyhow::Result<()> {
    use std::io::Write;
    use worktrunk::shell_exec::execute_streaming;
    use worktrunk::styling::{eprint, stderr};

    // Flush stdout before executing command to ensure all our messages appear
    // before the child process output
    super::flush()?;

    // Reset ANSI codes on stderr to prevent color bleeding (see function docs for details)
    // This fixes color bleeding observed when worktrunk prints colored output to stdout
    // followed immediately by child process output to stderr (e.g., pre-commit run output).
    eprint!("{}", anstyle::Reset);
    stderr().flush().ok(); // Ignore flush errors - reset is best-effort, command execution should proceed

    // Execute with stdout→stderr redirect for deterministic ordering
    // Hooks don't need stdin inheritance (inherit_stdin=false)
    execute_streaming(command, worktree_path, true, stdin_content, false, true)?;

    // Flush to ensure all output appears before we continue
    super::flush()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use worktrunk::git::IntegrationReason;

    #[test]
    fn test_format_switch_message() {
        let path = PathBuf::from("/tmp/test");

        // Switched to existing worktree (no creation, no remote)
        let msg = format_switch_message("feature", &path, false, None, None);
        assert!(msg.contains("Switched to worktree for"));
        assert!(msg.contains("feature"));

        // Created new worktree from base branch
        let msg = format_switch_message("feature", &path, true, Some("main"), None);
        assert!(msg.contains("Created new worktree for"));
        assert!(msg.contains("from"));
        assert!(msg.contains("main"));

        // Created worktree from remote (DWIM)
        let msg = format_switch_message("feature", &path, false, None, Some("origin/feature"));
        assert!(msg.contains("Created worktree for"));
        assert!(msg.contains("origin/feature"));
    }

    #[test]
    fn test_get_flag_note() {
        // --no-delete-branch flag (text only, no symbol, no suffix)
        let note = get_flag_note(
            BranchDeletionMode::Keep,
            &BranchDeletionOutcome::NotDeleted,
            None,
        );
        assert_eq!(note.text, " (--no-delete-branch)");
        assert!(note.symbol.is_none());
        assert!(note.suffix.is_empty());

        // NotDeleted without flag (empty)
        let note = get_flag_note(
            BranchDeletionMode::SafeDelete,
            &BranchDeletionOutcome::NotDeleted,
            None,
        );
        assert!(note.text.is_empty());
        assert!(note.symbol.is_none());
        assert!(note.suffix.is_empty());

        // Force deleted (text only, no symbol, no suffix)
        let note = get_flag_note(
            BranchDeletionMode::ForceDelete,
            &BranchDeletionOutcome::ForceDeleted,
            None,
        );
        assert_eq!(note.text, " (--force-delete)");
        assert!(note.symbol.is_none());
        assert!(note.suffix.is_empty());

        // Integration reasons - text includes description and target, symbol is separate, suffix is closing paren
        let cases = [
            (IntegrationReason::SameCommit, "same commit as"),
            (IntegrationReason::Ancestor, "ancestor of"),
            (IntegrationReason::NoAddedChanges, "no added changes on"),
            (IntegrationReason::TreesMatch, "tree matches"),
            (IntegrationReason::MergeAddsNothing, "all changes in"),
        ];
        for (reason, expected_desc) in cases {
            let note = get_flag_note(
                BranchDeletionMode::SafeDelete,
                &BranchDeletionOutcome::Integrated(reason),
                Some("main"),
            );
            assert!(
                note.text.contains(expected_desc),
                "reason {:?} text should contain '{}'",
                reason,
                expected_desc
            );
            assert!(
                note.text.contains("main"),
                "reason {:?} text should contain target 'main'",
                reason
            );
            assert!(
                note.symbol.is_some(),
                "reason {:?} should have a symbol",
                reason
            );
            let symbol = note.symbol.as_ref().unwrap();
            assert!(
                symbol.contains(reason.symbol()),
                "reason {:?} symbol part should contain the symbol",
                reason
            );
            assert_eq!(
                note.suffix, ")",
                "reason {:?} suffix should be closing paren",
                reason
            );
        }
    }

    #[test]
    fn test_shell_integration_hint() {
        let hint = shell_integration_hint();
        assert!(hint.contains("wt config shell install"));
    }

    #[test]
    fn test_build_remove_command() {
        let path = PathBuf::from("/tmp/test-worktree");

        // Without branch deletion, without force
        let cmd = build_remove_command(&path, None, false);
        assert!(cmd.contains("git worktree remove"));
        assert!(cmd.contains("/tmp/test-worktree"));
        assert!(!cmd.contains("branch -D"));
        assert!(!cmd.contains("--force"));

        // With branch deletion, without force
        let cmd = build_remove_command(&path, Some("feature-branch"), false);
        assert!(cmd.contains("git worktree remove"));
        assert!(cmd.contains("git branch -D"));
        assert!(cmd.contains("feature-branch"));
        assert!(!cmd.contains("--force"));

        // With force flag
        let cmd = build_remove_command(&path, None, true);
        assert!(cmd.contains("git worktree remove --force"));

        // With branch deletion and force
        let cmd = build_remove_command(&path, Some("feature-branch"), true);
        assert!(cmd.contains("git worktree remove --force"));
        assert!(cmd.contains("git branch -D"));

        // Shell escaping for special characters
        let special_path = PathBuf::from("/tmp/test worktree");
        let cmd = build_remove_command(&special_path, Some("feature/branch"), false);
        assert!(cmd.contains("worktree remove"));
    }

    #[test]
    fn test_branch_deletion_outcome_matching() {
        // Ensure the match patterns work correctly
        let outcomes = [
            (BranchDeletionOutcome::NotDeleted, false),
            (BranchDeletionOutcome::ForceDeleted, true),
            (
                BranchDeletionOutcome::Integrated(IntegrationReason::SameCommit),
                true,
            ),
        ];
        for (outcome, expected_deleted) in outcomes {
            let deleted = matches!(
                outcome,
                BranchDeletionOutcome::ForceDeleted | BranchDeletionOutcome::Integrated(_)
            );
            assert_eq!(deleted, expected_deleted);
        }
    }
}
