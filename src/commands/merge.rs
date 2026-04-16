use anyhow::Context;
use worktrunk::HookType;
use worktrunk::config::{Approvals, UserConfig};
use worktrunk::git::Repository;
use worktrunk::styling::{eprintln, info_message};

use super::command_approval::approve_command_batch;
use super::command_executor::CommandContext;
use super::command_executor::FailureStrategy;
use super::commit::CommitOptions;
use super::context::CommandEnv;
use super::hooks::{execute_hook, prepare_background_hooks, spawn_hook_pipeline};
use super::project_config::{ApprovableCommand, collect_commands_for_hooks};
use super::repository_ext::{
    RepositoryCliExt, check_not_default_branch, compute_integration_reason, is_primary_worktree,
};
use super::worktree::{
    MergeOperations, RemoveResult, handle_no_ff_merge, handle_push, path_mismatch,
};
use worktrunk::git::BranchDeletionMode;

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
    /// CLI override for ff. None = use effective config default.
    pub ff: Option<bool>,
    /// CLI override for verify. None = use effective config default.
    pub verify: Option<bool>,
    pub yes: bool,
    /// CLI override for stage mode. None = use effective config default.
    pub stage: Option<super::commit::StageMode>,
    /// Output format (text or json).
    pub format: crate::cli::SwitchFormat,
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
) -> anyhow::Result<(Vec<ApprovableCommand>, String)> {
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
        hooks.push(HookType::PostCommit);
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
    let json_mode = opts.format == crate::cli::SwitchFormat::Json;
    let MergeOptions {
        target,
        squash: squash_opt,
        commit: commit_opt,
        rebase: rebase_opt,
        remove: remove_opt,
        ff: ff_opt,
        verify: verify_opt,
        yes,
        stage,
        ..
    } = opts;

    // Load config once, run LLM setup prompt if committing, then reuse config
    let mut config = UserConfig::load().context("Failed to load config")?;
    if commit_opt.unwrap_or(true) {
        // One-time LLM setup prompt (errors logged internally; don't block merge)
        let _ = crate::output::prompt_commit_generation(&mut config);
    }

    let env = CommandEnv::for_action(config)?;
    let repo = &env.repo;
    let config = &env.config;
    // Merge requires being on a branch (can't merge from detached HEAD)
    let current_branch = env.require_branch("merge")?.to_string();

    // Get effective settings (project-specific merged with global, defaults applied)
    let resolved = env.resolved();

    // CLI flags override config values
    let squash = squash_opt.unwrap_or(resolved.merge.squash());
    let commit = commit_opt.unwrap_or(resolved.merge.commit());
    let rebase = rebase_opt.unwrap_or(resolved.merge.rebase());
    let remove = remove_opt.unwrap_or(resolved.merge.remove());
    let ff = ff_opt.unwrap_or(resolved.merge.ff());
    let verify = verify_opt.unwrap_or(resolved.merge.verify());
    let stage_mode = stage.unwrap_or(resolved.commit.stage());

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

    // Quick check for command approval: will removal be attempted?
    // The authoritative guard is prepare_merge_removal (shared with wt remove),
    // but we need a lightweight answer here to decide whether to include
    // pre-remove/post-remove hooks in the batch approval prompt.
    let on_target = current_branch == target_branch;
    let remove_requested = remove && !on_target;

    // Collect and approve all commands upfront for batch permission request
    let (all_commands, project_id) =
        collect_merge_commands(repo, commit, verify, remove_requested, squash_enabled)?;

    // Approve all commands in a single batch (shows templates, not expanded values)
    let approvals = Approvals::load().context("Failed to load approvals")?;
    let approved = approve_command_batch(&all_commands, &project_id, &approvals, yes, false)?;

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
            options.verify = verify;
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
                verify,
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

    // Target worktree path for template variables (pre-merge and post-merge hooks).
    // Computed once here so both hook sites can reference it.
    let target_wt_path_str = target_worktree_path
        .as_deref()
        .map(|p| worktrunk::path::to_posix_path(&p.to_string_lossy()));

    // Run pre-merge checks unless --no-hooks was specified
    // Do this after commit/squash/rebase to validate the final state that will be pushed
    if verify {
        let ctx = env.context(yes);
        let mut extra: Vec<(&str, &str)> = vec![("target", target_branch.as_str())];
        if let Some(ref p) = target_wt_path_str {
            extra.push(("target_worktree_path", p));
        }
        execute_hook(
            &ctx,
            HookType::PreMerge,
            &extra,
            FailureStrategy::FailFast,
            &[],
            crate::output::pre_hook_display_path(ctx.worktree_path),
        )?;
    }

    // Merge to target branch
    let operations = Some(MergeOperations {
        committed,
        squashed,
        rebased,
    });
    if !ff {
        // Create a merge commit on the target branch via commit-tree + update-ref
        handle_no_ff_merge(Some(&target_branch), operations, &current_branch)?;
    } else {
        // Fast-forward push to target branch
        handle_push(Some(&target_branch), "Merged to", operations)?;
    }

    // Destination: prefer the target branch's worktree; fall back to home path.
    let destination_path = match target_worktree_path {
        Some(path) => path,
        None => repo.home_path()?,
    };

    // Capture feature worktree identity BEFORE removal for post-merge template vars.
    // After removal the feature worktree is gone, but post-merge hooks need to
    // reference it as the Active identity (branch, worktree_path, commit).
    let feature_path_str = worktrunk::path::to_posix_path(&env.worktree_path.to_string_lossy());
    let feature_name = env
        .worktree_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();
    let feature_commit = repo
        .current_worktree()
        .run_command(&["rev-parse", "HEAD"])
        .ok()
        .map(|s| s.trim().to_string());
    let feature_short_commit = feature_commit
        .as_ref()
        .filter(|c| c.len() >= 7)
        .map(|c| c[..7].to_string());

    // Finish worktree unless removal is disabled or blocked.
    // Guards are shared with `wt remove`: is_primary_worktree (Phase 2) and
    // check_not_default_branch (Phase 3) are the same helpers both paths use.
    let removed = if !remove {
        eprintln!("{}", info_message("Worktree preserved (--no-remove)"));
        false
    } else if on_target {
        eprintln!(
            "{}",
            info_message("Worktree preserved (already on target branch)")
        );
        false
    } else if is_primary_worktree(repo)? {
        eprintln!("{}", info_message("Worktree preserved (primary worktree)"));
        false
    } else {
        // Phase 3: reject removing default branch (merge always uses SafeDelete).
        check_not_default_branch(repo, &current_branch, &BranchDeletionMode::SafeDelete)?;

        let current_wt = repo.current_worktree();
        current_wt.ensure_clean("remove worktree after merge", Some(&current_branch), false)?;

        let worktree_root = current_wt.root()?;
        let (integration_reason, _) = compute_integration_reason(
            repo,
            Some(&current_branch),
            Some(&target_branch),
            BranchDeletionMode::SafeDelete,
        );
        let expected_path = path_mismatch(repo, &current_branch, &worktree_root, config);

        let remove_result = RemoveResult::RemovedWorktree {
            main_path: destination_path.clone(),
            worktree_path: worktree_root,
            changed_directory: true,
            branch_name: Some(current_branch.to_string()),
            deletion_mode: BranchDeletionMode::SafeDelete,
            target_branch: Some(target_branch.to_string()),
            integration_reason,
            force_worktree: false,
            expected_path,
            removed_commit: feature_commit.clone(),
        };
        crate::output::handle_remove_output(&remove_result, false, verify, false, false)?;
        true
    };

    if verify {
        // Post-merge hooks run in the destination worktree (target), but bare vars
        // point to the Active (feature branch) per the template variable model.
        // The destination worktree is the execution context (cwd).
        let ctx = CommandContext::new(repo, config, Some(&current_branch), &destination_path, yes);
        let display_path = if removed {
            crate::output::post_hook_display_path(&destination_path)
        } else {
            crate::output::pre_hook_display_path(&destination_path)
        };

        // Override bare vars to Active (feature branch identity)
        let mut extra: Vec<(&str, &str)> = vec![("target", target_branch.as_str())];
        if let Some(ref p) = target_wt_path_str {
            extra.push(("target_worktree_path", p));
        }
        // Active = feature: override worktree_path and friends
        extra.push(("worktree_path", &feature_path_str));
        extra.push(("worktree", &feature_path_str)); // deprecated alias
        extra.push(("worktree_name", &feature_name));
        if let Some(ref c) = feature_commit {
            extra.push(("commit", c));
        }
        if let Some(ref sc) = feature_short_commit {
            extra.push(("short_commit", sc));
        }

        for steps in prepare_background_hooks(&ctx, HookType::PostMerge, &extra, display_path)? {
            spawn_hook_pipeline(&ctx, steps)?;
        }
    }

    if json_mode {
        let output = serde_json::json!({
            "branch": current_branch,
            "target": target_branch,
            "committed": committed,
            "squashed": squashed,
            "rebased": rebased,
            "removed": removed,
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    }

    Ok(())
}
