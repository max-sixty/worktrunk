//! Output handlers for worktree operations using the global output context

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anstyle::AnsiColor;
use color_print::cformat;
use worktrunk::shell_exec::Cmd;
use worktrunk::styling::{eprint, format_bash_with_gutter, stderr};

use crate::commands::command_executor::CommandContext;
use crate::commands::command_executor::FailureStrategy;
use crate::commands::hooks::{
    announce_and_spawn_background_hooks, execute_hook, prepare_background_hooks,
};
use crate::commands::process::{
    HookLog, InternalOp, build_remove_command, build_remove_command_staged, spawn_detached,
};
use crate::commands::worktree::hooks::PostRemoveContext;
use crate::commands::worktree::{RemoveResult, SwitchBranchInfo, SwitchResult};
use worktrunk::config::UserConfig;
use worktrunk::git::GitError;
use worktrunk::git::IntegrationReason;
use worktrunk::git::Repository;
use worktrunk::git::path_dir_name;
use worktrunk::git::{
    BranchDeletionMode, BranchDeletionOutcome, BranchDeletionResult, RemoveOptions,
    remove_worktree_with_cleanup, stage_worktree_removal,
};
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{
    FormattedMessage, eprintln, error_message, format_with_gutter, hint_message, info_message,
    progress_message, success_message, suggest_command, warning_message,
};

use super::shell_integration::{
    compute_shell_warning_reason, explicit_path_hint, git_subcommand_warning,
    shell_integration_hint, should_show_explicit_path_hint,
};

// ============================================================================
// Background Removal Helpers
// ============================================================================

/// Spawn background worktree removal: stop fsmonitor, rename-then-prune, spawn detached rm.
///
/// Shared sequence for both detached HEAD and branch background removal paths.
/// The caller is responsible for output messages before this call, and hooks after.
fn spawn_background_removal(
    repo: &Repository,
    main_path: &Path,
    worktree_path: &Path,
    branch_to_delete: Option<&str>,
    force_worktree: bool,
    log_label: &str,
    changed_directory: bool,
) -> anyhow::Result<()> {
    // Stop fsmonitor daemon BEFORE rename (must happen while path still exists).
    // Best effort — prevents zombie daemons from accumulating.
    let _ = repo
        .worktree_at(worktree_path)
        .run_command(&["fsmonitor--daemon", "stop"]);

    let remove_command = execute_instant_removal_or_fallback(
        repo,
        worktree_path,
        branch_to_delete,
        force_worktree,
        changed_directory,
    );

    spawn_detached(
        repo,
        main_path,
        &remove_command,
        log_label,
        &HookLog::internal(InternalOp::Remove),
        None,
    )?;
    Ok(())
}

/// Execute instant worktree removal via rename-then-prune, returning the background command.
///
/// This function has side effects: it renames the worktree directory and prunes git metadata.
/// On the fast path, the branch is also deleted synchronously (since after prune, the branch
/// is no longer checked out in any worktree), and the background command is just `rm -rf`.
/// If rename fails (cross-filesystem, permissions, Windows file locking), returns the legacy
/// `git worktree remove` command with branch deletion deferred to the background.
///
/// The caller is responsible for spawning the returned command in the background.
fn execute_instant_removal_or_fallback(
    repo: &Repository,
    worktree_path: &Path,
    branch_to_delete: Option<&str>,
    force_worktree: bool,
    changed_directory: bool,
) -> String {
    // Fast path: rename worktree into .git/wt/trash/ (instant on same filesystem),
    // prune git metadata, then background process just does `rm -rf`.
    if let Some(staged_path) = stage_worktree_removal(repo, worktree_path) {
        // Delete branch synchronously now that prune has removed the worktree metadata.
        // The branch is no longer checked out, so `git branch -D` will succeed.
        // This avoids a race where the user creates a new worktree with the same branch
        // name before the background `rm -rf` completes.
        if let Some(branch) = branch_to_delete
            && let Err(e) = repo.run_command(&["branch", "-D", branch])
        {
            log::debug!("Failed to delete branch {} synchronously: {}", branch, e);
        }
        if changed_directory {
            // Create an empty placeholder at the original path so the shell's working
            // directory ($env.PWD) remains valid until the wrapper has cd'd away.
            // Without this, shells that validate PWD (notably Nushell) emit errors
            // between binary exit and the cd directive executing.
            // Best-effort: if create_dir fails (permissions, race), the only effect
            // is that Nushell may still emit PWD errors — not a correctness issue.
            let _ = std::fs::create_dir(worktree_path);
        }
        build_remove_command_staged(&staged_path, worktree_path, changed_directory)
    } else {
        // Fallback: cross-filesystem, permissions, Windows file locking, etc.
        // Use legacy git worktree remove which handles these cases.
        // Branch deletion stays in the background command since the worktree
        // still references the branch until `git worktree remove` runs.
        //
        // Git refuses to remove worktrees with initialized submodules without
        // --force. We preemptively set --force when .gitmodules exists — broader
        // than checking initialization, but harmless for clean worktrees.
        //
        // TOCTOU note: the clean check runs during planning in
        // prepare_worktree_removal(). In this fallback path, we may add --force
        // later (at execution time) when .gitmodules is present. That creates a
        // small check-vs-use window where newly introduced changes could be
        // removed. See remove_worktree() docs for the detailed safety analysis.
        let force = force_worktree || worktree_path.join(".gitmodules").exists();
        build_remove_command(worktree_path, branch_to_delete, force, changed_directory)
    }
}

/// List top-level entries remaining in a directory after a failed removal.
///
/// Returns None if the directory doesn't exist, can't be read, or is empty.
/// Entries are sorted, with directories suffixed with `/`.
fn list_remaining_entries(path: &Path) -> Option<Vec<String>> {
    let mut entries: Vec<String> = std::fs::read_dir(path)
        .ok()?
        .filter_map(|e| {
            let e = e.ok()?;
            let name = e.file_name().to_string_lossy().into_owned();
            if e.file_type().ok()?.is_dir() {
                Some(format!("{name}/"))
            } else {
                Some(name)
            }
        })
        .collect();
    entries.sort();
    (!entries.is_empty()).then_some(entries)
}

// ============================================================================
// Switch Output Handlers
// ============================================================================

/// Format a switch message based on what was created
///
/// # Message formats
/// - Branch + worktree created (`--create`): "Created branch X from Y and worktree @ path"
/// - Branch from remote + worktree (DWIM): "Created branch X (tracking remote) and worktree @ path"
/// - Worktree only created: "Created worktree for X @ path"
/// - Switched to existing: "Switched to worktree for X @ path"
fn format_switch_message(
    branch: &str,
    path: &Path,
    worktree_created: bool,
    created_branch: bool,
    base_branch: Option<&str>,
    from_remote: Option<&str>,
) -> String {
    let path_display = format_path_for_display(path);

    if created_branch {
        // --create flag: created branch and worktree
        match base_branch {
            Some(base) => cformat!(
                "Created branch <bold>{branch}</> from <bold>{base}</> and worktree @ <bold>{path_display}</>"
            ),
            None => {
                cformat!("Created branch <bold>{branch}</> and worktree @ <bold>{path_display}</>")
            }
        }
    } else if let Some(remote) = from_remote {
        // DWIM from remote: created local tracking branch and worktree
        cformat!(
            "Created branch <bold>{branch}</> (tracking <bold>{remote}</>) and worktree @ <bold>{path_display}</>"
        )
    } else if worktree_created {
        // Local branch existed, created worktree only
        cformat!("Created worktree for <bold>{branch}</> @ <bold>{path_display}</>")
    } else {
        // Switched to existing worktree
        cformat!("Switched to worktree for <bold>{branch}</> @ <bold>{path_display}</>")
    }
}

/// Format a branch-worktree mismatch warning message.
///
/// Shows when a worktree is at a path that doesn't match the config template.
/// Displays both the actual location and the expected location.
fn format_path_mismatch_warning(
    branch: &str,
    actual_path: &Path,
    expected_path: &Path,
) -> FormattedMessage {
    let actual_display = format_path_for_display(actual_path);
    let expected_display = format_path_for_display(expected_path);
    warning_message(cformat!(
        "Branch-worktree mismatch: <bold>{branch}</> @ <bold>{actual_display}</>, expected @ <bold>{expected_display}</> <red>⚑</>"
    ))
}

struct SwitchOutputContext {
    path: PathBuf,
    path_display: String,
    branch: String,
    shell_warning_reason: Option<String>,
    user_wont_be_in_worktree: bool,
    branch_worktree_mismatch_warning: Option<FormattedMessage>,
    is_git_subcommand: bool,
}

fn build_switch_output_context(
    result: &SwitchResult,
    branch_info: &SwitchBranchInfo,
    change_dir: bool,
) -> SwitchOutputContext {
    let path = super::to_logical_path(result.path());
    let path_display = format_path_for_display(&path);
    let branch = branch_info
        .branch
        .clone()
        .unwrap_or_else(|| "detached worktree".to_string());

    let is_git_subcommand = crate::is_git_subcommand();
    let is_shell_integration_active = super::is_shell_integration_active();
    let shell_warning_reason = if !change_dir || is_shell_integration_active {
        None
    } else if is_git_subcommand {
        Some("ran git wt; running through git prevents cd".to_string())
    } else {
        Some(compute_shell_warning_reason())
    };
    let user_wont_be_in_worktree = !change_dir || shell_warning_reason.is_some();
    let branch_worktree_mismatch_warning = branch_info
        .expected_path
        .as_ref()
        .map(|expected| format_path_mismatch_warning(&branch, &path, expected));

    SwitchOutputContext {
        path,
        path_display,
        branch,
        shell_warning_reason,
        user_wont_be_in_worktree,
        branch_worktree_mismatch_warning,
        is_git_subcommand,
    }
}

fn print_switch_path_mismatch_warning(ctx: &SwitchOutputContext) {
    if let Some(warning) = &ctx.branch_worktree_mismatch_warning {
        eprintln!("{}", warning);
    }
}

fn print_switch_directory_hint(branch: &str, is_git_subcommand: bool) {
    if is_git_subcommand {
        eprintln!("{}", hint_message(git_subcommand_warning()));
    } else if should_show_explicit_path_hint() {
        eprintln!("{}", hint_message(explicit_path_hint(branch)));
    }
}

fn handle_switch_already_at_output(ctx: &SwitchOutputContext) -> Option<PathBuf> {
    print_switch_path_mismatch_warning(ctx);
    eprintln!(
        "{}",
        info_message(cformat!(
            "Already on worktree for <bold>{}</> @ <bold>{}</>",
            ctx.branch,
            ctx.path_display
        ))
    );
    None
}

fn handle_switch_existing_output(ctx: &SwitchOutputContext) -> Option<PathBuf> {
    print_switch_path_mismatch_warning(ctx);

    if let Some(reason) = &ctx.shell_warning_reason {
        eprintln!(
            "{}",
            warning_message(cformat!(
                "Worktree for <bold>{}</> @ <bold>{}</>, but cannot change directory — {reason}",
                ctx.branch,
                ctx.path_display
            ))
        );
        print_switch_directory_hint(&ctx.branch, ctx.is_git_subcommand);
    } else {
        eprintln!(
            "{}",
            info_message(format_switch_message(
                &ctx.branch,
                &ctx.path,
                false, // worktree_created
                false, // created_branch
                None,
                None,
            ))
        );
    }

    ctx.user_wont_be_in_worktree.then(|| ctx.path.clone())
}

fn maybe_print_worktree_path_hint(created_branch: bool) {
    if !created_branch {
        return;
    }

    if let Ok(repo) = worktrunk::git::Repository::current() {
        let has_custom_config = UserConfig::load()
            .map(|c| {
                c.has_custom_worktree_path()
                    || repo
                        .project_identifier()
                        .ok()
                        .is_some_and(|p| c.has_project_worktree_path(&p))
            })
            .unwrap_or(false);
        if !has_custom_config && !repo.has_shown_hint("worktree-path") {
            let hint = hint_message(cformat!(
                "To customize worktree locations, run <underline>wt config create</>"
            ));
            eprintln!("{}", hint);
            let _ = repo.mark_hint_shown("worktree-path");
        }
    }
}

fn handle_switch_created_output(
    ctx: &SwitchOutputContext,
    created_branch: bool,
    base_branch: Option<&str>,
    from_remote: Option<&str>,
) -> Option<PathBuf> {
    eprintln!(
        "{}",
        success_message(format_switch_message(
            &ctx.branch,
            &ctx.path,
            true, // worktree_created
            created_branch,
            base_branch,
            from_remote,
        ))
    );

    maybe_print_worktree_path_hint(created_branch);

    if let Some(reason) = &ctx.shell_warning_reason {
        eprintln!(
            "{}",
            warning_message(cformat!("Cannot change directory — {reason}"))
        );
        print_switch_directory_hint(&ctx.branch, ctx.is_git_subcommand);
    }

    ctx.user_wont_be_in_worktree.then(|| ctx.path.clone())
}

struct BranchDeletionDisplay {
    result: BranchDeletionResult,
    show_unmerged_hint: bool,
}

fn print_retained_unmerged_branch(branch_name: &str) {
    eprintln!(
        "{}",
        info_message(cformat!(
            "Branch <bold>{branch_name}</> retained; has unmerged changes"
        ))
    );
    let cmd = suggest_command("remove", &[branch_name], &["-D"]);
    eprintln!(
        "{}",
        hint_message(cformat!(
            "To delete the unmerged branch, run <underline>{cmd}</>"
        ))
    );
}

/// Handle the result of a branch deletion attempt.
///
/// Converts a deletion attempt into structured display data:
/// - `NotDeleted`: We checked and chose not to delete (not integrated)
/// - `Err(e)`: Git command failed - show warning with actual error
fn handle_branch_deletion_result(
    result: anyhow::Result<BranchDeletionResult>,
    branch_name: &str,
) -> anyhow::Result<BranchDeletionDisplay> {
    match result {
        Ok(result) => Ok(BranchDeletionDisplay {
            show_unmerged_hint: matches!(result.outcome, BranchDeletionOutcome::NotDeleted),
            result,
        }),
        Err(e) => {
            // Git command failed - this is an error (we decided to delete but couldn't)
            eprintln!(
                "{}",
                error_message(cformat!("Failed to delete branch <bold>{branch_name}</>"))
            );
            eprintln!("{}", format_with_gutter(&e.to_string(), None));
            Err(e)
        }
    }
}

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

    fn after(&self, color: AnsiColor) -> String {
        match &self.symbol {
            Some(s) => match color {
                AnsiColor::Cyan => cformat!("{}<cyan>{}</>", s, self.suffix),
                AnsiColor::Green => cformat!("{}<green>{}</>", s, self.suffix),
                _ => format!("{s}{}", self.suffix),
            },
            None => String::new(),
        }
    }
}

/// Get flag acknowledgment note for remove messages
///
/// `target_branch`: The branch we checked integration against (shown in reason)
fn flag_note(
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

/// Show switch message when changing directory after worktree removal.
///
/// When shell integration is not active, warns that cd cannot happen.
/// This is important for remove/merge since the user would be left in a deleted directory.
///
/// # Warning Message Format
///
/// Uses the standard "Cannot change directory — {reason}" pattern.
/// See [`compute_shell_warning_reason`] for the full list of reasons.
fn print_switch_message_if_changed(
    changed_directory: bool,
    main_path: &Path,
) -> anyhow::Result<()> {
    if !changed_directory {
        return Ok(());
    }

    // Use main_path for discovery - the worktree we came from may have been removed
    let Ok(repo) = Repository::at(main_path) else {
        return Ok(());
    };
    let Ok(Some(dest_branch)) = repo.worktree_at(main_path).branch() else {
        return Ok(());
    };

    let logical_path = super::to_logical_path(main_path);
    let path_display = format_path_for_display(&logical_path);

    if super::is_shell_integration_active() {
        // Shell integration active - cd will work
        eprintln!(
            "{}",
            info_message(cformat!(
                "Switched to worktree for <bold>{dest_branch}</> @ <bold>{path_display}</>"
            ))
        );
    } else if crate::is_git_subcommand() {
        // Running as `git wt` - explain why cd can't work
        eprintln!(
            "{}",
            warning_message(
                "Cannot change directory — ran git wt; running through git prevents cd",
            )
        );
        eprintln!("{}", hint_message(git_subcommand_warning()));
    } else {
        // Shell integration not active - compute specific reason
        let reason = compute_shell_warning_reason();
        eprintln!(
            "{}",
            warning_message(cformat!("Cannot change directory — {reason}"))
        );
        // Show appropriate hint based on invocation mode
        if should_show_explicit_path_hint() {
            eprintln!("{}", hint_message(explicit_path_hint(&dest_branch)));
        } else {
            eprintln!("{}", hint_message(shell_integration_hint()));
        }
    }
    Ok(())
}

/// Compute the target directory for `cd` after switching, preserving the user's
/// subdirectory position when possible.
///
/// If the user is in `source_root/apps/gateway/` and `target_root/apps/gateway/`
/// exists, returns `target_root/apps/gateway/`. Otherwise returns `target_root`.
fn resolve_subdir_in_target(target_root: &Path, source_root: Option<&Path>, cwd: &Path) -> PathBuf {
    if let Some(source_root) = source_root {
        // Canonicalize both paths to handle symlinks (e.g., /var -> /private/var on macOS)
        let cwd = dunce::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
        let source_root =
            dunce::canonicalize(source_root).unwrap_or_else(|_| source_root.to_path_buf());
        if let Ok(relative) = cwd.strip_prefix(&source_root)
            && !relative.as_os_str().is_empty()
        {
            let candidate = target_root.join(relative);
            if candidate.is_dir() {
                return candidate;
            }
        }
    }
    target_root.to_path_buf()
}

/// Handle output for a switch operation
///
/// # Shell Integration Warnings
///
/// Always warn when the shell's directory won't change. Users expect to be in
/// the target worktree after switching.
///
/// **When to warn:** Shell integration is not active (no directive files set).
/// This applies to both `Existing` and `Created` results.
///
/// **When NOT to warn:**
/// - `AlreadyAt` — user is already in the target directory
/// - Shell integration IS active — cd will happen automatically
///
/// **Warning format:** `Cannot change directory — {reason}`
///
/// See [`compute_shell_warning_reason`] for the full list of reasons.
///
/// **Message order for Created:** Success message first, then warning. Creation
/// is a real accomplishment, but users still need to know they won't cd there.
///
/// # Arguments
///
/// * `change_dir` — When false, skip the directory change (user requested `--no-cd`)
///
/// # Return Value
///
/// Returns `Some(path)` when post-switch hooks should show "@ path" in their
/// announcements (because the user's shell won't be in that directory). This happens when:
/// - Shell integration is not active (user's shell stays in original directory)
/// - `change_dir` is false (user explicitly requested no directory change)
///
/// Returns `None` when the user will be in the worktree directory (shell integration
/// active or already at the worktree), so no path annotation needed.
pub fn handle_switch_output(
    result: &SwitchResult,
    branch_info: &SwitchBranchInfo,
    change_dir: bool,
    source_worktree_root: Option<&Path>,
    cwd: &Path,
) -> anyhow::Result<Option<std::path::PathBuf>> {
    // Set target directory for command execution, preserving subdirectory position.
    // If the user is in apps/gateway/ in the source worktree and that directory exists
    // in the target, cd to apps/gateway/ in the target instead of the root.
    if change_dir {
        let cd_target = resolve_subdir_in_target(result.path(), source_worktree_root, cwd);
        super::change_directory(&cd_target)?;
    }

    // Translate to the user's logical (symlink-preserved) path for display messages.
    // The cd directive (above) handles its own translation internally.
    let ctx = build_switch_output_context(result, branch_info, change_dir);

    let display_path_for_hooks = match result {
        SwitchResult::AlreadyAt(_) => handle_switch_already_at_output(&ctx),
        SwitchResult::Existing { .. } => handle_switch_existing_output(&ctx),
        SwitchResult::Created {
            created_branch,
            base_branch,
            from_remote,
            ..
        } => handle_switch_created_output(
            &ctx,
            *created_branch,
            base_branch.as_deref(),
            from_remote.as_deref(),
        ),
    };

    stderr().flush()?;
    Ok(display_path_for_hooks)
}

/// Execute the --execute command after hooks have run.
///
/// `display_path` is shown when the user's shell won't be in the worktree
/// directory (shell integration not active). This helps users understand where
/// the command runs.
///
/// When the conservative EXEC scrub is in effect (nested `wt` inside an alias
/// or hook body), no `Executing` header is printed — `execute()` emits its own
/// warning explaining the skip, and a contradictory header would read as a
/// broken promise. See `output::global::warn_exec_scrubbed_once`.
pub fn execute_user_command(command: &str, display_path: Option<&Path>) -> anyhow::Result<()> {
    if super::exec_would_be_refused() {
        // execute() will emit the conservative-scrub warning and return Ok.
        return super::execute(command);
    }

    // Show what command is being executed (section header + gutter content)
    // Include path when user's shell won't be there (shell integration not active)
    let header = match display_path {
        Some(path) => {
            let path_display = format_path_for_display(path);
            cformat!("Executing (--execute) @ <bold>{path_display}</>:")
        }
        None => "Executing (--execute):".to_string(),
    };
    eprintln!("{}", progress_message(header));
    eprintln!("{}", format_bash_with_gutter(command));

    super::execute(command)?;

    Ok(())
}

/// Handle output for a remove operation
///
/// Approval is handled at the gate (command entry point), not here.
/// When `quiet` is true (prune context), suppresses informational messages
/// like "No worktree found for branch X" that are noise in batch operations.
pub fn handle_remove_output(
    result: &RemoveResult,
    foreground: bool,
    verify: bool,
    quiet: bool,
    show_branch_in_hooks: bool,
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
            expected_path,
            removed_commit,
        } => handle_removed_worktree_output(RemovedWorktreeOutputContext {
            main_path,
            worktree_path,
            changed_directory: *changed_directory,
            branch_name: branch_name.as_deref(),
            deletion_mode: *deletion_mode,
            target_branch: target_branch.as_deref(),
            pre_computed_integration: *integration_reason,
            force_worktree: *force_worktree,
            expected_path: expected_path.as_deref(),
            removed_commit: removed_commit.as_deref(),
            foreground,
            verify,
            show_branch_in_hooks,
        }),
        RemoveResult::BranchOnly {
            branch_name,
            deletion_mode,
            pruned,
            target_branch,
            integration_reason,
        } => handle_branch_only_output(
            branch_name,
            *deletion_mode,
            *pruned,
            *integration_reason,
            target_branch.as_deref(),
            quiet,
        ),
    }
}

/// Handle output for BranchOnly removal (branch exists but no worktree)
///
/// When `quiet` is true, suppresses the "No worktree found for branch X"
/// info line for non-pruned cases (noise in prune/batch context).
fn handle_branch_only_output(
    branch_name: &str,
    deletion_mode: BranchDeletionMode,
    pruned: bool,
    integration_reason: Option<IntegrationReason>,
    target_branch: Option<&str>,
    quiet: bool,
) -> anyhow::Result<()> {
    let branch_info = if pruned {
        cformat!("Worktree directory missing for <bold>{branch_name}</>; pruned")
    } else {
        cformat!("No worktree found for branch <bold>{branch_name}</>")
    };

    // If we won't delete the branch, show info and return early
    if deletion_mode.should_keep() {
        eprintln!("{}", info_message(&branch_info));
        stderr().flush()?;
        return Ok(());
    }

    let check_target = target_branch.unwrap_or("HEAD");

    // Decide outcome from pre-computed integration (computed in prepare_worktree_removal).
    let outcome = if deletion_mode.is_force() {
        Some(BranchDeletionOutcome::ForceDeleted)
    } else {
        integration_reason.map(BranchDeletionOutcome::Integrated)
    };

    let deletion = if let Some(outcome) = outcome {
        let repo = worktrunk::git::Repository::current()?;
        let result = repo.run_command(&["branch", "-D", branch_name]);
        handle_branch_deletion_result(
            result.map(|_| BranchDeletionResult {
                outcome,
                integration_target: check_target.to_string(),
            }),
            branch_name,
        )?
    } else {
        BranchDeletionDisplay {
            result: BranchDeletionResult {
                outcome: BranchDeletionOutcome::NotDeleted,
                integration_target: check_target.to_string(),
            },
            show_unmerged_hint: true,
        }
    };

    if matches!(deletion.result.outcome, BranchDeletionOutcome::NotDeleted) {
        eprintln!("{}", info_message(&branch_info));
        if deletion.show_unmerged_hint {
            print_retained_unmerged_branch(branch_name);
        }
    } else {
        let flag_note = flag_note(
            deletion_mode,
            &deletion.result.outcome,
            Some(&deletion.result.integration_target),
        );
        let flag_text = &flag_note.text;
        let flag_after = flag_note.after(AnsiColor::Green);

        if pruned {
            // Combined: pruned stale metadata & deleted branch in one line
            eprintln!(
                "{}",
                FormattedMessage::new(cformat!(
                    "<green>✓ Pruned stale worktree & removed branch <bold>{branch_name}</>{flag_text}</>{flag_after}"
                ))
            );
        } else {
            if !quiet {
                eprintln!("{}", info_message(&branch_info));
            }
            eprintln!(
                "{}",
                FormattedMessage::new(cformat!(
                    "<green>✓ Removed branch <bold>{branch_name}</>{flag_text}</>{flag_after}"
                ))
            );
        }
    }

    stderr().flush()?;
    Ok(())
}

/// Spawn post-remove and post-switch hooks as a single batch after worktree removal.
///
/// Combines both hook types into one output line for consistency with how
/// post-switch and post-start are combined during worktree creation.
///
/// Post-remove template variables reflect the removed worktree (branch, path, commit).
/// Post-switch hooks only run when `changed_directory` is true (user cd'd to main).
///
/// Only runs if `ctx.verify` is true (hooks approved).
fn spawn_hooks_after_remove(
    repo: &Repository,
    ctx: &RemovedWorktreeOutputContext<'_>,
    removed_branch: &str,
) -> anyhow::Result<()> {
    if !ctx.verify {
        return Ok(());
    }
    let Ok(config) = UserConfig::load() else {
        return Ok(());
    };

    // When removing the current worktree, user cd's to main_path → use post_hook logic
    // (suppresses path if shell integration will cd there).
    // When removing a different worktree, user stays at cwd → use pre_hook logic
    // (shows path if main_path differs from cwd).
    let display_path = if ctx.changed_directory {
        super::post_hook_display_path(ctx.main_path)
    } else {
        super::pre_hook_display_path(ctx.main_path)
    };

    // Build post-remove template variables from the removed worktree identity.
    let remove_vars =
        PostRemoveContext::new(ctx.worktree_path, ctx.removed_commit, ctx.main_path, repo);
    let extra_vars = remove_vars.extra_vars(removed_branch);

    // All hooks use remove_ctx for spawning: log files are named after the removed
    // branch since both post-remove and post-switch are consequences of that removal.
    let remove_ctx = CommandContext::new(repo, &config, Some(removed_branch), ctx.main_path, false);

    // Collect post-remove and post-switch hooks for a single combined announcement.
    let mut pipelines = Vec::new();
    pipelines.extend(
        prepare_background_hooks(
            &remove_ctx,
            worktrunk::HookType::PostRemove,
            &extra_vars,
            display_path,
        )?
        .into_iter()
        .map(|g| (remove_ctx, g)),
    );

    // Post-switch: only when the user actually changed directory.
    // Uses its own context with the destination branch for template variables.
    // dest_branch hoisted so it outlives the pipelines vec.
    let dest_branch = if ctx.changed_directory {
        Some(repo.worktree_at(ctx.main_path).branch()?)
    } else {
        None
    };
    if let Some(ref dest_branch) = dest_branch {
        let switch_ctx =
            CommandContext::new(repo, &config, dest_branch.as_deref(), ctx.main_path, false);
        pipelines.extend(
            prepare_background_hooks(
                &switch_ctx,
                worktrunk::HookType::PostSwitch,
                &[],
                display_path,
            )?
            .into_iter()
            .map(|g| (switch_ctx, g)),
        );
    }

    announce_and_spawn_background_hooks(pipelines, ctx.show_branch_in_hooks)?;

    Ok(())
}

// ============================================================================
// Removal Display Info: Shared data for background/foreground output
// ============================================================================

/// Information needed to display removal messages and hints.
///
/// This struct captures the outcome of a branch deletion decision (whether
/// pre-computed for background mode or actual for foreground mode) so that
/// message formatting can be shared between both modes.
struct RemovalDisplayInfo {
    /// The deletion outcome (NotDeleted, ForceDeleted, or Integrated)
    outcome: BranchDeletionOutcome,
    /// The target branch used for integration check (may be upstream if ahead of local)
    integration_target: Option<String>,
    /// Whether the branch was integrated (used for hints when branch is kept)
    branch_was_integrated: bool,
    /// Whether to show the "unmerged, run -D" hint (foreground only, based on actual deletion)
    show_unmerged_hint: bool,
    /// Whether --force was used (for display purposes)
    force_worktree: bool,
}

impl RemovalDisplayInfo {
    /// Build display info from pre-computed integration (background mode).
    ///
    /// Uses the pre-computed integration reason to avoid race conditions when
    /// removing multiple worktrees (background processes can hold git locks).
    fn from_precomputed(
        deletion_mode: BranchDeletionMode,
        pre_computed_integration: Option<IntegrationReason>,
        target_branch: Option<&str>,
        force_worktree: bool,
    ) -> Self {
        let (outcome, integration_target) = if deletion_mode.should_keep() {
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
            let outcome = match pre_computed_integration {
                Some(r) => BranchDeletionOutcome::Integrated(r),
                None => BranchDeletionOutcome::NotDeleted,
            };
            (outcome, target_branch.map(String::from))
        };

        Self {
            outcome,
            integration_target,
            branch_was_integrated: pre_computed_integration.is_some(),
            show_unmerged_hint: false, // Background mode never shows this hint
            force_worktree,
        }
    }

    /// Build display info from actual deletion result (foreground mode).
    fn from_branch_result(
        branch_deletion: Option<anyhow::Result<BranchDeletionResult>>,
        branch_name: &str,
        pre_computed_integration: Option<IntegrationReason>,
        target_branch: Option<&str>,
        force_worktree: bool,
    ) -> anyhow::Result<Self> {
        let branch_was_integrated = pre_computed_integration.is_some();

        let (outcome, integration_target, show_unmerged_hint) = match branch_deletion {
            Some(result) => {
                let deletion = handle_branch_deletion_result(result, branch_name)?;
                // Only use integration_target for display if we had a real target (not "HEAD" fallback)
                let display_target =
                    target_branch.map(|_| deletion.result.integration_target.clone());
                (
                    deletion.result.outcome,
                    display_target,
                    deletion.show_unmerged_hint,
                )
            }
            None => (
                BranchDeletionOutcome::NotDeleted,
                target_branch.map(String::from),
                false,
            ),
        };

        Ok(Self {
            outcome,
            integration_target,
            branch_was_integrated,
            show_unmerged_hint,
            force_worktree,
        })
    }

    /// Whether the branch will be/was deleted.
    fn branch_deleted(&self) -> bool {
        matches!(
            self.outcome,
            BranchDeletionOutcome::ForceDeleted | BranchDeletionOutcome::Integrated(_)
        )
    }

    /// Print the removal message (progress for background, success for foreground).
    fn print_message(&self, branch_name: &str, foreground: bool) -> anyhow::Result<()> {
        let flag_note = flag_note(
            if self.branch_deleted() {
                BranchDeletionMode::SafeDelete // Doesn't matter, outcome already determined
            } else {
                BranchDeletionMode::Keep
            },
            &self.outcome,
            self.integration_target.as_deref(),
        );
        let force_text = if self.force_worktree {
            " (--force)"
        } else {
            ""
        };

        let msg = if foreground {
            if self.branch_deleted() {
                let flag_text = &flag_note.text;
                success_message(cformat!(
                    "Removed <bold>{branch_name}</> worktree{force_text} & branch{flag_text}"
                ))
                .append(&flag_note.after(AnsiColor::Green))
            } else {
                success_message(cformat!(
                    "Removed <bold>{branch_name}</> worktree{force_text}"
                ))
            }
        } else if self.branch_deleted() {
            let flag_text = &flag_note.text;
            progress_message(cformat!(
                "Removing <bold>{branch_name}</> worktree{force_text} & branch in background{flag_text}"
            ))
            .append(&flag_note.after(AnsiColor::Cyan))
        } else {
            progress_message(cformat!(
                "Removing <bold>{branch_name}</> worktree{force_text} in background"
            ))
        };
        eprintln!("{msg}");
        Ok(())
    }

    /// Print hints about branch status (why it was kept, how to force delete).
    fn print_hints(
        &self,
        branch_name: &str,
        deletion_mode: BranchDeletionMode,
        pre_computed_integration: Option<IntegrationReason>,
    ) -> anyhow::Result<()> {
        if self.branch_deleted() {
            return Ok(());
        }

        if deletion_mode.should_keep() {
            if let Some(reason) = pre_computed_integration.as_ref() {
                // User kept an integrated branch - show integration info
                let target = self.integration_target.as_deref().unwrap_or("target");
                let desc = reason.description();
                let symbol = reason.symbol();
                eprintln!(
                    "{}",
                    hint_message(cformat!(
                        "Branch integrated ({desc} <underline>{target}</>, <dim>{symbol}</>); retained with <underline>--no-delete-branch</>"
                    ))
                );
            }
        } else if self.show_unmerged_hint
            || (!deletion_mode.should_keep() && !self.branch_was_integrated)
        {
            // Unmerged, no flag - show how to force delete
            // (Background: !should_keep && !integrated, Foreground: show_unmerged_hint)
            let cmd = suggest_command("remove", &[branch_name], &["-D"]);
            eprintln!(
                "{}",
                hint_message(cformat!(
                    "Branch unmerged; to delete, run <underline>{cmd}</>"
                ))
            );
        }
        // else: Unmerged + flag - no hint (flag had no effect)

        Ok(())
    }
}

// ============================================================================

struct RemovedWorktreeOutputContext<'a> {
    main_path: &'a Path,
    worktree_path: &'a Path,
    changed_directory: bool,
    branch_name: Option<&'a str>,
    deletion_mode: BranchDeletionMode,
    target_branch: Option<&'a str>,
    pre_computed_integration: Option<IntegrationReason>,
    force_worktree: bool,
    expected_path: Option<&'a Path>,
    removed_commit: Option<&'a str>,
    foreground: bool,
    verify: bool,
    /// Show branch name in hook announcements for disambiguation in batch contexts.
    show_branch_in_hooks: bool,
}

fn execute_pre_remove_hooks_if_needed(
    repo: &Repository,
    ctx: &RemovedWorktreeOutputContext<'_>,
) -> anyhow::Result<()> {
    if !ctx.verify {
        return Ok(());
    }

    let Ok(config) = UserConfig::load() else {
        return Ok(());
    };

    let command_ctx = CommandContext::new(
        repo,
        &config,
        ctx.branch_name,
        ctx.worktree_path,
        false, // yes=false for CommandContext (not approval-related)
    );
    let display_path = if ctx.changed_directory {
        None
    } else {
        Some(ctx.worktree_path)
    };
    let target_branch = repo
        .worktree_at(ctx.main_path)
        .branch()
        .ok()
        .flatten()
        .unwrap_or_default();
    let target_path_str = worktrunk::path::to_posix_path(&ctx.main_path.to_string_lossy());
    let extra_vars: Vec<(&str, &str)> = vec![
        ("target", &target_branch),
        ("target_worktree_path", &target_path_str),
    ];

    execute_hook(
        &command_ctx,
        worktrunk::HookType::PreRemove,
        &extra_vars,
        FailureStrategy::FailFast,
        &[],
        display_path,
    )
}

fn prepare_remove_directory_change(
    main_path: &Path,
    changed_directory: bool,
) -> anyhow::Result<()> {
    if changed_directory {
        super::change_directory(main_path)?;
        stderr().flush()?; // Force flush to ensure shell processes the cd
        // Mark that the CWD worktree is being removed, so the error handler
        // can show a hint if a subsequent command (e.g., post-merge hook) fails.
        super::mark_cwd_removed();
    }

    Ok(())
}

fn handle_detached_removed_worktree_output(
    repo: &Repository,
    ctx: &RemovedWorktreeOutputContext<'_>,
) -> anyhow::Result<()> {
    if ctx.foreground {
        eprintln!(
            "{}",
            progress_message(cformat!(
                "Removing worktree @ <bold>{}</>... (detached HEAD, no branch to delete)",
                format_path_for_display(ctx.worktree_path)
            ))
        );
        let output = remove_worktree_with_cleanup(
            repo,
            ctx.worktree_path,
            RemoveOptions {
                branch: None,
                deletion_mode: ctx.deletion_mode,
                target_branch: ctx.target_branch.map(String::from),
                force_worktree: ctx.force_worktree,
            },
        )
        .map_err(|err| GitError::WorktreeRemovalFailed {
            branch: path_dir_name(ctx.worktree_path).to_string(),
            path: ctx.worktree_path.to_path_buf(),
            remaining_entries: list_remaining_entries(ctx.worktree_path),
            error: err.to_string(),
        })?;
        if let Some(staged) = output.staged_path {
            let _ = std::fs::remove_dir_all(&staged);
        }
        eprintln!(
            "{}",
            success_message(cformat!(
                "Removed worktree @ <bold>{}</> (detached HEAD, no branch to delete)",
                format_path_for_display(ctx.worktree_path)
            ))
        );
    } else {
        let path_display = format_path_for_display(ctx.worktree_path);
        eprintln!(
            "{}",
            progress_message(cformat!(
                "Removing worktree @ <bold>{path_display}</> in background (detached HEAD, no branch to delete)"
            ))
        );

        spawn_background_removal(
            repo,
            ctx.main_path,
            ctx.worktree_path,
            None,
            ctx.force_worktree,
            "detached",
            ctx.changed_directory,
        )?;
    }

    // Post-remove hooks for detached HEAD use "HEAD" as the branch identifier
    spawn_hooks_after_remove(repo, ctx, "HEAD")?;
    stderr().flush()?;
    Ok(())
}

fn handle_named_removed_worktree_foreground(
    repo: &Repository,
    ctx: &RemovedWorktreeOutputContext<'_>,
    branch_name: &str,
) -> anyhow::Result<()> {
    eprintln!(
        "{}",
        progress_message(cformat!("Removing <bold>{branch_name}</> worktree..."))
    );

    if let Some(expected) = ctx.expected_path {
        eprintln!(
            "{}",
            format_path_mismatch_warning(branch_name, ctx.worktree_path, expected)
        );
    }

    let output = remove_worktree_with_cleanup(
        repo,
        ctx.worktree_path,
        RemoveOptions {
            branch: Some(branch_name.to_string()),
            deletion_mode: ctx.deletion_mode,
            target_branch: ctx.target_branch.map(String::from),
            force_worktree: ctx.force_worktree,
        },
    )
    .map_err(|err| GitError::WorktreeRemovalFailed {
        branch: branch_name.into(),
        path: ctx.worktree_path.to_path_buf(),
        remaining_entries: list_remaining_entries(ctx.worktree_path),
        error: err.to_string(),
    })?;
    if let Some(staged) = output.staged_path {
        let _ = std::fs::remove_dir_all(&staged);
    }

    let display_info = RemovalDisplayInfo::from_branch_result(
        output.branch_result,
        branch_name,
        ctx.pre_computed_integration,
        ctx.target_branch,
        ctx.force_worktree,
    )?;

    display_info.print_message(branch_name, true)?;
    display_info.print_hints(branch_name, ctx.deletion_mode, ctx.pre_computed_integration)?;
    print_switch_message_if_changed(ctx.changed_directory, ctx.main_path)?;

    spawn_hooks_after_remove(repo, ctx, branch_name)?;
    stderr().flush()?;
    Ok(())
}

fn handle_named_removed_worktree_background(
    repo: &Repository,
    ctx: &RemovedWorktreeOutputContext<'_>,
    branch_name: &str,
) -> anyhow::Result<()> {
    if let Some(expected) = ctx.expected_path {
        eprintln!(
            "{}",
            format_path_mismatch_warning(branch_name, ctx.worktree_path, expected)
        );
    }

    let display_info = RemovalDisplayInfo::from_precomputed(
        ctx.deletion_mode,
        ctx.pre_computed_integration,
        ctx.target_branch,
        ctx.force_worktree,
    );

    display_info.print_message(branch_name, false)?;
    display_info.print_hints(branch_name, ctx.deletion_mode, ctx.pre_computed_integration)?;
    print_switch_message_if_changed(ctx.changed_directory, ctx.main_path)?;

    spawn_background_removal(
        repo,
        ctx.main_path,
        ctx.worktree_path,
        display_info.branch_deleted().then_some(branch_name),
        ctx.force_worktree,
        branch_name,
        ctx.changed_directory,
    )?;

    spawn_hooks_after_remove(repo, ctx, branch_name)?;
    stderr().flush()?;
    Ok(())
}

/// Handle output for RemovedWorktree removal
fn handle_removed_worktree_output(ctx: RemovedWorktreeOutputContext<'_>) -> anyhow::Result<()> {
    // Use main_path for discovery - the worktree being removed might be cwd,
    // and git operations after removal need a valid working directory.
    let repo = worktrunk::git::Repository::at(ctx.main_path)?;

    execute_pre_remove_hooks_if_needed(&repo, &ctx)?;
    prepare_remove_directory_change(ctx.main_path, ctx.changed_directory)?;

    // Handle detached HEAD case (no branch known)
    let Some(branch_name) = ctx.branch_name else {
        return handle_detached_removed_worktree_output(&repo, &ctx);
    };

    if ctx.foreground {
        handle_named_removed_worktree_foreground(&repo, &ctx, branch_name)
    } else {
        handle_named_removed_worktree_background(&repo, &ctx, branch_name)
    }
}

/// Run a shell command with streaming output, signal forwarding, and ANSI reset.
///
/// Unified entry point for all foreground command execution — hooks, aliases,
/// and `for-each` all call this. The background pipeline runner
/// (`run_pipeline.rs`) has its own spawning logic since it redirects to log
/// files and runs detached.
///
/// Capabilities: stdout→stderr redirect for deterministic ordering,
/// SIGINT/SIGTERM forwarding to child process group, ANSI reset before child
/// runs, `Cmd` tracing/logging, and directive file control.
///
/// ## Directive files
///
/// `directives` controls whether the child can write shell-integration
/// directives back to the parent shell. The CD file is always safe to pass
/// through (raw path, no injection surface); the EXEC file is never passed
/// through — only wt-internal Rust code writes arbitrary shell directives.
///
/// - `DirectivePassthrough::none()` — scrubs all directive env vars from the
///   child. Used by `for-each` (runs in other worktrees) and background hooks
///   (outlive the parent shell).
/// - `DirectivePassthrough::inherit_from_env()` — re-adds whichever directive
///   env vars are currently set in this process. Used by aliases and
///   foreground hooks, which may emit `cd` directives. In new-protocol mode
///   only the CD file passes through; in legacy compat mode the single
///   legacy file passes through to preserve pre-split behavior.
///
/// ## ANSI reset
///
/// Resets ANSI codes on stderr before the child runs. Terminal emulators
/// maintain a single rendering state machine — if stdout writes color codes
/// but stderr's output arrives next, the terminal applies stdout's color
/// state to stderr's text. The reset to stderr prevents this.
pub fn execute_shell_command(
    working_dir: &std::path::Path,
    command: &str,
    stdin_content: Option<&str>,
    command_log_label: Option<&str>,
    directives: DirectivePassthrough,
) -> anyhow::Result<()> {
    // Flush stdout before executing command to ensure all our messages appear
    // before the child process output
    stderr().flush()?;

    // Reset ANSI codes on stderr to prevent color bleeding (see function docs for details)
    // This fixes color bleeding observed when worktrunk prints colored output to stdout
    // followed immediately by child process output to stderr (e.g., pre-commit run output).
    eprint!("{}", anstyle::Reset);
    stderr().flush().ok(); // Ignore flush errors - reset is best-effort, command execution should proceed

    // Execute with stdout→stderr redirect for deterministic ordering
    let mut cmd = Cmd::shell(command)
        .current_dir(working_dir)
        .stdout(Stdio::from(std::io::stderr()))
        .forward_signals();

    if let Some(label) = command_log_label {
        cmd = cmd.external(label);
    }

    if let Some(content) = stdin_content {
        cmd = cmd.stdin_bytes(content);
    }

    if let Some(path) = directives.cd_file {
        cmd = cmd.directive_cd_file(path);
    }
    if let Some(path) = directives.legacy_file {
        cmd = cmd.directive_legacy_file(path);
    }

    cmd.stream()?;

    // Flush to ensure all output appears before we continue
    stderr().flush()?;

    Ok(())
}

/// Selector for which directive file env vars to pass through to a child shell.
///
/// Constructed by callers via [`DirectivePassthrough::none`] or
/// [`DirectivePassthrough::inherit_from_env`]. The EXEC file is intentionally
/// never included — alias/hook shell bodies must not inject arbitrary shell
/// into the parent session.
#[derive(Debug, Default, Clone)]
pub struct DirectivePassthrough {
    pub cd_file: Option<std::path::PathBuf>,
    pub legacy_file: Option<std::path::PathBuf>,
}

impl DirectivePassthrough {
    /// Scrub all directive file env vars from the child process.
    pub fn none() -> Self {
        Self::default()
    }

    /// Pass CD and legacy directive files through to the child, reading the
    /// current process environment. Used by trusted contexts (aliases,
    /// foreground hooks) that may legitimately emit a `cd` directive. The
    /// EXEC file is deliberately omitted.
    pub fn inherit_from_env() -> Self {
        use worktrunk::shell_exec::{DIRECTIVE_CD_FILE_ENV_VAR, DIRECTIVE_FILE_ENV_VAR};
        let read = |var: &str| {
            std::env::var_os(var)
                .map(std::path::PathBuf::from)
                .filter(|p| !p.as_os_str().is_empty())
        };
        Self {
            cd_file: read(DIRECTIVE_CD_FILE_ENV_VAR),
            legacy_file: read(DIRECTIVE_FILE_ENV_VAR),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;

    #[test]
    fn test_format_switch_message() {
        let path = PathBuf::from("/tmp/test");

        // Switched to existing worktree (no creation)
        let msg = format_switch_message("feature", &path, false, false, None, None);
        assert_snapshot!(msg, @"Switched to worktree for [1mfeature[22m @ [1m/tmp/test[22m");

        // Created branch and worktree with --create
        let msg = format_switch_message("feature", &path, true, true, Some("main"), None);
        assert_snapshot!(msg, @"Created branch [1mfeature[22m from [1mmain[22m and worktree @ [1m/tmp/test[22m");

        // Created worktree from remote (DWIM) - also creates local tracking branch
        let msg =
            format_switch_message("feature", &path, true, false, None, Some("origin/feature"));
        assert_snapshot!(msg, @"Created branch [1mfeature[22m (tracking [1morigin/feature[22m) and worktree @ [1m/tmp/test[22m");

        // Created worktree only (local branch already existed)
        let msg = format_switch_message("feature", &path, true, false, None, None);
        assert!(!msg.contains("branch")); // Should NOT mention branch creation
        assert_snapshot!(msg, @"Created worktree for [1mfeature[22m @ [1m/tmp/test[22m");
    }

    #[test]
    fn test_flag_note() {
        // --no-delete-branch flag (text only, no symbol, no suffix)
        let note = flag_note(
            BranchDeletionMode::Keep,
            &BranchDeletionOutcome::NotDeleted,
            None,
        );
        assert_eq!(note.text, " (--no-delete-branch)");
        assert!(note.symbol.is_none());
        assert!(note.suffix.is_empty());

        // NotDeleted without flag (empty)
        let note = flag_note(
            BranchDeletionMode::SafeDelete,
            &BranchDeletionOutcome::NotDeleted,
            None,
        );
        assert!(note.text.is_empty());
        assert!(note.symbol.is_none());
        assert!(note.suffix.is_empty());

        // Force deleted (text only, no symbol, no suffix)
        let note = flag_note(
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
            let note = flag_note(
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
    fn test_resolve_subdir_in_target_no_source_root() {
        let target = PathBuf::from("/target/worktree");
        let cwd = PathBuf::from("/some/dir");
        assert_eq!(resolve_subdir_in_target(&target, None, &cwd), target);
    }

    #[test]
    fn test_resolve_subdir_in_target_subdir_exists() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        std::fs::create_dir_all(source.join("apps/gateway")).unwrap();
        std::fs::create_dir_all(target.join("apps/gateway")).unwrap();

        let cwd = source.join("apps/gateway");
        let result = resolve_subdir_in_target(&target, Some(&source), &cwd);
        assert_eq!(result, target.join("apps/gateway"));
    }

    #[test]
    fn test_resolve_subdir_in_target_subdir_missing() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        std::fs::create_dir_all(source.join("apps/gateway")).unwrap();
        std::fs::create_dir_all(&target).unwrap();

        let cwd = source.join("apps/gateway");
        let result = resolve_subdir_in_target(&target, Some(&source), &cwd);
        assert_eq!(result, target); // Falls back to root
    }

    #[test]
    fn test_resolve_subdir_in_target_at_root() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&target).unwrap();

        let result = resolve_subdir_in_target(&target, Some(&source), &source);
        assert_eq!(result, target);
    }

    #[test]
    fn test_shell_integration_hint() {
        let hint = shell_integration_hint();
        assert_snapshot!(hint, @"To enable automatic cd, run [4mwt config shell install[24m");
    }

    #[test]
    fn test_git_subcommand_warning() {
        let warning = git_subcommand_warning();
        assert_snapshot!(warning, @"For automatic cd, invoke directly (with the [4m-[24m): [4mgit-wt[24m");
    }
}
