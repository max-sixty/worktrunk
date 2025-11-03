use worktrunk::HookType;
use worktrunk::config::{ProjectConfig, WorktrunkConfig};
use worktrunk::git::{GitError, GitResultExt, Repository};
use worktrunk::styling::{
    AnstyleStyle, CYAN, CYAN_BOLD, HINT, HINT_EMOJI, eprintln, format_with_gutter,
};

use super::merge::{
    commit_with_generated_message, execute_post_merge_commands, format_commit_message_for_display,
    run_pre_commit_commands, run_pre_merge_commands, show_llm_config_hint_if_needed,
};
use super::worktree::{
    execute_post_create_commands, execute_post_start_commands_sequential, parse_diff_shortstat,
};

/// Handle `wt dev run-hook` command
pub fn handle_dev_run_hook(hook_type: HookType, force: bool) -> Result<(), GitError> {
    // Derive context from current environment
    let repo = Repository::current();
    let worktree_path = std::env::current_dir()
        .map_err(|e| GitError::CommandFailed(format!("Failed to get current directory: {}", e)))?;
    let branch = repo
        .current_branch()
        .git_context("Failed to get current branch")?
        .ok_or_else(|| GitError::CommandFailed("Not on a branch (detached HEAD)".to_string()))?;
    let config = WorktrunkConfig::load().git_context("Failed to load config")?;

    // Load project config (show helpful error if missing)
    let project_config = load_project_config(&repo)?;

    // TODO: Add support for custom variable overrides (e.g., --var key=value)
    // This would allow testing hooks with different contexts without being in that context

    // Execute the hook based on type
    match hook_type {
        HookType::PostCreate => {
            check_hook_configured(&project_config.post_create_command, hook_type)?;
            execute_post_create_commands(&worktree_path, &repo, &config, &branch, force)
        }
        HookType::PostStart => {
            check_hook_configured(&project_config.post_start_command, hook_type)?;
            execute_post_start_commands_sequential(&worktree_path, &repo, &config, &branch, force)
        }
        HookType::PreCommit => {
            check_hook_configured(&project_config.pre_commit_command, hook_type)?;
            // Pre-commit hook can optionally use target branch context
            let target_branch = repo.default_branch().ok();
            run_pre_commit_commands(
                &project_config,
                &branch,
                &worktree_path,
                &repo,
                &config,
                force,
                target_branch.as_deref(),
            )
        }
        HookType::PreMerge => {
            check_hook_configured(&project_config.pre_merge_command, hook_type)?;
            let target_branch = repo.default_branch().unwrap_or_else(|_| "main".to_string());
            run_pre_merge_commands(
                &project_config,
                &branch,
                &target_branch,
                &worktree_path,
                &repo,
                &config,
                force,
            )
        }
        HookType::PostMerge => {
            check_hook_configured(&project_config.post_merge_command, hook_type)?;
            let target_branch = repo.default_branch().unwrap_or_else(|_| "main".to_string());
            execute_post_merge_commands(
                &worktree_path,
                &repo,
                &config,
                &branch,
                &target_branch,
                force,
            )
        }
    }
}

fn load_project_config(repo: &Repository) -> Result<ProjectConfig, GitError> {
    let repo_root = repo.worktree_root()?;
    let config_path = repo_root.join(".config").join("wt.toml");

    match ProjectConfig::load(&repo_root).git_context("Failed to load project config")? {
        Some(cfg) => Ok(cfg),
        None => {
            // No project config found - show helpful error
            use worktrunk::styling::ERROR;
            use worktrunk::styling::ERROR_EMOJI;
            let hint_bold = HINT.bold();
            eprintln!("{ERROR_EMOJI} {ERROR}No project configuration found{ERROR:#}",);
            eprintln!(
                "{HINT_EMOJI} {HINT}Create a config file at: {hint_bold}{}{hint_bold:#}{HINT:#}",
                config_path.display()
            );
            Err(GitError::CommandFailed(
                "No project configuration found".to_string(),
            ))
        }
    }
}

fn check_hook_configured<T>(hook: &Option<T>, hook_type: HookType) -> Result<(), GitError> {
    if hook.is_none() {
        eprintln!(
            "{HINT_EMOJI} {HINT}No {hook_type} commands configured in project config{HINT:#}"
        );
        return Err(GitError::CommandFailed(format!(
            "No {hook_type} commands configured"
        )));
    }
    Ok(())
}

/// Handle `wt dev commit` command
pub fn handle_dev_commit(force: bool, no_verify: bool) -> Result<(), GitError> {
    let repo = Repository::current();
    let config = WorktrunkConfig::load().git_context("Failed to load config")?;
    let current_branch = repo
        .current_branch()?
        .ok_or_else(|| GitError::CommandFailed("Not on a branch (detached HEAD)".to_string()))?;

    // Run pre-commit hook unless --no-verify was specified
    if !no_verify && let Ok(Some(project_config)) = ProjectConfig::load(&repo.worktree_root()?) {
        let worktree_path =
            std::env::current_dir().git_context("Failed to get current directory")?;
        // No target branch context for standalone commits
        run_pre_commit_commands(
            &project_config,
            &current_branch,
            &worktree_path,
            &repo,
            &config,
            force,
            None,
        )?;
    }

    // Stage all changes including untracked files
    repo.run_command(&["add", "-A"])
        .git_context("Failed to stage changes")?;

    commit_with_generated_message("Committing changes...", &config.commit_generation)
}

/// Handle `wt dev squash` command
/// Returns true if squashing occurred, false if no squashing was needed
pub fn handle_dev_squash(
    target: Option<&str>,
    force: bool,
    no_verify: bool,
) -> Result<bool, GitError> {
    let repo = Repository::current();
    let config = WorktrunkConfig::load().git_context("Failed to load config")?;
    let current_branch = repo
        .current_branch()?
        .ok_or_else(|| GitError::CommandFailed("Not on a branch (detached HEAD)".to_string()))?;

    // Get target branch (default to default branch if not provided)
    let target_branch = repo.resolve_target_branch(target)?;

    // Run pre-commit hook unless --no-verify was specified
    if !no_verify && let Ok(Some(project_config)) = ProjectConfig::load(&repo.worktree_root()?) {
        let worktree_path =
            std::env::current_dir().git_context("Failed to get current directory")?;
        run_pre_commit_commands(
            &project_config,
            &current_branch,
            &worktree_path,
            &repo,
            &config,
            force,
            Some(&target_branch),
        )?;
    }

    // Get merge base with target branch
    let merge_base = repo.merge_base("HEAD", &target_branch)?;

    // Count commits since merge base
    let commit_count = repo.count_commits(&merge_base, "HEAD")?;

    // Check if there are staged changes
    let has_staged = repo.has_staged_changes()?;

    // Handle different scenarios
    if commit_count == 0 && !has_staged {
        // No commits and no staged changes - nothing to squash
        let dim = AnstyleStyle::new().dimmed();
        crate::output::progress(format!(
            "{dim}No commits to squash - already at merge base{dim:#}"
        ))?;
        return Ok(false);
    }

    if commit_count == 0 && has_staged {
        // Just staged changes, no commits - commit them directly (no squashing needed)
        commit_with_generated_message("Committing changes...", &config.commit_generation)?;
        return Ok(false);
    }

    if commit_count == 1 && !has_staged {
        // Single commit, no staged changes - nothing to do
        // Don't show hint here - the merge message will indicate "no squashing needed"
        return Ok(false);
    }

    // Either multiple commits OR single commit with staged changes - squash them
    // Get diff stats early for display in progress message
    let range = format!("{}..HEAD", merge_base);
    let diff_shortstat = repo
        .run_command(&["diff", "--shortstat", &range])
        .unwrap_or_default();
    let stats = parse_diff_shortstat(&diff_shortstat);
    let stats_parts = stats.format_summary();

    let commit_text = if commit_count == 1 {
        "commit"
    } else {
        "commits"
    };
    let with_changes = if has_staged {
        " with working tree changes"
    } else {
        ""
    };
    let squash_progress = match stats_parts.is_empty() {
        true => format!(
            "🔄 {CYAN}Squashing {commit_count} {commit_text}{with_changes} into 1...{CYAN:#}"
        ),
        false => format!(
            "🔄 {CYAN}Squashing {commit_count} {commit_text}{with_changes} into 1{CYAN:#} ({})...",
            stats_parts.join(", ")
        ),
    };
    crate::output::progress(squash_progress)?;

    // Get commit subjects for the squash message
    let subjects = repo.commit_subjects(&range)?;

    // Generate squash commit message
    crate::output::progress(format!(
        "🔄 {CYAN}Generating squash commit message...{CYAN:#}"
    ))?;

    show_llm_config_hint_if_needed(&config.commit_generation)?;

    // Get current branch and repo name for template variables
    let repo_root = repo.worktree_root()?;
    let repo_name = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo");

    let commit_message = crate::llm::generate_squash_message(
        &target_branch,
        &subjects,
        &current_branch,
        repo_name,
        &config.commit_generation,
    )
    .git_context("Failed to generate commit message")?;

    // Display the generated commit message
    let formatted_message = format_commit_message_for_display(&commit_message);
    crate::output::progress(format_with_gutter(&formatted_message, "", None))?;

    // Reset to merge base (soft reset stages all changes, including any already-staged uncommitted changes)
    repo.run_command(&["reset", "--soft", &merge_base])
        .git_context("Failed to reset to merge base")?;

    // Check if there are actually any changes to commit
    if !repo.has_staged_changes()? {
        use worktrunk::styling::{ERROR, ERROR_EMOJI, HINT};
        return Err(GitError::CommandFailed(format!(
            "{ERROR_EMOJI} {ERROR}No changes to commit after squashing {commit_count} commits{ERROR:#}\n\n{HINT_EMOJI} {HINT}The commits resulted in no net changes (e.g., changes were reverted or already in {HINT:#}{CYAN_BOLD}{target_branch}{CYAN_BOLD:#}{HINT}){HINT:#}"
        )));
    }

    // Commit with the generated message
    repo.run_command(&["commit", "-m", &commit_message])
        .git_context("Failed to create squash commit")?;

    // Get commit hash for display
    let commit_hash = repo
        .run_command(&["rev-parse", "--short", "HEAD"])?
        .trim()
        .to_string();

    // Show success immediately after completing the squash
    use worktrunk::styling::{GREEN, SUCCESS_EMOJI};
    let green_dim = GREEN.dimmed();
    crate::output::success(format!(
        "{SUCCESS_EMOJI} {GREEN}Squashed @ {green_dim}{commit_hash}{green_dim:#}{GREEN:#}"
    ))?;

    Ok(true)
}

/// Handle `wt dev push` command
pub fn handle_dev_push(target: Option<&str>, allow_merge_commits: bool) -> Result<(), GitError> {
    super::worktree::handle_push(target, allow_merge_commits, "Pushed to", false, false)
}

/// Handle `wt dev rebase` command
/// Returns true if rebasing occurred, false if already up-to-date
pub fn handle_dev_rebase(target: Option<&str>) -> Result<bool, GitError> {
    let repo = Repository::current();

    // Get target branch (default to default branch if not provided)
    let target_branch = repo.resolve_target_branch(target)?;

    // Check if already up-to-date
    let merge_base = repo.merge_base("HEAD", &target_branch)?;
    let target_sha = repo
        .run_command(&["rev-parse", &target_branch])?
        .trim()
        .to_string();

    if merge_base == target_sha {
        // Already up-to-date, no rebase needed
        return Ok(false);
    }

    // Rebase onto target
    crate::output::progress(format!(
        "🔄 {CYAN}Rebasing onto {CYAN:#}{CYAN_BOLD}{target_branch}{CYAN_BOLD:#}{CYAN}...{CYAN:#}"
    ))?;

    let rebase_result = repo.run_command(&["rebase", &target_branch]);

    // If rebase failed, check if it's due to conflicts
    if let Err(e) = rebase_result {
        if let Some(state) = repo.worktree_state()?
            && state.starts_with("REBASING")
        {
            // Extract git's stderr output from the error
            let git_output = match &e {
                GitError::CommandFailed(msg) => msg.clone(),
                _ => e.to_string(),
            };
            return Err(GitError::RebaseConflict {
                state,
                target_branch: target_branch.to_string(),
                git_output,
            });
        }
        // Not a rebase conflict, return original error
        return Err(GitError::CommandFailed(format!(
            "Failed to rebase onto '{}': {}",
            target_branch, e
        )));
    }

    // Verify rebase completed successfully (safety check for edge cases)
    if let Some(state) = repo.worktree_state()? {
        return Err(GitError::RebaseConflict {
            state,
            target_branch: target_branch.to_string(),
            git_output: String::new(), // No error output in this edge case
        });
    }

    // Success
    use worktrunk::styling::{GREEN, SUCCESS_EMOJI};
    let green_bold = GREEN.bold();
    crate::output::success(format!(
        "{SUCCESS_EMOJI} {GREEN}Rebased onto {green_bold}{target_branch}{green_bold:#}{GREEN:#}"
    ))?;

    Ok(true)
}
