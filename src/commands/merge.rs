use worktrunk::HookType;
use worktrunk::config::{Command, CommandPhase, ProjectConfig};
use worktrunk::git::{GitError, Repository};
use worktrunk::styling::{CYAN, CYAN_BOLD, ERROR, ERROR_EMOJI, GREEN_BOLD, HINT, HINT_EMOJI};

use super::command_approval::approve_command_batch;
use super::command_executor::CommandContext;
use super::commit::{CommitOptions, commit_changes};
use super::context::CommandEnv;
use super::hooks::{HookFailureStrategy, HookPipeline};
use super::project_config::{ProjectConfigRepoExt, collect_commands_for_hooks};
use super::worktree::{RemoveResult, handle_push};

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
    fn collect(self) -> Result<CollectedCommands, GitError> {
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

        if !self.no_commit && self.repo.is_dirty()? {
            hooks.push(HookType::PreCommit);
        }

        if !self.no_verify {
            hooks.push(HookType::PreMerge);
        }

        hooks.push(HookType::PostMerge);

        all_commands.extend(collect_commands_for_hooks(&project_config, &hooks));

        let project_id = self.repo.project_identifier()?;
        Ok((all_commands, project_id))
    }
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
    let env = CommandEnv::current()?;
    let repo = &env.repo;
    let config = &env.config;
    let current_branch = env.branch.clone();

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

    // When current == target, force --no-remove (can't remove the worktree we're on)
    let no_remove_effective = no_remove || current_branch == target_branch;

    // Collect and approve all commands upfront for batch permission request
    let (all_commands, project_id) = MergeCommandCollector {
        repo,
        no_commit,
        no_verify,
    }
    .collect()?;

    // Approve all commands in a single batch
    // Commands collected here are not yet expanded - expansion happens later in prepare_project_commands
    approve_command_batch(&all_commands, &project_id, config, force, false)?;

    // Handle uncommitted changes (skip if --no-commit) - track whether commit occurred
    let committed = if !no_commit && repo.is_dirty()? {
        if squash_enabled {
            false // Squash path handles staging and committing
        } else {
            let ctx = env.context(force);
            let mut options = CommitOptions::new(&ctx);
            options.target_branch = Some(&target_branch);
            options.no_verify = no_verify;
            options.tracked_only = tracked_only;
            options.auto_trust = true;
            options.warn_about_untracked = !tracked_only;
            options.show_no_squash_note = true;

            commit_changes(options)?;
            true // Committed directly
        }
    } else {
        false // No dirty changes or --no-commit
    };

    // Squash commits if enabled - track whether squashing occurred
    let squashed = if squash_enabled {
        super::standalone::handle_squash(
            Some(&target_branch),
            force,
            no_verify,
            true,
            tracked_only,
            !tracked_only,
        )?
    } else {
        false
    };

    // Rebase onto target (skip if --no-commit) - track whether rebasing occurred
    let rebased = if !no_commit {
        super::standalone::handle_rebase(Some(&target_branch))?
    } else {
        false
    };

    // Run pre-merge checks unless --no-verify was specified
    // Do this after commit/squash/rebase to validate the final state that will be pushed
    if !no_verify && let Some(project_config) = repo.load_project_config()? {
        let ctx = env.context(force);
        run_pre_merge_commands(&project_config, &ctx, &target_branch)?;
    }

    // Fast-forward push to target branch with commit/squash/rebase info for consolidated message
    handle_push(
        Some(&target_branch),
        false,
        "Merged to",
        Some(committed),
        Some(squashed),
        Some(rebased),
    )?;

    // Get primary worktree path before cleanup (while we can still run git commands)
    let worktrees = repo.list_worktrees()?;
    let primary_worktree_dir = worktrees.worktrees[0].path.clone();

    // Finish worktree unless --no-remove was specified
    if !no_remove_effective {
        // STEP 1: Check for uncommitted changes before attempting cleanup
        // This prevents showing "Cleaning up worktree..." before failing
        repo.ensure_clean_working_tree()?;

        // STEP 2: Switch to target branch in primary worktree (fails safely if there's an issue)
        let primary_repo = Repository::at(&primary_worktree_dir);
        let new_branch = primary_repo.current_branch()?;
        if new_branch.as_deref() != Some(&target_branch) {
            crate::output::progress(format!(
                "{CYAN}Switching to {CYAN_BOLD}{target_branch}{CYAN_BOLD:#}{CYAN}...{CYAN:#}"
            ))?;
            if let Err(err) = primary_repo.run_command(&["switch", &target_branch]) {
                return Err(match err {
                    GitError::CommandFailed(msg) => GitError::SwitchFailed {
                        branch: target_branch.clone(),
                        error: msg,
                    },
                    other => other,
                });
            }
        }

        // STEP 3: Remove worktree via shared remove output handler so final message matches wt remove
        crate::output::progress(format!("{CYAN}Removing worktree & branch...{CYAN:#}"))?;
        let worktree_root = repo.worktree_root()?;
        let remove_result = RemoveResult::RemovedWorktree {
            primary_path: primary_worktree_dir.clone(),
            worktree_path: worktree_root,
            changed_directory: true,
            branch_name: current_branch.clone(),
            no_delete_branch: false,
        };
        crate::output::handle_remove_output(&remove_result, Some(&current_branch), true)?;
    } else {
        // Print comprehensive summary (worktree preserved)
        handle_merge_summary_output(None)?;
    }

    // Execute post-merge commands in the main worktree
    // This runs after cleanup so the context is clear to the user
    // Create a fresh Repository instance at the primary worktree (the old repo may be invalid)
    let primary_repo = Repository::at(&primary_worktree_dir);
    let primary_repo_root = primary_worktree_dir.clone();
    let ctx = CommandContext::new(
        &primary_repo,
        config,
        &current_branch,
        &primary_worktree_dir,
        &primary_repo_root,
        force,
    );
    execute_post_merge_commands(&ctx, &target_branch)?;

    Ok(())
}

/// Format the merge summary message (no emoji - output system adds it)
fn format_merge_summary(primary_path: Option<&std::path::Path>) -> String {
    use worktrunk::styling::GREEN;

    // Show where we ended up
    if let Some(path) = primary_path {
        format!(
            "{GREEN}Returned to primary at {GREEN_BOLD}{}{GREEN_BOLD:#}{GREEN:#}",
            path.display()
        )
    } else {
        format!("{GREEN}Worktree preserved (--no-remove){GREEN:#}")
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
/// Run pre-merge commands sequentially (blocking, fail-fast)
pub fn run_pre_merge_commands(
    project_config: &ProjectConfig,
    ctx: &CommandContext,
    target_branch: &str,
) -> Result<(), GitError> {
    let Some(pre_merge_config) = &project_config.pre_merge_command else {
        return Ok(());
    };

    let pipeline = HookPipeline::new(*ctx);

    pipeline.run_sequential(
        pre_merge_config,
        CommandPhase::PreMerge,
        true, // auto_trust: commands already approved in batch
        &[("target", target_branch)],
        "pre-merge",
        HookFailureStrategy::FailFast {
            hook_type: HookType::PreMerge,
        },
    )
}

/// Execute post-merge commands sequentially in the main worktree (blocking)
pub fn execute_post_merge_commands(
    ctx: &CommandContext,
    target_branch: &str,
) -> Result<(), GitError> {
    // Load project config from the main worktree path directly
    let project_config = match ctx.repo.load_project_config()? {
        Some(cfg) => cfg,
        None => return Ok(()),
    };

    let Some(post_merge_config) = &project_config.post_merge_command else {
        return Ok(());
    };

    let pipeline = HookPipeline::new(*ctx);

    pipeline.run_sequential(
        post_merge_config,
        CommandPhase::PostMerge,
        true, // auto_trust: commands already approved in batch
        &[("target", target_branch)],
        "post-merge",
        HookFailureStrategy::Warn,
    )
}
