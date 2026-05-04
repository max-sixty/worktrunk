//! Post-merge "finish" sequence: capture identity, decide on removal, register
//! the post-merge hook.
//!
//! Extracted from `handle_merge` so the merge command's body stays focused on
//! the merge ref-update itself. Three steps run in strict order:
//!
//! 1. Capture the feature worktree's path + commit BEFORE removal — afterward
//!    the worktree directory is gone, but post-merge hooks still need to
//!    reference it via Active template overrides.
//! 2. Decide whether to remove the feature worktree. Four conditions block
//!    removal: `--no-remove`, on-target, primary-worktree, and default-branch.
//!    Otherwise `ensure_clean` gates removal and `handle_remove_output`
//!    performs it (sharing the same code path as `wt remove`).
//! 3. Register the post-merge hook with the announcer. The caller owns
//!    `flush()` because it's a command-level lifecycle operation, not part of
//!    the finish sequence.
//!
//! The capture-before-removal ordering is enforced inside this function:
//! `feature_commit` is read via `git rev-parse HEAD` before the removal branch
//! runs, so post-merge hooks see the right SHA even after the worktree is
//! gone.

use std::path::Path;

use worktrunk::HookType;
use worktrunk::config::UserConfig;
use worktrunk::git::{BranchDeletionMode, Repository};
use worktrunk::styling::{eprintln, info_message};

use super::resolve::path_mismatch;
use super::types::RemoveResult;
use crate::commands::command_executor::CommandContext;
use crate::commands::context::CommandEnv;
use crate::commands::hooks::HookAnnouncer;
use crate::commands::repository_ext::{
    check_not_default_branch, compute_integration_reason, is_primary_worktree,
};
use crate::commands::template_vars::TemplateVars;

/// Inputs to [`finish_after_merge`]. Owned by the caller; this struct just
/// bundles them so the function signature stays readable.
pub struct FinishAfterMergeArgs<'a> {
    pub current_branch: &'a str,
    pub target_branch: &'a str,
    pub target_worktree_path: Option<&'a Path>,
    pub remove: bool,
    pub verify: bool,
    pub yes: bool,
}

/// Run the post-merge finish sequence: capture feature identity, optionally
/// remove the feature worktree, register the post-merge hook. Returns whether
/// the feature worktree was removed (the caller folds this into its
/// `--format=json` blob).
///
/// `announcer` is mutated in place; `flush()` stays with the caller because it
/// covers the whole command's background hooks (post-commit, post-remove,
/// post-switch, post-merge), not just this step.
pub fn finish_after_merge(
    repo: &Repository,
    config: &UserConfig,
    env: &CommandEnv,
    announcer: &mut HookAnnouncer<'_>,
    args: FinishAfterMergeArgs<'_>,
) -> anyhow::Result<bool> {
    let FinishAfterMergeArgs {
        current_branch,
        target_branch,
        target_worktree_path,
        remove,
        verify,
        yes,
    } = args;

    let on_target = current_branch == target_branch;

    // Destination: prefer the target branch's worktree; fall back to home path.
    let destination_path = match target_worktree_path {
        Some(path) => path.to_path_buf(),
        None => repo.home_path()?,
    };

    // Capture feature worktree identity BEFORE removal as Active overrides for
    // post-merge hooks. After removal the feature worktree is gone, but
    // post-merge hooks need to reference its branch, path, and commit. Skip the
    // subprocess when nothing reads the result (`--no-remove --no-hooks`).
    let mut feature_vars = TemplateVars::new().with_active_worktree(&env.worktree_path);
    let feature_commit = if verify || remove {
        repo.current_worktree()
            .run_command(&["rev-parse", "HEAD"])
            .ok()
            .map(|s| s.trim().to_string())
    } else {
        None
    };
    if let Some(commit) = feature_commit.as_deref() {
        let short = repo
            .short_sha(commit)
            .unwrap_or_else(|_| commit.to_string());
        feature_vars = feature_vars.with_active_commit(commit, &short);
    }

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
        check_not_default_branch(repo, current_branch, &BranchDeletionMode::SafeDelete)?;

        let current_wt = repo.current_worktree();
        current_wt.ensure_clean("remove worktree after merge", Some(current_branch), false)?;

        let worktree_root = current_wt.root()?;
        // Capture a fresh snapshot AFTER the merge has updated the local
        // target ref (`update-ref` ran inside `handle_no_ff_merge` /
        // `handle_push`). Without this, `integration_reason` would observe
        // pre-merge state and misclassify the just-merged branch as
        // unmerged — the bug class PR #2507 worked around with
        // `ref_is_ancestor`.
        let snapshot = repo.capture_refs()?;
        let (integration_reason, _) = compute_integration_reason(
            repo,
            &snapshot,
            Some(current_branch),
            Some(target_branch),
            BranchDeletionMode::SafeDelete,
        );
        let expected_path = path_mismatch(repo, current_branch, &worktree_root, config);

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
        crate::output::handle_remove_output(&remove_result, false, verify, false, announcer)?;
        true
    };

    if verify {
        // Post-merge hooks run in the destination worktree (target), but bare vars
        // point to the Active (feature branch) per the template variable model.
        // The destination worktree is the execution context (cwd).
        let ctx = CommandContext::new(repo, config, Some(current_branch), &destination_path, yes);
        let display_path = if removed {
            crate::output::post_hook_display_path(&destination_path)
        } else {
            crate::output::pre_hook_display_path(&destination_path)
        };

        let mut vars = feature_vars.with_target(target_branch);
        if let Some(p) = target_worktree_path {
            vars = vars.with_target_worktree_path(p);
        }
        announcer.register(
            &ctx,
            HookType::PostMerge,
            &vars.as_extra_vars(),
            display_path,
        )?;
    }

    Ok(removed)
}
