use anyhow::Context;
use color_print::cformat;
use worktrunk::HookType;
use worktrunk::git::Repository;
use worktrunk::styling::format_with_gutter;

use super::commit::{CommitGenerator, CommitOptions};
use super::context::CommandEnv;
use super::hooks::HookPipeline;
use super::merge::{
    execute_post_merge_commands, execute_pre_remove_commands, run_pre_merge_commands,
};
use super::project_config::collect_commands_for_hooks;
use super::repository_ext::RepositoryCliExt;

/// Handle `wt step hook` command
pub fn handle_standalone_run_hook(hook_type: HookType, force: bool) -> anyhow::Result<()> {
    // Derive context from current environment
    let env = CommandEnv::for_action(&format!("run {hook_type} hook"))?;
    let repo = &env.repo;
    let ctx = env.context(force);

    // Load project config (show helpful error if missing)
    let project_config = repo.require_project_config()?;

    // TODO: Add support for custom variable overrides (e.g., --var key=value)
    // This would allow testing hooks with different contexts without being in that context

    // Execute the hook based on type
    match hook_type {
        HookType::PostCreate => {
            check_hook_configured(&project_config.post_create, hook_type)?;
            ctx.execute_post_create_commands()
        }
        HookType::PostStart => {
            check_hook_configured(&project_config.post_start, hook_type)?;
            ctx.execute_post_start_commands_sequential()
        }
        HookType::PreCommit => {
            check_hook_configured(&project_config.pre_commit, hook_type)?;
            // Pre-commit hook can optionally use target branch context
            let target_branch = repo.default_branch().ok();
            HookPipeline::new(ctx).run_pre_commit(&project_config, target_branch.as_deref(), false)
        }
        HookType::PreMerge => {
            check_hook_configured(&project_config.pre_merge, hook_type)?;
            // Use current branch as target - when running standalone, the "target"
            // represents what branch we're on (vs. in `wt merge` where it's the
            // branch being merged into)
            run_pre_merge_commands(&project_config, &ctx, &env.branch, false)
        }
        HookType::PostMerge => {
            check_hook_configured(&project_config.post_merge, hook_type)?;
            // Use current branch as target - when running standalone, the "target"
            // represents what branch we're on (vs. in `wt merge` where it's the
            // branch being merged into)
            execute_post_merge_commands(&ctx, &env.branch, false)
        }
        HookType::PreRemove => {
            check_hook_configured(&project_config.pre_remove, hook_type)?;
            execute_pre_remove_commands(&ctx, false)
        }
    }
}

fn check_hook_configured<T>(hook: &Option<T>, hook_type: HookType) -> anyhow::Result<()> {
    if hook.is_none() {
        return Err(worktrunk::git::GitError::Other {
            message: format!("No {hook_type} hook configured"),
        }
        .into());
    }
    Ok(())
}

/// Handle `wt step commit` command
pub fn handle_standalone_commit(
    force: bool,
    no_verify: bool,
    stage_mode: super::commit::StageMode,
) -> anyhow::Result<()> {
    let env = CommandEnv::for_action("commit")?;
    let ctx = env.context(force);
    let mut options = CommitOptions::new(&ctx);
    options.no_verify = no_verify;
    options.stage_mode = stage_mode;
    options.auto_trust = false;
    options.show_no_squash_note = false;
    // Only warn about untracked if we're staging all
    options.warn_about_untracked = stage_mode == super::commit::StageMode::All;

    options.commit()
}

/// Result of a squash operation
#[derive(Debug, Clone)]
pub enum SquashResult {
    /// Squash or commit occurred
    Squashed,
    /// Nothing to squash: no commits ahead of target branch
    NoCommitsAhead(String),
    /// Nothing to squash: already a single commit
    AlreadySingleCommit,
    /// Squash attempted but resulted in no net changes (commits canceled out)
    NoNetChanges,
}

/// Handle shared squash workflow (used by `wt step squash` and `wt merge`)
///
/// # Arguments
/// * `auto_trust` - If true, skip approval prompts for pre-commit commands (already approved in batch)
/// * `stage_mode` - What to stage before committing (All or Tracked; None not supported for squash)
pub fn handle_squash(
    target: Option<&str>,
    force: bool,
    skip_pre_commit: bool,
    auto_trust: bool,
    stage_mode: super::commit::StageMode,
) -> anyhow::Result<SquashResult> {
    use super::commit::StageMode;

    let env = CommandEnv::for_action("squash")?;
    let repo = &env.repo;
    let current_branch = env.branch.clone();
    let ctx = env.context(force);
    let generator = CommitGenerator::new(&env.config.commit_generation);

    // Get target branch (default to default branch if not provided)
    let target_branch = repo.resolve_target_branch(target)?;

    // Auto-stage changes before running pre-commit hooks so both beta and merge paths behave identically
    match stage_mode {
        StageMode::All => {
            repo.warn_if_auto_staging_untracked()?;
            repo.run_command(&["add", "-A"])
                .context("Failed to stage changes")?;
        }
        StageMode::Tracked => {
            repo.run_command(&["add", "-u"])
                .context("Failed to stage tracked changes")?;
        }
        StageMode::None => {
            // Stage nothing - use what's already staged
        }
    }

    // Run pre-commit hook unless explicitly skipped
    let project_config = repo.load_project_config()?;
    let has_pre_commit = project_config
        .as_ref()
        .map(|c| c.pre_commit.is_some())
        .unwrap_or(false);

    if skip_pre_commit && has_pre_commit {
        crate::output::hint(cformat!(
            "Skipping pre-commit hook (<bright-black>--no-verify</>)"
        ))?;
    } else if let Some(ref config) = project_config {
        HookPipeline::new(ctx).run_pre_commit(config, Some(&target_branch), auto_trust)?;
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
        return Ok(SquashResult::NoCommitsAhead(target_branch));
    }

    if commit_count == 0 && has_staged {
        // Just staged changes, no commits - commit them directly (no squashing needed)
        generator.commit_staged_changes(true, stage_mode)?;
        return Ok(SquashResult::Squashed);
    }

    if commit_count == 1 && !has_staged {
        // Single commit, no staged changes - already squashed
        return Ok(SquashResult::AlreadySingleCommit);
    }

    // Either multiple commits OR single commit with staged changes - squash them
    // Get diff stats early for display in progress message
    let range = format!("{}..HEAD", merge_base);

    let commit_text = if commit_count == 1 {
        "commit"
    } else {
        "commits"
    };

    // Get total stats (commits + any working tree changes)
    let total_stats = if has_staged {
        repo.diff_stats_summary(&["diff", "--shortstat", &merge_base, "--cached"])
    } else {
        repo.diff_stats_summary(&["diff", "--shortstat", &range])
    };

    let with_changes = if has_staged {
        match stage_mode {
            super::commit::StageMode::Tracked => " & tracked changes",
            _ => " & working tree changes",
        }
    } else {
        ""
    };

    // Build parenthesized content: stats only (stage mode is in message text)
    let parts = total_stats;

    let squash_progress = if parts.is_empty() {
        format!("Squashing {commit_count} {commit_text}{with_changes} into a single commit...")
    } else {
        // Gray parenthetical with separate cformat for closing paren (avoids optimizer)
        let parts_str = parts.join(", ");
        let paren_close = cformat!("<bright-black>)</>");
        cformat!(
            "Squashing {commit_count} {commit_text}{with_changes} into a single commit <bright-black>({parts_str}</>{paren_close}..."
        )
    };
    crate::output::progress(squash_progress)?;

    // Create safety backup before potentially destructive reset if there are working tree changes
    if has_staged {
        let backup_message = format!("{} → {} (squash)", current_branch, target_branch);
        let (sha, _restore_cmd) = repo.create_safety_backup(&backup_message)?;
        crate::output::hint(format!("Backup created @ {sha}"))?;
    }

    // Get commit subjects for the squash message
    let subjects = repo.commit_subjects(&range)?;

    // Generate squash commit message
    crate::output::progress("Generating squash commit message...")?;

    generator.emit_hint_if_needed()?;

    // Get current branch and repo name for template variables
    let repo_root = repo.worktree_root()?;
    let repo_name = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo");

    let commit_message = crate::llm::generate_squash_message(
        &target_branch,
        &merge_base,
        &subjects,
        &current_branch,
        repo_name,
        &env.config.commit_generation,
    )?;

    // Display the generated commit message
    let formatted_message = generator.format_message_for_display(&commit_message);
    crate::output::gutter(format_with_gutter(&formatted_message, "", None))?;

    // Reset to merge base (soft reset stages all changes, including any already-staged uncommitted changes)
    repo.run_command(&["reset", "--soft", &merge_base])
        .context("Failed to reset to merge base")?;

    // Check if there are actually any changes to commit
    if !repo.has_staged_changes()? {
        crate::output::info(format!(
            "No changes after squashing {commit_count} {commit_text}"
        ))?;
        return Ok(SquashResult::NoNetChanges);
    }

    // Commit with the generated message
    repo.run_command(&["commit", "-m", &commit_message])
        .context("Failed to create squash commit")?;

    // Get commit hash for display
    let commit_hash = repo
        .run_command(&["rev-parse", "--short", "HEAD"])?
        .trim()
        .to_string();

    // Show success immediately after completing the squash
    crate::output::success(cformat!("Squashed @ <dim>{commit_hash}</>"))?;

    Ok(SquashResult::Squashed)
}

/// Result of a rebase operation
pub enum RebaseResult {
    /// Rebase occurred (either true rebase or fast-forward)
    Rebased,
    /// Already up-to-date with target branch
    UpToDate(String),
}

/// Handle shared rebase workflow (used by `wt step rebase` and `wt merge`)
pub fn handle_rebase(target: Option<&str>) -> anyhow::Result<RebaseResult> {
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
        return Ok(RebaseResult::UpToDate(target_branch));
    }

    // Check if this is a fast-forward or true rebase
    let head_sha = repo.run_command(&["rev-parse", "HEAD"])?.trim().to_string();
    let is_fast_forward = merge_base == head_sha;

    // Only show progress for true rebases (fast-forwards are instant)
    if !is_fast_forward {
        crate::output::progress(cformat!("Rebasing onto <bold>{target_branch}</>..."))?;
    }

    let rebase_result = repo.run_command(&["rebase", &target_branch]);

    // If rebase failed, check if it's due to conflicts
    if let Err(e) = rebase_result {
        if let Some(state) = repo.worktree_state()?
            && state.starts_with("REBASING")
        {
            // Extract git's stderr output from the error
            let git_output = e.to_string();
            return Err(worktrunk::git::GitError::RebaseConflict {
                target_branch: target_branch.clone(),
                git_output,
            }
            .into());
        }
        // Not a rebase conflict, return original error
        return Err(worktrunk::git::GitError::Other {
            message: format!("Failed to rebase onto '{}': {}", target_branch, e),
        }
        .into());
    }

    // Verify rebase completed successfully (safety check for edge cases)
    if let Some(state) = repo.worktree_state()? {
        let _ = state; // used for diagnostics
        return Err(worktrunk::git::GitError::RebaseConflict {
            target_branch: target_branch.clone(),
            git_output: String::new(),
        }
        .into());
    }

    // Success
    if is_fast_forward {
        crate::output::success(cformat!("Fast-forwarded to <bold>{target_branch}</>"))?;
    } else {
        crate::output::success(cformat!("Rebased onto <bold>{target_branch}</>"))?;
    }

    Ok(RebaseResult::Rebased)
}

/// Handle `wt config approvals add` command - approve all commands in the project
pub fn handle_standalone_add_approvals(force: bool, show_all: bool) -> anyhow::Result<()> {
    use super::command_approval::approve_command_batch;
    use worktrunk::config::WorktrunkConfig;

    let repo = Repository::current();
    let project_id = repo.project_identifier()?;
    let config = WorktrunkConfig::load().context("Failed to load config")?;

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
        crate::output::info("No commands configured in project")?;
        return Ok(());
    }

    // Filter to only unapproved commands (unless --all is specified)
    let commands_to_approve = if !show_all {
        let unapproved: Vec<_> = commands
            .into_iter()
            .filter(|cmd| !config.is_command_approved(&project_id, &cmd.template))
            .collect();

        if unapproved.is_empty() {
            crate::output::info("All commands already approved")?;
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
        if force {
            // When using --force, commands aren't saved to config
            crate::output::success("Commands approved; not saved (--force)")?;
        } else {
            // Interactive approval - commands were saved to config (unless save failed)
            crate::output::success("Commands approved & saved to config")?;
        }
    } else {
        crate::output::info("Commands declined")?;
    }

    Ok(())
}

/// Handle `wt config approvals clear` command - clear approved commands
pub fn handle_standalone_clear_approvals(global: bool) -> anyhow::Result<()> {
    use worktrunk::config::WorktrunkConfig;

    let mut config = WorktrunkConfig::load().context("Failed to load config")?;

    if global {
        // Clear all approvals for all projects
        let project_count = config.projects.len();

        if project_count == 0 {
            crate::output::info("No approvals to clear")?;
            return Ok(());
        }

        config.projects.clear();
        config.save().context("Failed to save config")?;

        crate::output::success(format!(
            "Cleared approvals for {project_count} project{}",
            if project_count == 1 { "" } else { "s" }
        ))?;
    } else {
        // Clear approvals for current project (default)
        let repo = Repository::current();
        let project_id = repo.project_identifier()?;

        // Check if project has any approvals
        let had_approvals = config.projects.contains_key(&project_id);

        if !had_approvals {
            crate::output::info("No approvals to clear for this project")?;
            return Ok(());
        }

        // Count approvals before removing
        let approval_count = config
            .projects
            .get(&project_id)
            .map(|p| p.approved_commands.len())
            .unwrap_or(0);

        config
            .revoke_project(&project_id)
            .context("Failed to clear project approvals")?;

        crate::output::success(format!(
            "Cleared {approval_count} approval{} for this project",
            if approval_count == 1 { "" } else { "s" }
        ))?;
    }

    Ok(())
}
