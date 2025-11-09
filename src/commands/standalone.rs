use worktrunk::HookType;
use worktrunk::git::{GitError, GitResultExt, Repository};
use worktrunk::styling::{
    AnstyleStyle, CYAN, CYAN_BOLD, GREEN_BOLD, HINT, HINT_EMOJI, eprintln, format_with_gutter,
};

use super::commit::{
    CommitOptions, commit_changes, commit_staged_changes, format_commit_message_for_display,
    run_pre_commit_commands, show_llm_config_hint_if_needed, warn_untracked_auto_stage,
};
use super::context::CommandEnv;
use super::merge::{execute_post_merge_commands, run_pre_merge_commands};
use super::project_config::{ProjectConfigRepoExt, collect_commands_for_hooks};
use super::worktree::{execute_post_create_commands, execute_post_start_commands_sequential};

/// Handle `wt beta run-hook` command
pub fn handle_standalone_run_hook(hook_type: HookType, force: bool) -> Result<(), GitError> {
    // Derive context from current environment
    let env = CommandEnv::current()?;
    let repo = &env.repo;
    let ctx = env.context(force);

    // Load project config (show helpful error if missing)
    let project_config = repo.require_project_config()?;

    // TODO: Add support for custom variable overrides (e.g., --var key=value)
    // This would allow testing hooks with different contexts without being in that context

    // Execute the hook based on type
    match hook_type {
        HookType::PostCreate => {
            check_hook_configured(&project_config.post_create_command, hook_type)?;
            execute_post_create_commands(&ctx)
        }
        HookType::PostStart => {
            check_hook_configured(&project_config.post_start_command, hook_type)?;
            execute_post_start_commands_sequential(&ctx)
        }
        HookType::PreCommit => {
            check_hook_configured(&project_config.pre_commit_command, hook_type)?;
            // Pre-commit hook can optionally use target branch context
            let target_branch = repo.default_branch().ok();
            run_pre_commit_commands(
                &project_config,
                &ctx,
                target_branch.as_deref(),
                false, // auto_trust: standalone command, needs approval
            )
        }
        HookType::PreMerge => {
            check_hook_configured(&project_config.pre_merge_command, hook_type)?;
            let target_branch = repo.default_branch().unwrap_or_else(|_| "main".to_string());
            run_pre_merge_commands(&project_config, &ctx, &target_branch)
        }
        HookType::PostMerge => {
            check_hook_configured(&project_config.post_merge_command, hook_type)?;
            let target_branch = repo.default_branch().unwrap_or_else(|_| "main".to_string());
            execute_post_merge_commands(&ctx, &target_branch)
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

/// Handle `wt beta commit` command
pub fn handle_standalone_commit(force: bool, no_verify: bool) -> Result<(), GitError> {
    let env = CommandEnv::current()?;
    let ctx = env.context(force);
    let mut options = CommitOptions::new(&ctx);
    options.no_verify = no_verify;
    options.auto_trust = false;
    options.show_no_squash_note = false;

    commit_changes(options)
}

/// Handle shared squash workflow (used by `wt beta squash` and `wt merge`)
///
/// # Arguments
/// * `auto_trust` - If true, skip approval prompts for pre-commit commands (already approved in batch)
/// * `tracked_only` - Stage only tracked files (mirrors `--tracked-only` in merge)
/// * `warn_about_untracked` - Emit a warning before auto-staging untracked files
///
/// Returns true if a commit or squash operation occurred, false if nothing needed to be done
pub fn handle_squash(
    target: Option<&str>,
    force: bool,
    skip_pre_commit: bool,
    auto_trust: bool,
    tracked_only: bool,
    warn_about_untracked: bool,
) -> Result<bool, GitError> {
    let env = CommandEnv::current()?;
    let repo = &env.repo;
    let current_branch = env.branch.clone();
    let ctx = env.context(force);

    // Get target branch (default to default branch if not provided)
    let target_branch = repo.resolve_target_branch(target)?;

    // Auto-stage changes before running pre-commit hooks so both beta and merge paths behave identically
    if warn_about_untracked && !tracked_only {
        warn_untracked_auto_stage(repo)?;
    }

    if tracked_only {
        repo.run_command(&["add", "-u"])
            .git_context("Failed to stage tracked changes")?;
    } else {
        repo.run_command(&["add", "-A"])
            .git_context("Failed to stage changes")?;
    }

    // Run pre-commit hook unless explicitly skipped
    if !skip_pre_commit && let Some(project_config) = repo.load_project_config()? {
        run_pre_commit_commands(&project_config, &ctx, Some(&target_branch), auto_trust)?;
    }

    // Get merge base with target branch
    let merge_base = repo.merge_base("HEAD", &target_branch)?;

    // Count commits since merge base
    let commit_count = repo.count_commits(&merge_base, "HEAD")?;

    // Check if there are staged changes in addition to commits
    let has_staged = repo.has_staged_changes()?;

    // Handle different scenarios
    if commit_count == 0 && !has_staged {
        // No commits and no staged changes - nothing to squash
        return Ok(false);
    }

    if commit_count == 0 && has_staged {
        // Just staged changes, no commits - commit them directly (no squashing needed)
        commit_staged_changes(&env.config.commit_generation, true)?;
        return Ok(true);
    }

    if commit_count == 1 && !has_staged {
        // Single commit, no staged changes - nothing to do
        return Ok(false);
    }

    // Either multiple commits OR single commit with staged changes - squash them
    // Get diff stats early for display in progress message
    let range = format!("{}..HEAD", merge_base);
    let stats_parts = repo.diff_stats_summary(&["diff", "--shortstat", &range]);

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
        true => {
            format!("{CYAN}Squashing {commit_count} {commit_text}{with_changes} into 1...{CYAN:#}")
        }
        false => format!(
            "{CYAN}Squashing {commit_count} {commit_text}{with_changes} into 1{CYAN:#} ({})...",
            stats_parts.join(", ")
        ),
    };
    crate::output::progress(squash_progress)?;

    // Get commit subjects for the squash message
    let subjects = repo.commit_subjects(&range)?;

    // Generate squash commit message
    crate::output::progress(format!("{CYAN}Generating squash commit message...{CYAN:#}"))?;

    show_llm_config_hint_if_needed(&env.config.commit_generation)?;

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
        &env.config.commit_generation,
    )
    .git_context("Failed to generate commit message")?;

    // Display the generated commit message
    let formatted_message = format_commit_message_for_display(&commit_message);
    crate::output::gutter(format_with_gutter(&formatted_message, "", None))?;

    // Reset to merge base (soft reset stages all changes, including any already-staged uncommitted changes)
    repo.run_command(&["reset", "--soft", &merge_base])
        .git_context("Failed to reset to merge base")?;

    // Check if there are actually any changes to commit
    if !repo.has_staged_changes()? {
        let dim = AnstyleStyle::new().dimmed();
        crate::output::info(format!(
            "{dim}No changes after squashing {commit_count} {commit_text}{dim:#}"
        ))?;
        return Ok(false);
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
    use worktrunk::styling::GREEN;
    let green_dim = GREEN.dimmed();
    crate::output::success(format!(
        "{GREEN}Squashed @ {green_dim}{commit_hash}{green_dim:#}{GREEN:#}"
    ))?;

    Ok(true)
}

/// Handle shared rebase workflow (used by `wt beta rebase` and `wt merge`)
/// Returns true if rebasing occurred, false if already up-to-date
pub fn handle_rebase(target: Option<&str>) -> Result<bool, GitError> {
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
        "{CYAN}Rebasing onto {CYAN_BOLD}{target_branch}{CYAN_BOLD:#}{CYAN}...{CYAN:#}"
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
    use worktrunk::styling::GREEN;
    crate::output::success(format!(
        "{GREEN}Rebased onto {GREEN_BOLD}{target_branch}{GREEN_BOLD:#}{GREEN:#}"
    ))?;

    Ok(true)
}

/// Handle `wt beta ask-approvals` command - approve all commands in the project
pub fn handle_standalone_ask_approvals(force: bool, show_all: bool) -> Result<(), GitError> {
    use super::command_approval::approve_command_batch;
    use worktrunk::config::WorktrunkConfig;

    let repo = Repository::current();
    let project_id = repo.project_identifier()?;
    let config = WorktrunkConfig::load().git_context("Failed to load config")?;

    // Load project config (show helpful error if missing)
    let project_config = repo.require_project_config()?;

    // Collect all commands from the project config
    let all_hooks = [
        HookType::PostCreate,
        HookType::PostStart,
        HookType::PreCommit,
        HookType::PreMerge,
        HookType::PostMerge,
    ];
    let commands = collect_commands_for_hooks(&project_config, &all_hooks);

    if commands.is_empty() {
        let dim = worktrunk::styling::AnstyleStyle::new().dimmed();
        crate::output::info(format!("{dim}No commands configured in project{dim:#}"))?;
        return Ok(());
    }

    // Filter to only unapproved commands (unless --all is specified)
    let commands_to_approve = if !show_all {
        let unapproved: Vec<_> = commands
            .into_iter()
            .filter(|cmd| !config.is_command_approved(&project_id, &cmd.template))
            .collect();

        if unapproved.is_empty() {
            let dim = worktrunk::styling::AnstyleStyle::new().dimmed();
            crate::output::info(format!("{dim}All commands already approved{dim:#}"))?;
            return Ok(());
        }

        unapproved
    } else {
        commands
    };

    // Call the approval prompt
    // When show_all=true, we've already included all commands in commands_to_approve
    // When show_all=false, we've already filtered to unapproved commands
    // So we pass skip_approval_filter=true to prevent double-filtering
    let approved = approve_command_batch(&commands_to_approve, &project_id, &config, force, true)?;

    // Show result
    if approved {
        use worktrunk::styling::GREEN;

        if force {
            // When using --force, commands aren't saved to config
            crate::output::success(format!(
                "{GREEN}Commands approved (not saved with --force){GREEN:#}"
            ))?;
        } else {
            // Interactive approval - commands were saved to config (unless save failed)
            crate::output::success(format!(
                "{GREEN}Commands approved and saved to config{GREEN:#}"
            ))?;
        }
    } else {
        let dim = worktrunk::styling::AnstyleStyle::new().dimmed();
        crate::output::info(format!("{dim}Commands declined{dim:#}"))?;
    }

    Ok(())
}
