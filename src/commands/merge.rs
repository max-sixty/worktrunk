use std::path::Path;

use worktrunk::HookType;
use worktrunk::config::ProjectConfig;
use worktrunk::git::Repository;
use worktrunk::styling::info_message;

use super::command_approval::approve_command_batch;
use super::command_executor::CommandContext;
use super::commit::CommitOptions;
use super::context::CommandEnv;
use super::hooks::{HookFailureStrategy, run_hook_with_filter};
use super::project_config::{HookCommand, collect_commands_for_hooks};
use super::repository_ext::RepositoryCliExt;
use super::worktree::{BranchDeletionMode, MergeOperations, RemoveResult, handle_push};

/// Options for the merge command
pub struct MergeOptions<'a> {
    pub target: Option<&'a str>,
    pub squash: bool,
    pub commit: bool,
    pub rebase: bool,
    pub remove: bool,
    pub verify: bool,
    pub yes: bool,
    pub stage_mode: super::commit::StageMode,
}

/// Collect all commands that will be executed during merge.
///
/// Returns (commands, project_identifier) for batch approval.
fn collect_merge_commands(
    repo: &Repository,
    commit: bool,
    verify: bool,
    will_remove: bool,
) -> anyhow::Result<(Vec<HookCommand>, String)> {
    let mut all_commands = Vec::new();
    let project_config = match repo.load_project_config()? {
        Some(cfg) => cfg,
        None => return Ok((all_commands, repo.project_identifier()?.to_string())),
    };

    // Collect pre-commit commands if we'll commit (direct or via squash)
    let mut hooks = Vec::new();

    if commit && verify && repo.is_dirty()? {
        hooks.push(HookType::PreCommit);
    }

    if verify {
        hooks.push(HookType::PreMerge);
        hooks.push(HookType::PostMerge);
        if will_remove {
            hooks.push(HookType::PreRemove);
            hooks.push(HookType::PostSwitch);
        }
    }

    all_commands.extend(collect_commands_for_hooks(&project_config, &hooks));

    let project_id = repo.project_identifier()?.to_string();
    Ok((all_commands, project_id))
}

pub fn handle_merge(opts: MergeOptions<'_>) -> anyhow::Result<()> {
    let MergeOptions {
        target,
        squash,
        commit,
        rebase,
        remove,
        verify,
        yes,
        stage_mode,
    } = opts;

    let env = CommandEnv::for_action("merge")?;
    let repo = &env.repo;
    let config = &env.config;
    // Merge requires being on a branch (can't merge from detached HEAD)
    let current_branch = env.require_branch("merge")?.to_string();

    // Validate --no-commit: requires clean working tree
    if !commit && repo.is_dirty()? {
        return Err(worktrunk::git::GitError::UncommittedChanges {
            action: Some("merge with --no-commit".into()),
            branch: Some(current_branch.clone()),
        }
        .into());
    }

    // --no-commit implies --no-squash
    let squash_enabled = squash && commit;

    // Get target branch (default to default branch if not provided)
    let target_branch = repo.resolve_target_branch(target)?;
    // Worktree for target is optional: if present we use it for safety checks and as destination.
    let target_worktree_path = repo.worktree_for_branch(&target_branch)?;

    // When current == target or we're in the main worktree, disable remove (can't remove it)
    let in_main = !repo.is_in_worktree().unwrap_or(false);
    let on_target = current_branch == target_branch;
    let remove_effective = remove && !on_target && !in_main;

    // Collect and approve all commands upfront for batch permission request
    let (all_commands, project_id) =
        collect_merge_commands(repo, commit, verify, remove_effective)?;

    // Approve all commands in a single batch (shows templates, not expanded values)
    let approved = approve_command_batch(&all_commands, &project_id, config, yes, false)?;

    // If commands were declined, skip hooks but continue with merge
    // Shadow verify to gate all subsequent hook execution on approval
    let verify = if !approved {
        crate::output::print(info_message("Commands declined, continuing merge"))?;
        false
    } else {
        verify
    };

    // Handle uncommitted changes (skip if --no-commit) - track whether commit occurred
    let committed = if commit && repo.is_dirty()? {
        if squash_enabled {
            false // Squash path handles staging and committing
        } else {
            let ctx = env.context(yes);
            let mut options = CommitOptions::new(&ctx);
            options.target_branch = Some(&target_branch);
            options.no_verify = !verify;
            options.stage_mode = stage_mode;
            options.warn_about_untracked = stage_mode == super::commit::StageMode::All;
            options.show_no_squash_note = true;

            options.commit()?;
            true // Committed directly
        }
    } else {
        false // No dirty changes or --no-commit
    };

    // Squash commits if enabled - track whether squashing occurred
    let squashed = if squash_enabled {
        matches!(
            super::step_commands::handle_squash(
                Some(&target_branch),
                yes,
                !verify, // skip_pre_commit when !verify
                stage_mode
            )?,
            super::step_commands::SquashResult::Squashed
        )
    } else {
        false
    };

    // Rebase onto target - track whether rebasing occurred
    let rebased = if rebase {
        // Auto-rebase onto target
        matches!(
            super::step_commands::handle_rebase(Some(&target_branch))?,
            super::step_commands::RebaseResult::Rebased
        )
    } else {
        // --no-rebase: verify already rebased, fail if not
        if !repo.is_rebased_onto(&target_branch)? {
            return Err(worktrunk::git::GitError::NotRebased {
                target_branch: target_branch.clone(),
            }
            .into());
        }
        false // Already rebased, no rebase occurred
    };

    // Run pre-merge checks unless --no-verify was specified
    // Do this after commit/squash/rebase to validate the final state that will be pushed
    if verify {
        let ctx = env.context(yes);
        let project_config = repo.load_project_config()?.unwrap_or_default();
        run_pre_merge_commands(&project_config, &ctx, &target_branch, None)?;
    }

    // Fast-forward push to target branch with commit/squash/rebase info for consolidated message
    handle_push(
        Some(&target_branch),
        "Merged to",
        Some(MergeOperations {
            committed,
            squashed,
            rebased,
        }),
    )?;

    // Destination: prefer the target branch's worktree; fall back to home path.
    let destination_path = match target_worktree_path {
        Some(path) => path,
        None => repo.home_path()?,
    };

    // Finish worktree unless --no-remove was specified
    if remove_effective {
        // STEP 1: Check for uncommitted changes before attempting cleanup
        // This prevents showing "Cleaning up worktree..." before failing
        repo.ensure_clean_working_tree("remove worktree after merge", Some(&current_branch))?;

        // STEP 2: Remove worktree via shared remove output handler so final message matches wt remove
        let worktree_root = repo.worktree_root()?.to_path_buf();
        // After a successful merge, compute integration reason from main_path
        let main_repo = worktrunk::git::Repository::at(&destination_path);
        let effective_target = main_repo.effective_integration_target(&target_branch);
        let mut provider =
            worktrunk::git::LazyGitIntegration::new(&main_repo, &current_branch, &effective_target);
        let integration_reason = worktrunk::git::check_integration(&mut provider);
        let remove_result = RemoveResult::RemovedWorktree {
            main_path: destination_path.clone(),
            worktree_path: worktree_root,
            changed_directory: true,
            branch_name: Some(current_branch.clone()),
            deletion_mode: BranchDeletionMode::SafeDelete,
            target_branch: Some(target_branch.clone()),
            integration_reason,
            // Don't force removal - if worktree has untracked files added after
            // commit, removal will fail and user can run `wt remove --force`
            force_worktree: false,
        };
        // Run hooks during merge removal (pass through verify flag)
        // Approval was handled at the gate (collect_merge_commands)
        crate::output::handle_remove_output(&remove_result, true, verify)?;
    } else {
        // Worktree preserved - show reason (priority: main worktree > on target > --no-remove flag)
        let message = if in_main {
            "Worktree preserved (main worktree)"
        } else if on_target {
            "Worktree preserved (already on target branch)"
        } else {
            "Worktree preserved (--no-remove)"
        };
        crate::output::print(info_message(message))?;
        crate::output::flush()?;
    }

    if verify {
        // Execute post-merge commands in the destination worktree
        // This runs after cleanup so the context is clear to the user
        // Create a fresh Repository instance at the destination (the old repo may be invalid)
        let destination_repo = Repository::at(&destination_path);
        let destination_repo_root = destination_path.clone();
        let ctx = CommandContext::new(
            &destination_repo,
            config,
            Some(&current_branch),
            &destination_path,
            &destination_repo_root,
            yes,
        );
        // Show path when user's shell won't be in the destination directory where hooks run.
        let display_path = if in_main || on_target {
            // User is already in the destination directory
            None
        } else if !remove_effective {
            // Worktree preserved — user's shell stays in feature worktree
            Some(destination_path.as_path())
        } else {
            // Worktree removed — show path only if shell integration won't cd there
            crate::output::hooks_display_path(&destination_path)
        };
        execute_post_merge_commands(&ctx, &target_branch, None, display_path)?;
    }

    Ok(())
}

/// Run pre-merge commands sequentially (blocking, fail-fast)
///
/// Runs user hooks first, then project hooks.
/// Approval is handled at the gate (command entry point).
pub fn run_pre_merge_commands(
    project_config: &ProjectConfig,
    ctx: &CommandContext,
    target_branch: &str,
    name_filter: Option<&str>,
) -> anyhow::Result<()> {
    run_hook_with_filter(
        ctx,
        ctx.config.hooks.pre_merge.as_ref(),
        project_config.hooks.pre_merge.as_ref(),
        HookType::PreMerge,
        &[("target", target_branch)],
        HookFailureStrategy::FailFast,
        name_filter,
        None, // No path display - running in expected directory
    )
    .map_err(worktrunk::git::add_hook_skip_hint)
}

/// Execute post-merge commands sequentially in the target worktree (blocking)
///
/// Runs user hooks first, then project hooks.
/// Approval is handled at the gate (command entry point).
///
/// `display_path`: When `Some`, shows the path in hook announcements. Pass this when
/// the worktree is preserved and the user's shell remains in a different directory.
pub fn execute_post_merge_commands(
    ctx: &CommandContext,
    target_branch: &str,
    name_filter: Option<&str>,
    display_path: Option<&Path>,
) -> anyhow::Result<()> {
    // Load project config from the main worktree path
    let project_config = ctx.repo.load_project_config()?;

    run_hook_with_filter(
        ctx,
        ctx.config.hooks.post_merge.as_ref(),
        project_config
            .as_ref()
            .and_then(|c| c.hooks.post_merge.as_ref()),
        HookType::PostMerge,
        &[("target", target_branch)],
        HookFailureStrategy::Warn,
        name_filter,
        display_path,
    )
    .map_err(worktrunk::git::add_hook_skip_hint)
}

/// Execute pre-remove commands sequentially in the worktree (blocking)
///
/// Runs user hooks first, then project hooks.
/// Runs before a worktree is removed. Non-zero exit aborts the removal.
/// Approval is handled at the gate (command entry point).
pub fn execute_pre_remove_commands(
    ctx: &CommandContext,
    name_filter: Option<&str>,
) -> anyhow::Result<()> {
    let project_config = ctx.repo.load_project_config()?;

    run_hook_with_filter(
        ctx,
        ctx.config.hooks.pre_remove.as_ref(),
        project_config
            .as_ref()
            .and_then(|c| c.hooks.pre_remove.as_ref()),
        HookType::PreRemove,
        &[],
        HookFailureStrategy::FailFast,
        name_filter,
        None, // No path display - running in expected directory
    )
    .map_err(worktrunk::git::add_hook_skip_hint)
}
