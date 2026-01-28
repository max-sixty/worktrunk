use anyhow::Context;
use worktrunk::HookType;
use worktrunk::config::UserConfig;
use worktrunk::git::Repository;
use worktrunk::styling::{eprintln, info_message};

use super::command_approval::approve_command_batch;
use super::command_executor::CommandContext;
use super::commit::CommitOptions;
use super::context::CommandEnv;
use super::hooks::{HookFailureStrategy, execute_hook};
use super::project_config::{HookCommand, collect_commands_for_hooks};
use super::repository_ext::RepositoryCliExt;
use super::worktree::{
    BranchDeletionMode, MergeOperations, RemoveResult, get_path_mismatch, handle_push,
};

/// Options for the merge command
///
/// All boolean fields are optional CLI overrides. If None, the effective config
/// (project-specific merged with global) is used. If that's also None, defaults apply.
pub struct MergeOptions<'a> {
    pub target: Option<&'a str>,
    /// CLI override for squash. None = use effective config default.
    pub squash: Option<bool>,
    /// CLI override for commit. None = use effective config default.
    pub commit: Option<bool>,
    /// CLI override for rebase. None = use effective config default.
    pub rebase: Option<bool>,
    /// CLI override for remove. None = use effective config default.
    pub remove: Option<bool>,
    /// CLI override for verify. None = use effective config default.
    pub verify: Option<bool>,
    pub yes: bool,
    /// CLI override for stage mode. None = use effective config default.
    pub stage: Option<super::commit::StageMode>,
}

/// Collect all commands that will be executed during merge.
///
/// Returns (commands, project_identifier) for batch approval.
fn collect_merge_commands(
    repo: &Repository,
    commit: bool,
    verify: bool,
    will_remove: bool,
    squash_enabled: bool,
) -> anyhow::Result<(Vec<HookCommand>, String)> {
    let mut all_commands = Vec::new();
    let project_config = match repo.load_project_config()? {
        Some(cfg) => cfg,
        None => return Ok((all_commands, repo.project_identifier()?)),
    };

    let mut hooks = Vec::new();

    // Pre-commit hooks run when a commit will actually be created
    let will_create_commit = repo.current_worktree().is_dirty()? || squash_enabled;
    if commit && verify && will_create_commit {
        hooks.push(HookType::PreCommit);
    }

    if verify {
        hooks.push(HookType::PreMerge);
        hooks.push(HookType::PostMerge);
        if will_remove {
            hooks.push(HookType::PreRemove);
            hooks.push(HookType::PostRemove);
            hooks.push(HookType::PostSwitch);
        }
    }

    all_commands.extend(collect_commands_for_hooks(&project_config, &hooks));

    let project_id = repo.project_identifier()?;
    Ok((all_commands, project_id))
}

pub fn handle_merge(opts: MergeOptions<'_>) -> anyhow::Result<()> {
    let MergeOptions {
        target,
        squash: squash_opt,
        commit: commit_opt,
        rebase: rebase_opt,
        remove: remove_opt,
        verify: verify_opt,
        yes,
        stage,
    } = opts;

    // Load config once, run LLM setup prompt if committing, then reuse config
    let mut config = UserConfig::load().context("Failed to load config")?;
    if commit_opt.unwrap_or(true) {
        // One-time LLM setup prompt (errors logged internally; don't block merge)
        let _ = crate::output::prompt_commit_generation(&mut config);
    }

    let env = CommandEnv::for_action("merge", config)?;
    let repo = &env.repo;
    let config = &env.config;
    // Merge requires being on a branch (can't merge from detached HEAD)
    let current_branch = env.require_branch("merge")?.to_string();

    // Get effective merge config (project-specific merged with global)
    let merge_config = env.merge();

    // Determine final values: CLI > project config > global config > default (true)
    let squash = squash_opt
        .or_else(|| merge_config.as_ref().and_then(|m| m.squash))
        .unwrap_or(true);
    let commit = commit_opt
        .or_else(|| merge_config.as_ref().and_then(|m| m.commit))
        .unwrap_or(true);
    let rebase = rebase_opt
        .or_else(|| merge_config.as_ref().and_then(|m| m.rebase))
        .unwrap_or(true);
    let remove = remove_opt
        .or_else(|| merge_config.as_ref().and_then(|m| m.remove))
        .unwrap_or(true);
    let verify = verify_opt
        .or_else(|| merge_config.as_ref().and_then(|m| m.verify))
        .unwrap_or(true);

    // Stage mode: CLI > project commit config > global commit config > default
    let stage_mode = stage
        .or_else(|| env.commit().and_then(|c| c.stage))
        .unwrap_or_default();

    // Cache current worktree for multiple queries
    let current_wt = repo.current_worktree();

    // Validate --no-commit: requires clean working tree
    if !commit && current_wt.is_dirty()? {
        return Err(worktrunk::git::GitError::UncommittedChanges {
            action: Some("merge with --no-commit".into()),
            branch: Some(current_branch),
            force_hint: false,
        }
        .into());
    }

    // --no-commit implies --no-squash
    let squash_enabled = squash && commit;

    // Get and validate target branch (must be a branch since we're updating it)
    let target_branch = repo.require_target_branch(target)?;
    // Worktree for target is optional: if present we use it for safety checks and as destination.
    let target_worktree_path = repo.worktree_for_branch(&target_branch)?;

    // When current == target or we're in the main worktree, disable remove (can't remove it)
    let in_main = !current_wt.is_linked().unwrap_or(false);
    let on_target = current_branch == target_branch;
    let remove_effective = remove && !on_target && !in_main;

    // Collect and approve all commands upfront for batch permission request
    let (all_commands, project_id) =
        collect_merge_commands(repo, commit, verify, remove_effective, squash_enabled)?;

    // Approve all commands in a single batch (shows templates, not expanded values)
    let approved = approve_command_batch(&all_commands, &project_id, config, yes, false)?;

    // If commands were declined, skip hooks but continue with merge
    // Shadow verify to gate all subsequent hook execution on approval
    let verify = if !approved {
        eprintln!("{}", info_message("Commands declined, continuing merge"));
        false
    } else {
        verify
    };

    // Handle uncommitted changes (skip if --no-commit) - track whether commit occurred
    let committed = if commit && current_wt.is_dirty()? {
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
                Some(stage_mode)
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
            return Err(worktrunk::git::GitError::NotRebased { target_branch }.into());
        }
        false // Already rebased, no rebase occurred
    };

    // Run pre-merge checks unless --no-verify was specified
    // Do this after commit/squash/rebase to validate the final state that will be pushed
    if verify {
        let ctx = env.context(yes);
        execute_hook(
            &ctx,
            HookType::PreMerge,
            &[("target", target_branch.as_str())],
            HookFailureStrategy::FailFast,
            None,
            crate::output::pre_hook_display_path(ctx.worktree_path),
        )?;
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
        current_wt.ensure_clean("remove worktree after merge", Some(&current_branch), false)?;

        // STEP 2: Remove worktree via shared remove output handler so final message matches wt remove
        let worktree_root = current_wt.root()?;
        // After a successful merge, get integration reason
        let (_, integration_reason) = repo.integration_reason(&current_branch, &target_branch)?;
        // Compute expected_path for path mismatch detection
        let expected_path = get_path_mismatch(repo, &current_branch, &worktree_root, config);
        // Capture commit SHA before removal for post-remove hook template variables
        let removed_commit = current_wt
            .run_command(&["rev-parse", "HEAD"])
            .ok()
            .map(|s| s.trim().to_string());
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
            expected_path,
            removed_commit,
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
        eprintln!("{}", info_message(message));
    }

    if verify {
        // Execute post-merge commands in the destination worktree
        // This runs after cleanup so the context is clear to the user
        let ctx = CommandContext::new(repo, config, Some(&current_branch), &destination_path, yes);
        // Show path when user's shell won't be in the destination directory where hooks run.
        let display_path = if remove_effective && !in_main && !on_target {
            // Worktree removed, user will cd to destination
            crate::output::post_hook_display_path(&destination_path)
        } else {
            // No cd happens — user stays at cwd (either already at destination,
            // or worktree preserved so they stay in feature)
            crate::output::pre_hook_display_path(&destination_path)
        };
        execute_hook(
            &ctx,
            HookType::PostMerge,
            &[("target", target_branch.as_str())],
            HookFailureStrategy::Warn,
            None,
            display_path,
        )?;
    }

    Ok(())
}
