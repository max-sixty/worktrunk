use anyhow::Context;
use color_print::cformat;
use std::fmt::Write as _;
use worktrunk::HookType;
use worktrunk::config::{CommandConfig, ProjectConfig, WorktrunkConfig};
use worktrunk::git::Repository;
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{
    INFO_EMOJI, PROMPT_EMOJI, format_bash_with_gutter, format_heading, format_with_gutter,
    hint_message, info_message, progress_message, success_message,
};

use super::command_executor::CommandContext;
use super::commit::{CommitGenerator, CommitOptions};
use super::context::CommandEnv;
use super::hooks::{HookFailureStrategy, HookPipeline, run_hook_with_filter};
use super::merge::{
    execute_post_merge_commands, execute_pre_remove_commands, run_pre_merge_commands,
};
use super::project_config::collect_commands_for_hooks;
use super::repository_ext::RepositoryCliExt;

/// Handle `wt hook` command
///
/// When explicitly invoking hooks, ALL hooks run (both user and project).
/// There's no skip flag - if you explicitly run hooks, all configured hooks run.
///
/// Works in detached HEAD state - `{{ branch }}` template variable will be "HEAD".
pub fn run_hook(hook_type: HookType, force: bool, name_filter: Option<&str>) -> anyhow::Result<()> {
    use super::command_approval::approve_hooks_filtered;

    // Derive context from current environment (branch-optional for CI compatibility)
    let env = CommandEnv::for_action_branchless()?;
    let repo = &env.repo;
    let ctx = env.context(force);

    // Load project config (optional - user hooks can run without project config)
    let project_config = repo.load_project_config()?;

    // "Approve at the Gate": approve project hooks upfront
    // Pass name_filter to only approve the targeted hook, not all hooks of this type
    let approved = approve_hooks_filtered(&ctx, &[hook_type], name_filter)?;
    // If declined, return early - the whole point of `wt hook` is to run hooks
    if !approved {
        crate::output::print(worktrunk::styling::info_message("Commands declined"))?;
        return Ok(());
    }

    // TODO: Add support for custom variable overrides (e.g., --var key=value)
    // This would allow testing hooks with different contexts without being in that context

    // Helper to get user hook config
    macro_rules! user_hook {
        ($field:ident) => {
            ctx.config.$field.as_ref()
        };
    }

    /// Helper to require at least one hook is configured (for standalone `wt hook` command)
    fn require_hooks(
        user: Option<&CommandConfig>,
        project: Option<&CommandConfig>,
        hook_type: HookType,
    ) -> anyhow::Result<()> {
        if user.is_none() && project.is_none() {
            return Err(worktrunk::git::GitError::Other {
                message: format!("No {hook_type} hook configured (neither user nor project)"),
            }
            .into());
        }
        Ok(())
    }

    // Execute the hook based on type
    match hook_type {
        HookType::PostCreate => {
            let user_config = user_hook!(post_create);
            let project_config = project_config.as_ref().and_then(|c| c.post_create.as_ref());
            require_hooks(user_config, project_config, hook_type)?;
            run_hook_with_filter(
                &ctx,
                user_config,
                project_config,
                hook_type,
                &[],
                HookFailureStrategy::FailFast,
                name_filter,
            )
        }
        HookType::PostStart => {
            let user_config = user_hook!(post_start);
            let project_config = project_config.as_ref().and_then(|c| c.post_start.as_ref());
            require_hooks(user_config, project_config, hook_type)?;
            run_hook_with_filter(
                &ctx,
                user_config,
                project_config,
                hook_type,
                &[],
                HookFailureStrategy::FailFast,
                name_filter,
            )
        }
        HookType::PreCommit => {
            let user_config = user_hook!(pre_commit);
            let project_config = project_config.as_ref().and_then(|c| c.pre_commit.as_ref());
            require_hooks(user_config, project_config, hook_type)?;
            // Pre-commit hook can optionally use target branch context
            let target_branch = repo.default_branch().ok();
            let extra_vars: Vec<(&str, &str)> = target_branch
                .as_deref()
                .into_iter()
                .map(|t| ("target", t))
                .collect();
            run_hook_with_filter(
                &ctx,
                user_config,
                project_config,
                hook_type,
                &extra_vars,
                HookFailureStrategy::FailFast,
                name_filter,
            )
        }
        HookType::PreMerge => {
            // pre-merge, post-merge, pre-remove use functions from merge.rs
            // which already handle user hooks (approval already happened at gate)
            // Use current branch as target (matches approval prompt for wt hook)
            let project_cfg = project_config.unwrap_or_default();
            run_pre_merge_commands(&project_cfg, &ctx, ctx.branch_or_head(), name_filter)
        }
        HookType::PostMerge => {
            // Use current branch as target (matches approval prompt for wt hook)
            execute_post_merge_commands(&ctx, ctx.branch_or_head(), name_filter)
        }
        HookType::PreRemove => execute_pre_remove_commands(&ctx, name_filter),
    }
}

/// Handle `wt step commit` command
pub fn step_commit(
    force: bool,
    no_verify: bool,
    stage_mode: super::commit::StageMode,
    show_prompt: bool,
) -> anyhow::Result<()> {
    use super::command_approval::approve_hooks;

    // Handle --show-prompt early: just build and output the prompt
    if show_prompt {
        let config = WorktrunkConfig::load().context("Failed to load config")?;
        let prompt = crate::llm::build_commit_prompt(&config.commit_generation)?;
        crate::output::data(prompt)?;
        return Ok(());
    }

    let env = CommandEnv::for_action("commit")?;
    let ctx = env.context(force);

    // "Approve at the Gate": approve pre-commit hooks upfront (unless --no-verify)
    // Shadow no_verify: if user declines approval, skip hooks but continue commit
    let no_verify = if !no_verify {
        let approved = approve_hooks(&ctx, &[HookType::PreCommit])?;
        if !approved {
            crate::output::print(worktrunk::styling::info_message(
                "Commands declined, committing without hooks",
            ))?;
            true // Skip hooks
        } else {
            false // Run hooks
        }
    } else {
        true // --no-verify was passed
    };

    let mut options = CommitOptions::new(&ctx);
    options.no_verify = no_verify;
    options.stage_mode = stage_mode;
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
/// * `skip_pre_commit` - If true, skip all pre-commit hooks (both user and project)
/// * `stage_mode` - What to stage before committing (All or Tracked; None not supported for squash)
pub fn handle_squash(
    target: Option<&str>,
    force: bool,
    skip_pre_commit: bool,
    stage_mode: super::commit::StageMode,
) -> anyhow::Result<SquashResult> {
    use super::commit::StageMode;

    let env = CommandEnv::for_action("squash")?;
    let repo = &env.repo;
    // Squash requires being on a branch (can't squash in detached HEAD)
    let current_branch = env.require_branch("squash")?.to_string();
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

    // Run pre-commit hooks unless explicitly skipped
    let project_config = repo.load_project_config()?;
    let has_project_pre_commit = project_config
        .as_ref()
        .map(|c| c.pre_commit.is_some())
        .unwrap_or(false);
    let has_user_pre_commit = ctx.config.pre_commit.is_some();
    let has_any_pre_commit = has_project_pre_commit || has_user_pre_commit;

    if skip_pre_commit && has_any_pre_commit {
        crate::output::print(info_message(cformat!(
            "Skipping pre-commit hooks (<bright-black>--no-verify</>)"
        )))?;
    }

    let pipeline = HookPipeline::new(ctx);
    let extra_vars = [("target", target_branch.as_str())];

    // Run user pre-commit hooks first (unless skipped)
    if !skip_pre_commit && let Some(user_config) = &ctx.config.pre_commit {
        use super::hooks::{HookFailureStrategy, HookSource};
        pipeline
            .run_sequential(
                user_config,
                HookType::PreCommit,
                HookSource::User,
                &extra_vars,
                HookFailureStrategy::FailFast,
                None,
            )
            .map_err(worktrunk::git::add_hook_skip_hint)?;
    }

    // Then run project pre-commit hooks (unless skipped)
    if !skip_pre_commit && let Some(ref config) = project_config {
        pipeline
            .run_pre_commit(config, Some(&target_branch), None)
            .map_err(worktrunk::git::add_hook_skip_hint)?;
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
    crate::output::print(progress_message(squash_progress))?;

    // Create safety backup before potentially destructive reset if there are working tree changes
    if has_staged {
        let backup_message = format!("{} → {} (squash)", current_branch, target_branch);
        let (sha, _restore_cmd) = repo.create_safety_backup(&backup_message)?;
        crate::output::print(hint_message(format!("Backup created @ {sha}")))?;
    }

    // Get commit subjects for the squash message
    let subjects = repo.commit_subjects(&range)?;

    // Generate squash commit message
    crate::output::print(progress_message("Generating squash commit message..."))?;

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
        crate::output::print(info_message(format!(
            "No changes after squashing {commit_count} {commit_text}"
        )))?;
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
    crate::output::print(success_message(cformat!(
        "Squashed @ <dim>{commit_hash}</>"
    )))?;

    Ok(SquashResult::Squashed)
}

/// Handle `wt step squash --show-prompt`
///
/// Builds and outputs the squash prompt without running the LLM or squashing.
pub fn step_show_squash_prompt(
    target: Option<&str>,
    config: &worktrunk::config::CommitGenerationConfig,
) -> anyhow::Result<()> {
    let repo = Repository::current();

    // Get target branch (default to default branch if not provided)
    let target_branch = repo.resolve_target_branch(target)?;

    // Get current branch
    let current_branch = repo.current_branch()?.unwrap_or_else(|| "HEAD".to_string());

    // Get merge base with target branch
    let merge_base = repo.merge_base("HEAD", &target_branch)?;

    // Get commit subjects for the squash message
    let range = format!("{}..HEAD", merge_base);
    let subjects = repo.commit_subjects(&range)?;

    // Get repo name from directory
    let repo_root = repo.worktree_root()?;
    let repo_name = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo");

    let prompt = crate::llm::build_squash_prompt(
        &target_branch,
        &merge_base,
        &subjects,
        &current_branch,
        repo_name,
        config,
    )?;
    crate::output::data(prompt)?;
    Ok(())
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
        crate::output::print(progress_message(cformat!(
            "Rebasing onto <bold>{target_branch}</>..."
        )))?;
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
        crate::output::print(success_message(cformat!(
            "Fast-forwarded to <bold>{target_branch}</>"
        )))?;
    } else {
        crate::output::print(success_message(cformat!(
            "Rebased onto <bold>{target_branch}</>"
        )))?;
    }

    Ok(RebaseResult::Rebased)
}

/// Handle `wt hook approvals add` command - approve all commands in the project
pub fn add_approvals(show_all: bool) -> anyhow::Result<()> {
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
        crate::output::print(info_message("No commands configured in project"))?;
        return Ok(());
    }

    // Filter to only unapproved commands (unless --all is specified)
    let commands_to_approve = if !show_all {
        let unapproved: Vec<_> = commands
            .into_iter()
            .filter(|cmd| !config.is_command_approved(&project_id, &cmd.template))
            .collect();

        if unapproved.is_empty() {
            crate::output::print(info_message("All commands already approved"))?;
            return Ok(());
        }

        unapproved
    } else {
        commands
    };

    // Call the approval prompt (force=false to require interactive approval and save)
    // When show_all=true, we've already included all commands in commands_to_approve
    // When show_all=false, we've already filtered to unapproved commands
    // So we pass skip_approval_filter=true to prevent double-filtering
    let approved = approve_command_batch(&commands_to_approve, &project_id, &config, false, true)?;

    // Show result
    if approved {
        crate::output::print(success_message("Commands approved & saved to config"))?;
    } else {
        crate::output::print(info_message("Commands declined"))?;
    }

    Ok(())
}

/// Handle `wt hook approvals clear` command - clear approved commands
pub fn clear_approvals(global: bool) -> anyhow::Result<()> {
    use worktrunk::config::WorktrunkConfig;

    let mut config = WorktrunkConfig::load().context("Failed to load config")?;

    if global {
        // Clear all approvals for all projects
        let project_count = config.projects.len();

        if project_count == 0 {
            crate::output::print(info_message("No approvals to clear"))?;
            return Ok(());
        }

        config.projects.clear();
        config.save().context("Failed to save config")?;

        crate::output::print(success_message(format!(
            "Cleared approvals for {project_count} project{}",
            if project_count == 1 { "" } else { "s" }
        )))?;
    } else {
        // Clear approvals for current project (default)
        let repo = Repository::current();
        let project_id = repo.project_identifier()?;

        // Check if project has any approvals
        let had_approvals = config.projects.contains_key(&project_id);

        if !had_approvals {
            crate::output::print(info_message("No approvals to clear for this project"))?;
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

        crate::output::print(success_message(format!(
            "Cleared {approval_count} approval{} for this project",
            if approval_count == 1 { "" } else { "s" }
        )))?;
    }

    Ok(())
}

/// Handle `wt hook show` command - display configured hooks
pub fn handle_hook_show(hook_type_filter: Option<&str>, expanded: bool) -> anyhow::Result<()> {
    use crate::help_pager::show_help_in_pager;

    let repo = Repository::current();
    let config = WorktrunkConfig::load().context("Failed to load user config")?;
    let project_config = repo.load_project_config()?;
    let project_id = repo.project_identifier().ok();

    // Parse hook type filter if provided
    let filter: Option<HookType> = hook_type_filter.map(|s| match s {
        "post-create" => HookType::PostCreate,
        "post-start" => HookType::PostStart,
        "pre-commit" => HookType::PreCommit,
        "pre-merge" => HookType::PreMerge,
        "post-merge" => HookType::PostMerge,
        "pre-remove" => HookType::PreRemove,
        _ => unreachable!("clap validates hook type"),
    });

    // Build context for template expansion (only used if --expanded)
    // Need to keep CommandEnv alive for the lifetime of ctx
    // Uses branchless mode - template expansion uses "HEAD" in detached HEAD state
    let env = if expanded {
        Some(CommandEnv::for_action_branchless()?)
    } else {
        None
    };
    let ctx = env.as_ref().map(|e| e.context(false));

    let mut output = String::new();

    // Render user hooks
    render_user_hooks(&mut output, &config, filter, ctx.as_ref())?;
    output.push('\n');

    // Render project hooks
    render_project_hooks(
        &mut output,
        &repo,
        project_config.as_ref(),
        &config,
        project_id.as_deref(),
        filter,
        ctx.as_ref(),
    )?;

    // Display through pager (fall back to stderr if pager unavailable)
    if show_help_in_pager(&output).is_err() {
        worktrunk::styling::eprintln!("{}", output);
    }

    Ok(())
}

/// Render user hooks section
fn render_user_hooks(
    out: &mut String,
    config: &WorktrunkConfig,
    filter: Option<HookType>,
    ctx: Option<&CommandContext>,
) -> anyhow::Result<()> {
    let config_path = worktrunk::config::get_config_path();

    writeln!(
        out,
        "{}",
        format_heading(
            "USER HOOKS",
            Some(
                &config_path
                    .as_ref()
                    .map(|p| format_path_for_display(p))
                    .unwrap_or_else(|| "(not found)".to_string())
            )
        )
    )?;

    // Collect all user hooks
    let hooks = [
        (HookType::PostCreate, &config.post_create),
        (HookType::PostStart, &config.post_start),
        (HookType::PreCommit, &config.pre_commit),
        (HookType::PreMerge, &config.pre_merge),
        (HookType::PostMerge, &config.post_merge),
        (HookType::PreRemove, &config.pre_remove),
    ];

    let mut has_any = false;
    for (hook_type, hook_config) in hooks {
        // Apply filter if specified
        if let Some(f) = filter
            && f != hook_type
        {
            continue;
        }

        if let Some(cfg) = hook_config {
            has_any = true;
            render_hook_commands(out, hook_type, cfg, None, ctx)?;
        }
    }

    if !has_any {
        writeln!(out, "{}", hint_message("(none configured)"))?;
    }

    Ok(())
}

/// Render project hooks section
fn render_project_hooks(
    out: &mut String,
    repo: &Repository,
    project_config: Option<&ProjectConfig>,
    user_config: &WorktrunkConfig,
    project_id: Option<&str>,
    filter: Option<HookType>,
    ctx: Option<&CommandContext>,
) -> anyhow::Result<()> {
    let repo_root = repo.worktree_root()?;
    let config_path = repo_root.join(".config").join("wt.toml");

    writeln!(
        out,
        "{}",
        format_heading(
            "PROJECT HOOKS",
            Some(&format_path_for_display(&config_path))
        )
    )?;

    let Some(config) = project_config else {
        writeln!(out, "{}", hint_message("(not found)"))?;
        return Ok(());
    };

    // Collect all project hooks
    let hooks = [
        (HookType::PostCreate, &config.post_create),
        (HookType::PostStart, &config.post_start),
        (HookType::PreCommit, &config.pre_commit),
        (HookType::PreMerge, &config.pre_merge),
        (HookType::PostMerge, &config.post_merge),
        (HookType::PreRemove, &config.pre_remove),
    ];

    let mut has_any = false;
    for (hook_type, hook_config) in hooks {
        // Apply filter if specified
        if let Some(f) = filter
            && f != hook_type
        {
            continue;
        }

        if let Some(cfg) = hook_config {
            has_any = true;
            render_hook_commands(out, hook_type, cfg, Some((user_config, project_id)), ctx)?;
        }
    }

    if !has_any {
        writeln!(out, "{}", hint_message("(none configured)"))?;
    }

    Ok(())
}

/// Render commands for a single hook type
fn render_hook_commands(
    out: &mut String,
    hook_type: HookType,
    config: &CommandConfig,
    // For project hooks: (user_config, project_id) to check approval status
    approval_context: Option<(&WorktrunkConfig, Option<&str>)>,
    ctx: Option<&CommandContext>,
) -> anyhow::Result<()> {
    let commands = config.commands();
    if commands.is_empty() {
        return Ok(());
    }

    for cmd in commands {
        // Build label: "hook-type name:" or "hook-type:"
        let label = match &cmd.name {
            Some(name) => cformat!("{hook_type} <bold>{name}</>:"),
            None => format!("{hook_type}:"),
        };

        // Check approval status for project hooks
        let needs_approval = if let Some((user_config, Some(project_id))) = approval_context {
            !user_config.is_command_approved(project_id, &cmd.template)
        } else {
            false
        };

        // Use ❓ for needs approval, ⚪ for approved/user hooks
        let (emoji, suffix) = if needs_approval {
            (PROMPT_EMOJI, cformat!(" <dim>(requires approval)</>"))
        } else {
            (INFO_EMOJI, String::new())
        };

        writeln!(out, "{emoji} {label}{suffix}")?;

        // Show template or expanded command
        let command_text = if let Some(command_ctx) = ctx {
            // Expand template with current context
            expand_command_template(&cmd.template, command_ctx, hook_type)
        } else {
            cmd.template.clone()
        };

        write!(out, "{}", format_bash_with_gutter(&command_text, ""))?;
    }

    Ok(())
}

/// Expand a command template with context variables
fn expand_command_template(template: &str, ctx: &CommandContext, hook_type: HookType) -> String {
    use super::command_executor::build_hook_context;

    // Build extra vars based on hook type (same logic as run_hook approval)
    let default_branch = ctx.repo.default_branch().ok();
    let extra_vars: Vec<(&str, &str)> = match hook_type {
        HookType::PreCommit => {
            // Pre-commit uses default branch as target (for comparison context)
            default_branch
                .as_deref()
                .into_iter()
                .map(|t| ("target", t))
                .collect()
        }
        HookType::PreMerge | HookType::PostMerge => {
            // Pre-merge and post-merge use current branch as target
            vec![("target", ctx.branch_or_head())]
        }
        _ => Vec::new(),
    };
    let template_ctx = build_hook_context(ctx, &extra_vars);
    let vars: std::collections::HashMap<&str, &str> = template_ctx
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    // Use the standard template expansion (shell-escaped)
    worktrunk::config::expand_template(template, &vars, true)
        .unwrap_or_else(|_| template.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_squash_result_variants() {
        // Test Debug implementation
        let result = SquashResult::Squashed;
        let debug = format!("{:?}", result);
        assert!(debug.contains("Squashed"));

        let result = SquashResult::NoCommitsAhead("main".to_string());
        let debug = format!("{:?}", result);
        assert!(debug.contains("NoCommitsAhead"));
        assert!(debug.contains("main"));

        let result = SquashResult::AlreadySingleCommit;
        let debug = format!("{:?}", result);
        assert!(debug.contains("AlreadySingleCommit"));

        let result = SquashResult::NoNetChanges;
        let debug = format!("{:?}", result);
        assert!(debug.contains("NoNetChanges"));
    }

    #[test]
    fn test_squash_result_clone() {
        let original = SquashResult::NoCommitsAhead("develop".to_string());
        let cloned = original.clone();
        assert!(matches!(cloned, SquashResult::NoCommitsAhead(ref s) if s == "develop"));
    }

    #[test]
    fn test_rebase_result_variants() {
        // RebaseResult doesn't derive Debug/Clone by default, just test matching
        let result = RebaseResult::Rebased;
        assert!(matches!(result, RebaseResult::Rebased));

        let result = RebaseResult::UpToDate("main".to_string());
        assert!(matches!(result, RebaseResult::UpToDate(ref s) if s == "main"));
    }

    #[test]
    fn test_rebase_result_up_to_date_branch_extraction() {
        let result = RebaseResult::UpToDate("feature-branch".to_string());
        if let RebaseResult::UpToDate(branch) = result {
            assert_eq!(branch, "feature-branch");
        } else {
            panic!("Expected UpToDate variant");
        }
    }
}
