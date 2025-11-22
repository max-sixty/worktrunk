use anyhow::{Context, bail};
use worktrunk::HookType;
use worktrunk::git::Repository;
use worktrunk::styling::{AnstyleStyle, CYAN, CYAN_BOLD, GREEN_BOLD, format_with_gutter};

use super::commit::{CommitGenerator, CommitOptions};
use super::context::CommandEnv;
use super::hooks::HookPipeline;
use super::merge::{execute_post_merge_commands, run_pre_merge_commands};
use super::project_config::collect_commands_for_hooks;
use super::repository_ext::RepositoryCliExt;

/// Handle `wt step hook` command
pub fn handle_standalone_run_hook(hook_type: HookType, force: bool) -> anyhow::Result<()> {
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
            ctx.execute_post_create_commands()
        }
        HookType::PostStart => {
            check_hook_configured(&project_config.post_start_command, hook_type)?;
            ctx.execute_post_start_commands_sequential()
        }
        HookType::PreCommit => {
            check_hook_configured(&project_config.pre_commit_command, hook_type)?;
            // Pre-commit hook can optionally use target branch context
            let target_branch = repo.default_branch().ok();
            HookPipeline::new(ctx).run_pre_commit(&project_config, target_branch.as_deref(), false)
        }
        HookType::PreMerge => {
            check_hook_configured(&project_config.pre_merge_command, hook_type)?;
            let target_branch = repo.default_branch().unwrap_or_else(|_| "main".to_string());
            run_pre_merge_commands(&project_config, &ctx, &target_branch, false)
        }
        HookType::PostMerge => {
            check_hook_configured(&project_config.post_merge_command, hook_type)?;
            let target_branch = repo.default_branch().unwrap_or_else(|_| "main".to_string());
            execute_post_merge_commands(&ctx, &target_branch, false)
        }
    }
}

fn check_hook_configured<T>(hook: &Option<T>, hook_type: HookType) -> anyhow::Result<()> {
    if hook.is_none() {
        return Err(anyhow::anyhow!(format!("No {hook_type} hook configured")));
    }
    Ok(())
}

/// Handle `wt step commit` command
pub fn handle_standalone_commit(
    force: bool,
    no_verify: bool,
    stage_mode: super::commit::StageMode,
) -> anyhow::Result<()> {
    let env = CommandEnv::current()?;
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

/// Handle shared squash workflow (used by `wt step squash` and `wt merge`)
///
/// # Arguments
/// * `auto_trust` - If true, skip approval prompts for pre-commit commands (already approved in batch)
/// * `stage_mode` - What to stage before committing (All or Tracked; None not supported for squash)
///
/// Returns true if a commit or squash operation occurred, false if nothing needed to be done
pub fn handle_squash(
    target: Option<&str>,
    force: bool,
    skip_pre_commit: bool,
    auto_trust: bool,
    stage_mode: super::commit::StageMode,
) -> anyhow::Result<bool> {
    use super::commit::StageMode;

    let env = CommandEnv::current()?;
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
    if !skip_pre_commit && let Some(project_config) = repo.load_project_config()? {
        HookPipeline::new(ctx).run_pre_commit(&project_config, Some(&target_branch), auto_trust)?;
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
        generator.commit_staged_changes(true)?;
        return Ok(true);
    }

    if commit_count == 1 && !has_staged {
        // Single commit, no staged changes - nothing to do
        return Ok(false);
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
        " & working tree changes"
    } else {
        ""
    };

    let squash_progress = if total_stats.is_empty() {
        format!(
            "{CYAN}Squashing {commit_count} {commit_text}{with_changes} into a single commit...{CYAN:#}"
        )
    } else {
        format!(
            "{CYAN}Squashing {commit_count} {commit_text}{with_changes} into a single commit{CYAN:#} ({})...",
            total_stats.join(", ")
        )
    };
    crate::output::progress(squash_progress)?;

    // Create safety backup before potentially destructive reset if there are working tree changes
    if has_staged {
        let backup_message = format!("{} → {} (squash)", current_branch, target_branch);
        let (sha, _restore_cmd) = repo.create_safety_backup(&backup_message)?;
        use worktrunk::styling::AnstyleStyle;
        let dim = AnstyleStyle::new().dimmed();
        crate::output::hint(format!("Backup created @ {dim}{sha}{dim:#}"))?;
    }

    // Get commit subjects for the squash message
    let subjects = repo.commit_subjects(&range)?;

    // Generate squash commit message
    crate::output::progress(format!("{CYAN}Generating squash commit message...{CYAN:#}"))?;

    generator.emit_hint_if_needed()?;

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
    .context("Failed to generate commit message")?;

    // Display the generated commit message
    let formatted_message = generator.format_message_for_display(&commit_message);
    crate::output::gutter(format_with_gutter(&formatted_message, "", None))?;

    // Reset to merge base (soft reset stages all changes, including any already-staged uncommitted changes)
    repo.run_command(&["reset", "--soft", &merge_base])
        .context("Failed to reset to merge base")?;

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
        .context("Failed to create squash commit")?;

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

/// Handle shared rebase workflow (used by `wt step rebase` and `wt merge`)
/// Returns true if rebasing occurred, false if already up-to-date
pub fn handle_rebase(target: Option<&str>) -> anyhow::Result<bool> {
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

    // Check if this is a fast-forward or true rebase
    let head_sha = repo.run_command(&["rev-parse", "HEAD"])?.trim().to_string();
    let is_fast_forward = merge_base == head_sha;

    // Only show progress for true rebases (fast-forwards are instant)
    if !is_fast_forward {
        crate::output::progress(format!(
            "{CYAN}Rebasing onto {CYAN_BOLD}{target_branch}{CYAN_BOLD:#}{CYAN}...{CYAN:#}"
        ))?;
    }

    let rebase_result = repo.run_command(&["rebase", &target_branch]);

    // If rebase failed, check if it's due to conflicts
    if let Err(e) = rebase_result {
        if let Some(state) = repo.worktree_state()?
            && state.starts_with("REBASING")
        {
            // Extract git's stderr output from the error
            let git_output = e.to_string();
            bail!(
                "{}",
                worktrunk::git::rebase_conflict(&target_branch, &git_output)
            );
        }
        // Not a rebase conflict, return original error
        bail!("Failed to rebase onto '{}': {}", target_branch, e);
    }

    // Verify rebase completed successfully (safety check for edge cases)
    if let Some(state) = repo.worktree_state()? {
        let _ = state; // used for diagnostics
        return Err(worktrunk::git::rebase_conflict(&target_branch, ""));
    }

    // Success
    use worktrunk::styling::GREEN;
    if is_fast_forward {
        crate::output::success(format!(
            "{GREEN}Fast-forwarded to {GREEN_BOLD}{target_branch}{GREEN_BOLD:#}{GREEN:#}"
        ))?;
    } else {
        crate::output::success(format!(
            "{GREEN}Rebased onto {GREEN_BOLD}{target_branch}{GREEN_BOLD:#}{GREEN:#}"
        ))?;
    }

    Ok(true)
}

/// Handle `wt config approvals ask` command - approve all commands in the project
pub fn handle_standalone_ask_approvals(force: bool, show_all: bool) -> anyhow::Result<()> {
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
        use worktrunk::styling::GREEN;

        if force {
            // When using --force, commands aren't saved to config
            crate::output::success(format!(
                "{GREEN}Commands approved; not saved (--force){GREEN:#}"
            ))?;
        } else {
            // Interactive approval - commands were saved to config (unless save failed)
            crate::output::success(format!(
                "{GREEN}Commands approved & saved to config{GREEN:#}"
            ))?;
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
            let dim = worktrunk::styling::AnstyleStyle::new().dimmed();
            crate::output::info(format!("{dim}No approvals to clear{dim:#}"))?;
            return Ok(());
        }

        config.projects.clear();
        config.save().context("Failed to save config")?;

        use worktrunk::styling::GREEN;
        crate::output::success(format!(
            "{GREEN}Cleared approvals for {project_count} project{}{GREEN:#}",
            if project_count == 1 { "" } else { "s" }
        ))?;
    } else {
        // Clear approvals for current project (default)
        let repo = Repository::current();
        let project_id = repo.project_identifier()?;

        // Check if project has any approvals
        let had_approvals = config.projects.contains_key(&project_id);

        if !had_approvals {
            let dim = worktrunk::styling::AnstyleStyle::new().dimmed();
            crate::output::info(format!(
                "{dim}No approvals to clear for this project{dim:#}"
            ))?;
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

        use worktrunk::styling::GREEN;
        crate::output::success(format!(
            "{GREEN}Cleared {approval_count} approval{} for this project{GREEN:#}",
            if approval_count == 1 { "" } else { "s" }
        ))?;
    }

    Ok(())
}
