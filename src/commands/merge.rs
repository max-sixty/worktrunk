use worktrunk::HookType;
use worktrunk::config::{ProjectConfig, WorktrunkConfig};
use worktrunk::git::{GitError, GitResultExt, Repository};
use worktrunk::styling::{
    AnstyleStyle, CYAN, CYAN_BOLD, ERROR, ERROR_EMOJI, HINT, HINT_EMOJI, WARNING, WARNING_EMOJI,
    format_bash_with_gutter, format_with_gutter,
};

use super::command_executor::{CommandContext, prepare_project_commands};
use super::worktree::{handle_push, parse_diff_shortstat};
use crate::output::execute_command_in_worktree;

/// Extract untracked files from git status --porcelain output
fn get_untracked_files(status_output: &str) -> Vec<String> {
    let mut untracked = Vec::new();

    for line in status_output.lines() {
        // Git status --porcelain format: XY filename
        // Untracked files have "??" status
        if let Some(filename) = line.strip_prefix("?? ") {
            untracked.push(filename.to_string());
        }
    }

    untracked
}

/// Warn about untracked files being auto-staged
fn show_untracked_warning(repo: &Repository) -> Result<(), GitError> {
    let status = repo
        .run_command(&["status", "--porcelain"])
        .git_context("Failed to get status")?;
    let untracked = get_untracked_files(&status);

    if untracked.is_empty() {
        return Ok(());
    }

    // Format file list (comma-separated)
    let file_list = untracked.join(", ");

    crate::output::progress(format!(
        "{WARNING_EMOJI} {WARNING}Auto-staging untracked files: {file_list}{WARNING:#}"
    ))?;

    Ok(())
}

pub fn handle_merge(
    target: Option<&str>,
    squash_enabled: bool,
    no_commit: bool,
    no_remove: bool,
    no_verify: bool,
    force: bool,
    tracked_only: bool,
) -> Result<(), GitError> {
    let repo = Repository::current();

    // Get current branch
    let current_branch = repo.current_branch()?.ok_or(GitError::DetachedHead)?;

    // Validate --no-commit: requires clean working tree
    if no_commit && repo.is_dirty()? {
        return Err(GitError::UncommittedChanges);
    }

    // Validate --no-commit flag compatibility
    if no_commit && !no_remove {
        return Err(GitError::CommandFailed(format!(
            "{ERROR_EMOJI} {ERROR}--no-commit requires --no-remove{ERROR:#}\n\n{HINT_EMOJI} {HINT}Cannot remove active worktree when skipping commit/rebase{HINT:#}"
        )));
    }

    // --no-commit implies --no-squash (validation above ensures --no-remove is already set)
    let squash_enabled = if no_commit { false } else { squash_enabled };

    // Get target branch (default to default branch if not provided)
    let target_branch = repo.resolve_target_branch(target)?;

    // Check if already on target branch
    if current_branch == target_branch {
        use worktrunk::styling::{GREEN, SUCCESS_EMOJI};
        let green_bold = GREEN.bold();
        crate::output::success(format!(
            "{SUCCESS_EMOJI} {GREEN}Already on {green_bold}{target_branch}{green_bold:#}, nothing to merge{GREEN:#}"
        ))?;
        return Ok(());
    }

    // Load config for LLM integration
    let config = WorktrunkConfig::load().git_context("Failed to load config")?;

    // Handle uncommitted changes (skip if --no-commit) - track whether commit occurred
    let committed = if !no_commit && repo.is_dirty()? {
        if squash_enabled {
            // Warn about untracked files before staging
            if !tracked_only {
                show_untracked_warning(&repo)?;
            }

            if tracked_only {
                repo.run_command(&["add", "-u"])
                    .git_context("Failed to stage tracked changes")?;
            } else {
                repo.run_command(&["add", "-A"])
                    .git_context("Failed to stage changes")?;
            }
            false // Staged but didn't commit (will squash later)
        } else {
            // Commit immediately when not squashing
            handle_commit_changes(
                &config.commit_generation,
                &current_branch,
                Some(&target_branch),
                no_verify,
                force,
                tracked_only,
            )?;
            true // Committed directly
        }
    } else {
        false // No dirty changes or --no-commit
    };

    // Squash commits if enabled - track whether squashing occurred
    let squashed = if squash_enabled {
        handle_squash(&target_branch, no_verify, force)?
    } else {
        false
    };

    // Rebase onto target (skip if --no-commit) - track whether rebasing occurred
    let rebased = if !no_commit {
        super::dev::handle_dev_rebase(Some(&target_branch))?
    } else {
        false
    };

    // Run pre-merge checks unless --no-verify was specified
    // Do this after commit/squash/rebase to validate the final state that will be pushed
    if !no_verify && let Ok(Some(project_config)) = ProjectConfig::load(&repo.worktree_root()?) {
        let worktree_path =
            std::env::current_dir().git_context("Failed to get current directory")?;
        run_pre_merge_commands(
            &project_config,
            &current_branch,
            &target_branch,
            &worktree_path,
            &repo,
            &config,
            force,
        )?;
    }

    // Fast-forward push to target branch with commit/squash/rebase info for consolidated message
    handle_push(
        Some(&target_branch),
        false,
        "Merged to",
        committed,
        squashed,
        rebased,
    )?;

    // Get primary worktree path before cleanup (while we can still run git commands)
    let primary_worktree_dir = repo.main_worktree_root()?;

    // Finish worktree unless --no-remove was specified
    if !no_remove {
        // STEP 1: Check for uncommitted changes before attempting cleanup
        // This prevents showing "Cleaning up worktree..." before failing
        repo.ensure_clean_working_tree()?;

        // STEP 2: Emit CD directive (just sets intent, doesn't actually change CWD)
        crate::output::change_directory(&primary_worktree_dir)?;

        // STEP 3: Switch to target branch in primary worktree (fails safely if there's an issue)
        let primary_repo = Repository::at(&primary_worktree_dir);
        let new_branch = primary_repo.current_branch()?;
        if new_branch.as_deref() != Some(&target_branch) {
            crate::output::progress(format!(
                "🔄 {CYAN}Switching to {CYAN_BOLD}{target_branch}{CYAN_BOLD:#}{CYAN}...{CYAN:#}"
            ))?;
            primary_repo
                .run_command(&["switch", &target_branch])
                .git_context(&format!("Failed to switch to '{}'", target_branch))?;
        }

        // STEP 4: Only NOW remove the worktree (after all checks passed)
        crate::output::progress(format!("🔄 {CYAN}Cleaning up worktree...{CYAN:#}"))?;
        let worktree_root = repo.worktree_root()?;
        repo.remove_worktree(&worktree_root)
            .git_context("Failed to remove worktree")?;

        // Print comprehensive summary
        handle_merge_summary_output(Some(&primary_worktree_dir))?;
    } else {
        // Print comprehensive summary (worktree preserved)
        handle_merge_summary_output(None)?;
    }

    // Execute post-merge commands in the main worktree
    // This runs after cleanup so the context is clear to the user
    // Create a fresh Repository instance at the primary worktree (the old repo may be invalid)
    let primary_repo = Repository::at(&primary_worktree_dir);
    execute_post_merge_commands(
        &primary_worktree_dir,
        &primary_repo,
        &config,
        &current_branch,
        &target_branch,
        force,
    )?;

    Ok(())
}

/// Format the merge summary message (includes emoji and color for consistency)
fn format_merge_summary(primary_path: Option<&std::path::Path>) -> String {
    use worktrunk::styling::{GREEN, HINT, HINT_EMOJI, SUCCESS_EMOJI};
    let green_bold = GREEN.bold();

    // Show where we ended up
    if let Some(path) = primary_path {
        format!(
            "{SUCCESS_EMOJI} {GREEN}Returned to primary at {green_bold}{}{green_bold:#}{GREEN:#}",
            path.display()
        )
    } else {
        format!("{HINT_EMOJI} {HINT}Use 'wt remove' to remove worktree{HINT:#}")
    }
}

/// Handle output for merge summary using global output context
fn handle_merge_summary_output(primary_path: Option<&std::path::Path>) -> Result<(), GitError> {
    let message = format_merge_summary(primary_path);

    // Show success message (formatting added by OutputContext)
    crate::output::success(message)?;

    // Flush output
    crate::output::flush()?;

    Ok(())
}

/// Format a commit message with the first line in bold, ready for gutter display
pub fn format_commit_message_for_display(message: &str) -> String {
    let bold = AnstyleStyle::new().bold();
    let lines: Vec<&str> = message.lines().collect();

    if lines.is_empty() {
        return String::new();
    }

    // Format first line in bold
    let mut result = format!("{bold}{}{bold:#}", lines[0]);

    // Add remaining lines without bold
    if lines.len() > 1 {
        for line in &lines[1..] {
            result.push('\n');
            result.push_str(line);
        }
    }

    result
}

/// Show hint if no LLM command is configured
pub fn show_llm_config_hint_if_needed(
    commit_generation_config: &worktrunk::config::CommitGenerationConfig,
) -> Result<(), GitError> {
    if !commit_generation_config.is_configured() {
        crate::output::hint(format!(
            "{HINT_EMOJI} {HINT}Using fallback commit message. Run 'wt config help' to configure LLM-generated messages{HINT:#}"
        ))?;
    }
    Ok(())
}

/// Commit already-staged changes with LLM-generated or fallback message
pub fn commit_staged_changes(
    commit_generation_config: &worktrunk::config::CommitGenerationConfig,
    show_no_squash_note: bool,
) -> Result<(), GitError> {
    let repo = Repository::current();

    // Get diff stats for staged changes
    let diff_shortstat = repo
        .run_command(&["diff", "--staged", "--shortstat"])
        .unwrap_or_default();
    let stats = parse_diff_shortstat(&diff_shortstat);
    let stats_parts = stats.format_summary();

    // Format progress message based on whether we're using LLM or fallback
    let action = if commit_generation_config.is_configured() {
        "Generating commit message and committing..."
    } else {
        "Committing with default message..."
    };

    // Build the progress message with optional squash status
    let mut parts = vec![];
    if !stats_parts.is_empty() {
        parts.extend(stats_parts);
    }
    if show_no_squash_note {
        parts.push("no squashing needed".to_string());
    }

    let full_progress_msg = if parts.is_empty() {
        format!("🔄 {CYAN}{action}{CYAN:#}")
    } else {
        format!("🔄 {CYAN}{action}{CYAN:#} ({})", parts.join(", "))
    };

    crate::output::progress(full_progress_msg)?;

    show_llm_config_hint_if_needed(commit_generation_config)?;
    let commit_message = crate::llm::generate_commit_message(commit_generation_config)?;

    let formatted_message = format_commit_message_for_display(&commit_message);
    crate::output::progress(format_with_gutter(&formatted_message, "", None))?;

    repo.run_command(&["commit", "-m", &commit_message])
        .git_context("Failed to commit")?;

    // Get commit hash for display
    let commit_hash = repo
        .run_command(&["rev-parse", "--short", "HEAD"])?
        .trim()
        .to_string();

    use worktrunk::styling::{GREEN, SUCCESS_EMOJI};
    let green_dim = GREEN.dimmed();
    crate::output::success(format!(
        "{SUCCESS_EMOJI} {GREEN}Committed changes @ {green_dim}{commit_hash}{green_dim:#}{GREEN:#}"
    ))?;

    Ok(())
}

/// Commit uncommitted changes with LLM-generated message
fn handle_commit_changes(
    commit_generation_config: &worktrunk::config::CommitGenerationConfig,
    current_branch: &str,
    target_branch: Option<&str>,
    no_verify: bool,
    force: bool,
    tracked_only: bool,
) -> Result<(), GitError> {
    let repo = Repository::current();
    let config = WorktrunkConfig::load().git_context("Failed to load config")?;

    // Run pre-commit hook unless --no-verify was specified
    if !no_verify && let Ok(Some(project_config)) = ProjectConfig::load(&repo.worktree_root()?) {
        let worktree_path =
            std::env::current_dir().git_context("Failed to get current directory")?;
        run_pre_commit_commands(
            &project_config,
            current_branch,
            &worktree_path,
            &repo,
            &config,
            force,
            target_branch,
        )?;
    }

    // Warn about untracked files before staging (only if using git add -A)
    if !tracked_only {
        show_untracked_warning(&repo)?;
    }

    // Stage changes
    if tracked_only {
        repo.run_command(&["add", "-u"])
            .git_context("Failed to stage tracked changes")?;
    } else {
        repo.run_command(&["add", "-A"])
            .git_context("Failed to stage changes")?;
    }

    // Show "no squashing needed" since we're committing directly (not in squash mode)
    commit_staged_changes(commit_generation_config, true)
}

fn handle_squash(target_branch: &str, no_verify: bool, force: bool) -> Result<bool, GitError> {
    // Delegate to the atomic dev command
    super::dev::handle_dev_squash(Some(target_branch), force, no_verify)
}

/// Run pre-merge commands sequentially (blocking, fail-fast)
pub fn run_pre_merge_commands(
    project_config: &ProjectConfig,
    current_branch: &str,
    target_branch: &str,
    worktree_path: &std::path::Path,
    repo: &Repository,
    config: &WorktrunkConfig,
    force: bool,
) -> Result<(), GitError> {
    let Some(pre_merge_config) = &project_config.pre_merge_command else {
        return Ok(());
    };

    let repo_root = repo.main_worktree_root()?;
    let ctx = CommandContext::new(
        repo,
        config,
        current_branch,
        worktree_path,
        &repo_root,
        force,
    );
    let commands = prepare_project_commands(
        pre_merge_config,
        &ctx,
        false,
        &[("target", target_branch)],
        "Pre-merge commands",
        |_name, command| {
            let dim = AnstyleStyle::new().dimmed();
            crate::output::progress(format!("{dim}Skipping pre-merge command: {command}{dim:#}"))
                .ok();
        },
    )?;
    for prepared in commands {
        let label = crate::commands::format_command_label("pre-merge", prepared.name.as_deref());
        crate::output::progress(format!("🔄 {CYAN}{label}{CYAN:#}"))?;
        crate::output::progress(format_bash_with_gutter(&prepared.expanded, ""))?;

        if let Err(e) = execute_command_in_worktree(worktree_path, &prepared.expanded) {
            return Err(GitError::HookCommandFailed {
                hook_type: HookType::PreMerge,
                command_name: prepared.name.clone(),
                error: e.to_string(),
            });
        }

        // No need to flush here - the redirect in execute_command_in_worktree ensures ordering
    }

    Ok(())
}

/// Execute post-merge commands sequentially in the main worktree (blocking)
pub fn execute_post_merge_commands(
    main_worktree_path: &std::path::Path,
    repo: &Repository,
    config: &WorktrunkConfig,
    branch: &str,
    target_branch: &str,
    force: bool,
) -> Result<(), GitError> {
    use worktrunk::styling::WARNING;

    // Load project config from the main worktree path directly
    let project_config = match ProjectConfig::load(main_worktree_path)
        .git_context("Failed to load project config")?
    {
        Some(cfg) => cfg,
        None => return Ok(()),
    };

    let Some(post_merge_config) = &project_config.post_merge_command else {
        return Ok(());
    };

    let ctx = CommandContext::new(
        repo,
        config,
        branch,
        main_worktree_path,
        main_worktree_path,
        force,
    );
    let commands = prepare_project_commands(
        post_merge_config,
        &ctx,
        false,
        &[("target", target_branch)],
        "Post-merge commands",
        |_name, command| {
            let dim = AnstyleStyle::new().dimmed();
            crate::output::progress(format!("{dim}Skipping command: {command}{dim:#}")).ok();
        },
    )?;

    if commands.is_empty() {
        return Ok(());
    }

    // Execute each command sequentially in the main worktree
    for prepared in commands {
        let label = crate::commands::format_command_label("post-merge", prepared.name.as_deref());
        crate::output::progress(format!("🔄 {CYAN}{label}{CYAN:#}"))?;
        crate::output::progress(format_bash_with_gutter(&prepared.expanded, ""))?;

        if let Err(e) = execute_command_in_worktree(main_worktree_path, &prepared.expanded) {
            use worktrunk::styling::WARNING_EMOJI;
            let warning_bold = WARNING.bold();
            let message = match &prepared.name {
                Some(name) => format!(
                    "{WARNING_EMOJI} {WARNING}Command {warning_bold}{name}{warning_bold:#} failed: {e}{WARNING:#}"
                ),
                None => format!("{WARNING_EMOJI} {WARNING}Command failed: {e}{WARNING:#}"),
            };
            crate::output::progress(message)?;
            // Continue with other commands even if one fails
        }
    }

    crate::output::flush()?;

    Ok(())
}

/// Run pre-commit commands sequentially (blocking, fail-fast)
pub fn run_pre_commit_commands(
    project_config: &ProjectConfig,
    current_branch: &str,
    worktree_path: &std::path::Path,
    repo: &Repository,
    config: &WorktrunkConfig,
    force: bool,
    target_branch: Option<&str>,
) -> Result<(), GitError> {
    let Some(pre_commit_config) = &project_config.pre_commit_command else {
        return Ok(());
    };

    let repo_root = repo.main_worktree_root()?;
    let ctx = CommandContext::new(
        repo,
        config,
        current_branch,
        worktree_path,
        &repo_root,
        force,
    );

    // Build extra variables - include target if provided
    let extra_vars: Vec<(&str, &str)> = if let Some(target) = target_branch {
        vec![("target", target)]
    } else {
        vec![]
    };

    let commands = prepare_project_commands(
        pre_commit_config,
        &ctx,
        false,
        &extra_vars,
        "Pre-commit commands",
        |_name, command| {
            let dim = AnstyleStyle::new().dimmed();
            crate::output::progress(format!("{dim}Skipping command: {command}{dim:#}")).ok();
        },
    )?;

    if commands.is_empty() {
        return Ok(());
    }

    // Execute each command sequentially
    for prepared in commands {
        let label = crate::commands::format_command_label("pre-commit", prepared.name.as_deref());
        crate::output::progress(format!("🔄 {CYAN}{label}{CYAN:#}"))?;
        crate::output::progress(format_bash_with_gutter(&prepared.expanded, ""))?;

        if let Err(e) = execute_command_in_worktree(worktree_path, &prepared.expanded) {
            return Err(GitError::HookCommandFailed {
                hook_type: HookType::PreCommit,
                command_name: prepared.name.clone(),
                error: e.to_string(),
            });
        }
    }

    crate::output::flush()?;

    Ok(())
}
