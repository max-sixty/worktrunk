//! Post-merge "finish" sequence: capture identity, decide on removal, register
//! the post-merge hook.
//!
//! Extracted from `handle_merge` so the merge command's body stays focused on
//! the merge ref-update itself. Three steps run in strict order:
//!
//! 1. Capture the feature worktree's path + commit BEFORE removal — afterward
//!    the worktree directory is gone, but post-merge hooks still need to
//!    reference it via Active template overrides.
//! 2. Apply the removal disposition frozen before the target ref update.
//!    `--no-remove`, on-target, primary-worktree, and a pre-existing lock retain
//!    the worktree. Otherwise the approved `pre-remove` hook runs exactly once;
//!    a lock it creates also retains the worktree. The default-branch and final
//!    cleanliness gates then run before `handle_remove_output_after_pre_remove`
//!    performs removal through the same path as `wt remove`.
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
use worktrunk::utils::escape_text_for_terminal;

use super::types::{RemovalPlan, SharedBranchCheckout};
use crate::commands::command_executor::CommandContext;
use crate::commands::context::CommandEnv;
use crate::commands::hook_plan::{ApprovedHookPlan, register_planned};
use crate::commands::hooks::HookAnnouncer;
use crate::commands::repository_ext::{
    check_not_default_branch, compute_integration_reason, is_primary_worktree,
    live_sibling_checkout,
};
use crate::commands::template_vars::TemplateVars;
use crate::output::{
    BackgroundFallbackMode, RemovalExecution, execute_pre_remove_hook,
    handle_remove_output_after_pre_remove, post_hook_display_path, pre_hook_display_path,
};

/// Inputs to [`finish_after_merge`]. Owned by the caller; this struct just
/// bundles them so the function signature stays readable.
pub struct FinishAfterMergeArgs<'a> {
    pub current_branch: &'a str,
    pub target_branch: &'a str,
    pub target_worktree_path: Option<&'a Path>,
    pub remove: bool,
    pub removal_disposition: MergeRemovalDisposition,
    pub verify: bool,
    pub yes: bool,
    /// The frozen, approved hook plan. `post-merge` and the removal's
    /// `pre-remove` / `post-remove` / `post-switch` execute only from this.
    pub plan: &'a ApprovedHookPlan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MergeRemovalDisposition {
    Disabled,
    OnTarget,
    PrimaryWorktree,
    Locked(Option<String>),
    Remove,
}

/// Resolve whether merge cleanup would remove the source worktree right now.
pub(crate) fn merge_removal_disposition(
    repo: &Repository,
    current_branch: &str,
    target_branch: &str,
    remove: bool,
) -> anyhow::Result<MergeRemovalDisposition> {
    if !remove {
        Ok(MergeRemovalDisposition::Disabled)
    } else if current_branch == target_branch {
        Ok(MergeRemovalDisposition::OnTarget)
    } else if is_primary_worktree(repo)? {
        Ok(MergeRemovalDisposition::PrimaryWorktree)
    } else if let Some(reason) = repo.current_worktree().lock_reason()? {
        Ok(MergeRemovalDisposition::Locked(reason))
    } else {
        Ok(MergeRemovalDisposition::Remove)
    }
}

/// Fail before updating the target ref if cleanup would remove a dirty source.
pub(crate) fn ensure_merge_removal_is_clean(
    repo: &Repository,
    current_branch: &str,
    removal_disposition: &MergeRemovalDisposition,
) -> anyhow::Result<()> {
    if *removal_disposition == MergeRemovalDisposition::Remove {
        check_not_default_branch(repo, current_branch, &BranchDeletionMode::SafeDelete)?;
        repo.current_worktree().ensure_clean(
            "remove worktree after merge",
            Some(current_branch),
            false,
        )?;
    }

    Ok(())
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
        removal_disposition,
        verify,
        yes,
        plan,
    } = args;

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
    let preserve_message = match removal_disposition {
        MergeRemovalDisposition::Disabled => Some("Worktree preserved (--no-remove)".into()),
        MergeRemovalDisposition::OnTarget => {
            Some("Worktree preserved (already on target branch)".into())
        }
        MergeRemovalDisposition::PrimaryWorktree => {
            Some("Worktree preserved (primary worktree)".into())
        }
        // The pre-update decision is frozen: unlocking after the target ref
        // moves must not upgrade a retained worktree into removal.
        MergeRemovalDisposition::Locked(Some(reason)) => Some(format!(
            "Worktree preserved (locked: {})",
            escape_text_for_terminal(&reason)
        )),
        MergeRemovalDisposition::Locked(None) => Some("Worktree preserved (locked)".into()),
        MergeRemovalDisposition::Remove => None,
    };

    let removed = if let Some(message) = preserve_message {
        eprintln!("{}", info_message(message));
        false
    } else {
        'removal: {
            let current_wt = repo.current_worktree();
            let worktree_root = current_wt.root()?;

            execute_pre_remove_hook(
                &destination_path,
                &worktree_root,
                true,
                Some(current_branch),
                plan,
            )?;

            // A pre-remove hook may lock the worktree to keep it. Honor that
            // request before emitting a cd directive or starting removal.
            if let Some(reason) = current_wt.lock_reason()? {
                let message = match reason {
                    Some(reason) => format!(
                        "Worktree preserved (locked: {})",
                        escape_text_for_terminal(&reason)
                    ),
                    None => "Worktree preserved (locked)".into(),
                };
                eprintln!("{}", info_message(message));
                break 'removal false;
            }

            // Phase 3: reject removing default branch (merge always uses SafeDelete).
            check_not_default_branch(repo, current_branch, &BranchDeletionMode::SafeDelete)?;

            current_wt.ensure_clean("remove worktree after merge", Some(current_branch), false)?;

            // Merge reaches the same ref deletion `wt remove` does, so it asks the
            // same question: is this branch checked out anywhere else? Merging is
            // the likeliest way to meet a `--force` duplicate — the branch is
            // integrated, so nothing else would stop the delete, and deleting it
            // strands the duplicate at a null OID.
            let branch_checked_out_at =
                live_sibling_checkout(repo.list_worktrees()?, current_branch, &worktree_root).map(
                    |sibling| {
                        SharedBranchCheckout::new(&sibling.path, &BranchDeletionMode::SafeDelete)
                    },
                );

            // A retained branch has no deletion to justify, so the integration
            // check is skipped rather than computed and discarded — same shape as
            // `prepare_worktree_removal`'s Phase 5.
            let (deletion_mode, display_target, integration_reason) =
                if branch_checked_out_at.is_some() {
                    (BranchDeletionMode::Keep, None, None)
                } else {
                    let (integration_reason, effective_target) = compute_integration_reason(
                        repo,
                        &repo.capture_refs()?,
                        Some(current_branch),
                        Some(target_branch),
                        BranchDeletionMode::SafeDelete,
                    );
                    (
                        BranchDeletionMode::SafeDelete,
                        effective_target.or_else(|| Some(target_branch.to_string())),
                        integration_reason,
                    )
                };

            // No config snapshot: `pre-remove` / `post-remove` were selected and
            // frozen into `plan` at the gate (anchored at `feature_path`), so the
            // executor needs no config — it runs only the frozen `plan`.
            let remove_result = RemovalPlan::Worktree {
                main_path: destination_path.clone(),
                worktree_path: worktree_root,
                changed_directory: true,
                branch_name: Some(current_branch.to_string()),
                deletion_mode,
                target_branch: display_target,
                integration_reason,
                force_worktree: false,
                removed_commit: feature_commit.clone(),
                branch_checked_out_at,
            };
            // Merge's `removed` flag means the removal path started; a
            // hook-created lock is the one successful preservation outcome.
            // Branch fate remains narrated by the shared handler.
            let removed = handle_remove_output_after_pre_remove(
                &remove_result,
                RemovalExecution::Background(BackgroundFallbackMode::Detached),
                plan,
                false,
                announcer,
            )?
            .removal_started();
            break 'removal removed;
        }
    };

    if verify {
        // Post-merge hooks run in the destination worktree (target). `ctx.repo`
        // is rooted there only for template *rendering* (the feature worktree
        // may be gone); the command set is the frozen `plan`, anchored at
        // `destination_path` at the gate — no re-read of the destination's
        // (now post-merge) `.config/wt.toml`.
        let dest_repo = Repository::at(&destination_path)?;
        let ctx = CommandContext::new(
            &dest_repo,
            config,
            Some(current_branch),
            &destination_path,
            yes,
        );
        let display_path = if removed {
            post_hook_display_path(&destination_path)
        } else {
            pre_hook_display_path(&destination_path)
        };

        let mut vars = feature_vars.with_target(target_branch);
        if let Some(p) = target_worktree_path {
            vars = vars.with_target_worktree_path(p);
        }
        register_planned(
            announcer,
            plan,
            &destination_path,
            &ctx,
            HookType::PostMerge,
            &vars.as_extra_vars(),
            display_path,
        )?;
    }

    Ok(removed)
}
