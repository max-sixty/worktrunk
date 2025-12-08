use worktrunk::HookType;
use worktrunk::config::{Command, CommandPhase, ProjectConfig};
use worktrunk::git::Repository;

use super::command_approval::approve_command_batch;
use super::command_executor::CommandContext;
use super::commit::CommitOptions;
use super::context::CommandEnv;
use super::hooks::{HookFailureStrategy, HookPipeline};
use super::project_config::collect_commands_for_hooks;
use super::repository_ext::RepositoryCliExt;
use super::worktree::{MergeOperations, RemoveResult, handle_push};

/// Reason why a worktree was preserved (not removed) after merge
enum PreserveReason {
    /// User explicitly passed --no-remove
    NoRemoveFlag,
    /// Running from the main worktree (can't remove main worktree)
    IsMainWorktree,
    /// Current branch is the same as the target branch
    AlreadyOnTarget,
}

/// Context for collecting merge commands
struct MergeCommandCollector<'a> {
    repo: &'a Repository,
    no_commit: bool,
    no_verify: bool,
}

/// Commands collected for batch approval with their project identifier
/// - `Vec<Command>`: Commands with both template and (initial) expanded forms
/// - `String`: Project identifier for config lookup
type CollectedCommands = (Vec<Command>, String);

impl<'a> MergeCommandCollector<'a> {
    /// Collect all commands that will be executed during merge
    ///
    /// Returns original (unexpanded) commands for approval matching
    fn collect(self) -> anyhow::Result<CollectedCommands> {
        let mut all_commands = Vec::new();
        let project_config = match self.repo.load_project_config()? {
            Some(cfg) => cfg,
            None => return Ok((all_commands, self.repo.project_identifier()?)),
        };

        // Collect original commands (not expanded) for approval
        // Expansion happens later in prepare_project_commands during execution

        // Collect pre-commit commands if we'll commit (direct or via squash)
        // These run before: (1) direct commit (line 179), or (2) squash commit (line 194 → handle_dev_squash)
        let mut hooks = Vec::new();

        if !self.no_commit && !self.no_verify && self.repo.is_dirty()? {
            hooks.push(HookType::PreCommit);
        }

        if !self.no_verify {
            hooks.push(HookType::PreMerge);
            hooks.push(HookType::PostMerge);
        }

        all_commands.extend(collect_commands_for_hooks(&project_config, &hooks));

        let project_id = self.repo.project_identifier()?;
        Ok((all_commands, project_id))
    }
}

pub fn handle_merge(
    target: Option<&str>,
    squash: bool,
    commit: bool,
    remove: bool,
    verify: bool,
    force: bool,
    stage_mode: super::commit::StageMode,
) -> anyhow::Result<()> {
    let env = CommandEnv::for_action("merge")?;
    let repo = &env.repo;
    let config = &env.config;
    let current_branch = env.branch.clone();

    // Validate --no-commit: requires clean working tree
    if !commit && repo.is_dirty()? {
        return Err(worktrunk::git::GitError::UncommittedChanges {
            action: Some("merge with --no-commit".into()),
            worktree: Some(current_branch.clone()),
        }
        .into());
    }

    // --no-commit implies --no-squash
    let squash_enabled = squash && commit;

    // Get target branch (default to default branch if not provided)
    let target_branch = repo.resolve_target_branch(target)?;
    let worktrees = repo.list_worktrees()?;
    // Worktree for target is optional: if present we use it for safety checks and as destination.
    let target_worktree_path = repo.worktree_for_branch(&target_branch)?;

    // When current == target or we're in the main worktree, disable remove (can't remove it)
    let in_main = !repo.is_in_worktree().unwrap_or(false);
    let on_target = current_branch == target_branch;
    let remove_effective = remove && !on_target && !in_main;

    // Collect and approve all commands upfront for batch permission request
    let (all_commands, project_id) = MergeCommandCollector {
        repo,
        no_commit: !commit,
        no_verify: !verify,
    }
    .collect()?;

    // Approve all commands in a single batch
    // Commands collected here are not yet expanded - expansion happens later in prepare_project_commands
    let approved = approve_command_batch(&all_commands, &project_id, config, force, false)?;

    // If commands were declined, show message explaining that merge continues anyway
    if !approved {
        crate::output::info("Commands declined, continuing merge")?;
    }

    // Handle uncommitted changes (skip if --no-commit) - track whether commit occurred
    let committed = if commit && repo.is_dirty()? {
        if squash_enabled {
            false // Squash path handles staging and committing
        } else {
            let ctx = env.context(force);
            let mut options = CommitOptions::new(&ctx);
            options.target_branch = Some(&target_branch);
            options.no_verify = !verify;
            options.stage_mode = stage_mode;
            options.auto_trust = true;
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
            super::standalone::handle_squash(
                Some(&target_branch),
                force,
                !verify,
                true,
                stage_mode
            )?,
            super::standalone::SquashResult::Squashed
        )
    } else {
        false
    };

    // Rebase onto target (skip if --no-commit) - track whether rebasing occurred
    let rebased = if commit {
        matches!(
            super::standalone::handle_rebase(Some(&target_branch))?,
            super::standalone::RebaseResult::Rebased
        )
    } else {
        false
    };

    // Run pre-merge checks unless --no-verify was specified
    // Do this after commit/squash/rebase to validate the final state that will be pushed
    if verify && let Some(project_config) = repo.load_project_config()? {
        let ctx = env.context(force);
        run_pre_merge_commands(&project_config, &ctx, &target_branch, true)?;
    }

    // Fast-forward push to target branch with commit/squash/rebase info for consolidated message
    handle_push(
        Some(&target_branch),
        false,
        "Merged to",
        Some(MergeOperations {
            committed,
            squashed,
            rebased,
        }),
    )?;

    // Destination: prefer the target branch's worktree; fall back to main when absent
    let destination_path = target_worktree_path
        .clone()
        .unwrap_or_else(|| worktrees.main().path.clone());

    // Finish worktree unless --no-remove was specified
    if remove_effective {
        // STEP 1: Check for uncommitted changes before attempting cleanup
        // This prevents showing "Cleaning up worktree..." before failing
        repo.ensure_clean_working_tree(Some("remove worktree after merge"), Some(&current_branch))?;

        // STEP 2: Remove worktree via shared remove output handler so final message matches wt remove
        let worktree_root = repo.worktree_root()?;
        let remove_result = RemoveResult::RemovedWorktree {
            main_path: destination_path.clone(),
            worktree_path: worktree_root,
            changed_directory: true,
            branch_name: Some(current_branch.clone()),
            no_delete_branch: false,
            force_delete: false,
            target_branch: Some(target_branch.clone()),
        };
        // Run hooks during merge removal (verify=true)
        crate::output::handle_remove_output(&remove_result, Some(&current_branch), true, true)?;
    } else {
        // Print comprehensive summary (worktree preserved)
        // Priority: main worktree > on target > --no-remove flag
        let reason = if in_main {
            PreserveReason::IsMainWorktree
        } else if on_target {
            PreserveReason::AlreadyOnTarget
        } else {
            PreserveReason::NoRemoveFlag
        };
        handle_merge_summary_output(reason)?;
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
            &current_branch,
            &destination_path,
            &destination_repo_root,
            force,
        );
        execute_post_merge_commands(&ctx, &target_branch, true)?;
    }

    Ok(())
}

/// Handle output for merge summary using global output context
fn handle_merge_summary_output(reason: PreserveReason) -> anyhow::Result<()> {
    let message = match reason {
        PreserveReason::IsMainWorktree => "Worktree preserved (main worktree)",
        PreserveReason::AlreadyOnTarget => "Worktree preserved (already on target branch)",
        PreserveReason::NoRemoveFlag => "Worktree preserved (--no-remove)",
    };
    crate::output::info(message)?;
    crate::output::flush()?;

    Ok(())
}
/// Run pre-merge commands sequentially (blocking, fail-fast)
///
/// # Security Note
/// `auto_trust`: When true, skip approval prompts (commands already approved in `wt merge`'s batch).
///              When false, require approval (standalone `wt step hook` execution).
pub fn run_pre_merge_commands(
    project_config: &ProjectConfig,
    ctx: &CommandContext,
    target_branch: &str,
    auto_trust: bool,
) -> anyhow::Result<()> {
    let Some(pre_merge_config) = &project_config.pre_merge else {
        return Ok(());
    };

    let pipeline = HookPipeline::new(*ctx);

    pipeline.run_sequential(
        pre_merge_config,
        CommandPhase::PreMerge,
        auto_trust,
        &[("target", target_branch)],
        "pre-merge",
        HookType::PreMerge,
        HookFailureStrategy::FailFast,
    )
}

/// Execute post-merge commands sequentially in the main worktree (blocking)
///
/// # Security Note
/// `auto_trust`: When true, skip approval prompts (commands already approved in `wt merge`'s batch).
///              When false, require approval (standalone `wt step hook` execution).
pub fn execute_post_merge_commands(
    ctx: &CommandContext,
    target_branch: &str,
    auto_trust: bool,
) -> anyhow::Result<()> {
    // Load project config from the main worktree path directly
    let project_config = match ctx.repo.load_project_config()? {
        Some(cfg) => cfg,
        None => return Ok(()),
    };

    let Some(post_merge_config) = &project_config.post_merge else {
        return Ok(());
    };

    let pipeline = HookPipeline::new(*ctx);

    pipeline.run_sequential(
        post_merge_config,
        CommandPhase::PostMerge,
        auto_trust,
        &[("target", target_branch)],
        "post-merge",
        HookType::PostMerge,
        HookFailureStrategy::Warn,
    )
}

/// Execute pre-remove commands sequentially in the worktree (blocking)
///
/// Runs before a worktree is removed. Non-zero exit aborts the removal.
///
/// # Arguments
/// * `ctx` - Command context pointing to the worktree being removed
/// * `auto_trust` - When true, skip approval prompts
pub fn execute_pre_remove_commands(ctx: &CommandContext, auto_trust: bool) -> anyhow::Result<()> {
    let project_config = match ctx.repo.load_project_config()? {
        Some(cfg) => cfg,
        None => return Ok(()),
    };

    let Some(pre_remove_config) = &project_config.pre_remove else {
        return Ok(());
    };

    let pipeline = HookPipeline::new(*ctx);

    pipeline.run_sequential(
        pre_remove_config,
        CommandPhase::PreRemove,
        auto_trust,
        &[],
        "pre-remove",
        HookType::PreRemove,
        HookFailureStrategy::FailFast,
    )
}
