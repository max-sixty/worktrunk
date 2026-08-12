use std::path::{Path, PathBuf};

use super::worktree::{RemovalPlan, SharedBranchCheckout};
use anyhow::{Context, bail};
use color_print::cformat;
use worktrunk::git::{
    BranchDeletionMode, GitError, IntegrationReason, RefSnapshot, Repository, WorktreeInfo,
    parse_porcelain_z, parse_untracked_files,
};
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{eprintln, format_with_gutter, suggest_command, warning_message};

/// Target for worktree removal.
#[derive(Debug)]
pub enum RemoveTarget {
    /// Delete a branch that has no worktree.
    ///
    /// A branch names a worktree only while it has exactly one: let it name
    /// two, which `git worktree add --force` allows, and the lookup silently
    /// picks git's first-listed checkout. So callers resolve first and pass
    /// [`WorktreePath`](Self::WorktreePath) for anything that has a worktree,
    /// and this variant carries only the branch-only case. One that has since
    /// acquired a worktree lost the race and errors rather than removing it
    /// unasked.
    BranchOnly(String),
    /// Remove the exact worktree at this path (supports detached HEAD and
    /// duplicate branch checkouts).
    WorktreePath(PathBuf),
}

/// CLI-only helpers implemented on [`Repository`] via an extension trait so we can keep orphan
/// implementations inside the binary crate.
pub trait RepositoryCliExt {
    /// Warn about untracked files being auto-staged.
    fn warn_if_auto_staging_untracked(&self) -> anyhow::Result<()>;

    /// Prepare the removal of whichever worktree or branch [`RemoveTarget`]
    /// names.
    ///
    /// Returns a `RemovalPlan` describing what will be removed. The actual
    /// removal is performed by the output handler. Planning is a pure read —
    /// every mutation, including unregistering a stale worktree entry
    /// (`RemovalPlan::BranchOnly::prune_entry`), happens at execution — so
    /// callers may plan speculatively: on a `--dry-run` scan, before an
    /// approval prompt, or on the picker's event loop.
    ///
    /// `current_path` is the worktree the caller started in. Callers resolve it
    /// once at their boundary so target preparation never depends on a later
    /// process-CWD lookup (notably on picker and prune background paths).
    ///
    /// `worktrees` provides a pre-fetched worktree list to avoid redundant
    /// `git worktree list` calls. Pass `None` to fetch on demand.
    ///
    /// `snapshot` provides a pre-captured ref snapshot to avoid redundant
    /// `for-each-ref` scans when preparing many removals in a row (e.g.
    /// `step_prune` validates one candidate per loop iteration). Pass `None`
    /// to capture a fresh snapshot inside this call.
    #[allow(clippy::too_many_arguments)]
    fn prepare_worktree_removal(
        &self,
        target: RemoveTarget,
        deletion_mode: BranchDeletionMode,
        force_worktree: bool,
        current_path: &Path,
        worktrees: Option<&[WorktreeInfo]>,
        snapshot: Option<&RefSnapshot>,
    ) -> anyhow::Result<RemovalPlan>;

    /// Refuse the push when target-worktree changes overlap the push range.
    ///
    /// Uncommitted changes at paths the push range doesn't touch are left
    /// alone — the two-tree merge in `advance_target` carries them in place —
    /// so this only names the files that genuinely conflict, before anything
    /// moves.
    ///
    /// The caller has already established that `target_worktree` exists on disk
    /// (`MergeContext::prepare` refuses a registered-but-missing worktree), so
    /// the status read here is free to fail if the directory is gone.
    ///
    /// Ignored files are deliberately out of scope — they're absent from the
    /// `git status --porcelain` read this works from, and matching git there is
    /// the decision, not an oversight. The module spec in
    /// `commands/worktree/push.rs` says why.
    fn ensure_no_target_conflicts(
        &self,
        target_worktree: Option<&PathBuf>,
        target_branch: &str,
    ) -> anyhow::Result<()>;

    /// Check if HEAD is a linear extension of the target branch.
    ///
    /// Returns true when:
    /// 1. The merge-base equals target's SHA (target hasn't advanced), AND
    /// 2. There are no merge commits between target and HEAD (history is linear)
    ///
    /// This detects branches that have merged the target into themselves — such
    /// branches need rebasing to linearize history even though merge-base equals target.
    fn is_rebased_onto(&self, target: &str) -> anyhow::Result<bool>;
}

impl RepositoryCliExt for Repository {
    fn warn_if_auto_staging_untracked(&self) -> anyhow::Result<()> {
        // Use -z for NUL-separated output to handle filenames with spaces/newlines
        let status = self
            .run_command(&["status", "--porcelain", "-z"])
            .context("Failed to get status")?;
        warn_about_untracked_files(&status)
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_worktree_removal(
        &self,
        target: RemoveTarget,
        deletion_mode: BranchDeletionMode,
        force_worktree: bool,
        current_path: &Path,
        worktrees: Option<&[WorktreeInfo]>,
        snapshot: Option<&RefSnapshot>,
    ) -> anyhow::Result<RemovalPlan> {
        let worktrees = match worktrees {
            Some(wts) => wts,
            None => self.list_worktrees()?,
        };
        // Primary worktree path: prefer default branch's worktree, fall back to first
        // worktree, then repo base for bare repos with no worktrees.
        let primary_path = self.home_path()?;

        // Reuse caller's snapshot when present; otherwise capture once for
        // branch-only integration checks below.
        let owned_snapshot;
        let snapshot = match snapshot {
            Some(s) => s,
            None => {
                owned_snapshot = self.capture_refs()?;
                &owned_snapshot
            }
        };

        // Phase 1: Resolve target to branch name and worktree disposition.
        // BranchOnly variants don't early-return — they go through shared validation below.
        enum Resolved {
            Worktree {
                path: PathBuf,
                branch: Option<String>,
                is_current: bool,
            },
            BranchOnly {
                /// Path of the stale worktree entry this fell back from,
                /// carried into the plan for execution-time pruning. `None`
                /// when the branch has no worktree entry at all, which is also
                /// the only case with no sibling to check — a branch that has
                /// one resolves to `Worktree` above, or, named as a `Branch`
                /// target, errors.
                pruned_from: Option<PathBuf>,
                branch: String,
            },
        }

        let resolved = match target {
            RemoveTarget::BranchOnly(branch) => {
                // The caller established there was no worktree, so one here
                // appeared in between. Falling through would remove it — the
                // wrong operation, and on a worktree nobody named — so the
                // race surfaces instead. `wt remove <path>` is the spelling
                // that does mean "remove that worktree".
                if let Some(wt) = worktrees
                    .iter()
                    .find(|wt| wt.branch.as_deref() == Some(branch.as_str()))
                {
                    let path = format_path_for_display(&wt.path);
                    bail!(cformat!(
                        "Branch <bold>{branch}</> gained a worktree @ <bold>{path}</> since it was selected; to remove that worktree, run <bold>{}</>",
                        suggest_command("remove", &[&path], &[])
                    ));
                }
                // Check the branch exists locally, so a typo or a remote-only
                // name reports itself rather than deleting nothing.
                let branch_handle = self.branch(&branch);
                if !branch_handle.exists_locally()? {
                    let remotes = branch_handle.remotes()?;
                    if !remotes.is_empty() {
                        return Err(GitError::RemoteOnlyBranch {
                            branch,
                            remote: remotes[0].clone(),
                        }
                        .into());
                    }
                    return Err(GitError::BranchNotFound {
                        branch,
                        show_create_hint: false,
                        last_fetch_ago: None,
                        pr_mr_platform: None,
                    }
                    .into());
                }
                Resolved::BranchOnly {
                    pruned_from: None,
                    branch,
                }
            }
            RemoveTarget::WorktreePath(lookup_path) => {
                let wt = worktrees
                    .iter()
                    .find(|wt| worktrunk::path::paths_match(&wt.path, &lookup_path))
                    .ok_or_else(|| {
                        anyhow::anyhow!("Worktree not found at {}", lookup_path.display())
                    })?;
                // Lock guard first, before the missing-directory fallback. A
                // lock means "don't remove this", and a temporarily-absent
                // directory (removable media, a network mount, a dropped VPN)
                // is exactly the case `git worktree lock` exists for — so an
                // absent directory must not route a locked worktree into the
                // pruning + branch-deletion path below (#3645). `--force` does
                // not override the lock, matching `git worktree remove`.
                if wt.locked.is_some() {
                    let name = wt
                        .branch
                        .clone()
                        .unwrap_or_else(|| wt.dir_name().to_string());
                    return Err(GitError::WorktreeLocked {
                        branch: name,
                        path: wt.path.clone(),
                        reason: wt.locked.clone(),
                    }
                    .into());
                }
                // Directory missing (e.g. external `rm -rf`): fall back to
                // branch-only deletion, recording the stale entry in the plan
                // so execution unregisters it — planning stays a pure read
                // (`wt step prune`'s scan doubles as `--dry-run`, and `wt
                // remove` plans before its approval prompt). A detached
                // worktree has no branch to fall back to, so an absent
                // directory leaves it to the prunable arm below rather than
                // here.
                //
                // The recorded prune names this worktree rather than sweeping
                // the repo, so a sibling whose directory is merely absent
                // right now keeps its registration. `git worktree remove`
                // refuses a locked worktree where a repo-wide prune ignored
                // one, which needs no guard here: the lock check above already
                // returned for every locked entry in this arm.
                //
                // `exists()` is that cleanup's precondition rather than a
                // proxy for health: `prune_worktree_entry` unregisters with
                // `git worktree remove`, which skips its validation only while
                // the directory is absent.
                if let Some(branch) = wt.branch.as_deref()
                    && !wt.path.exists()
                {
                    Resolved::BranchOnly {
                        pruned_from: Some(wt.path.clone()),
                        branch: branch.to_string(),
                    }
                } else if wt.is_prunable() {
                    // Still registered, but the directory no longer holds this
                    // worktree. Two shapes reach here: one deleted and
                    // recreated, which is what an interrupted `wt switch`
                    // leaves behind; and a detached one simply deleted, which
                    // the branch-only cleanup above cannot take because it has
                    // no branch to fall back to. Neither route out of here
                    // works: that cleanup wants a branch *and* an absent
                    // directory, and for the recreated directory the removal
                    // below walks into git's own validation a few calls later,
                    // reaching the user as a raw `exit 128`. The hint names the
                    // repo-wide `git worktree prune` because it is what clears
                    // both; the detached one, whose directory is absent, a
                    // targeted `git worktree remove <path>` would also clear.
                    return Err(GitError::WorktreeMissing {
                        branch: wt
                            .branch
                            .clone()
                            .unwrap_or_else(|| wt.dir_name().to_string()),
                    }
                    .into());
                } else {
                    let is_current = worktrunk::path::paths_match(&wt.path, current_path);
                    Resolved::Worktree {
                        path: wt.path.clone(),
                        branch: wt.branch.clone(),
                        is_current,
                    }
                }
            }
        };

        // Phase 2: Main-worktree guard (before default-branch check, since
        // -D can't override the main worktree restriction).
        if let Resolved::Worktree { ref path, .. } = resolved
            && !self.worktree_at(path).is_linked()?
        {
            return Err(GitError::CannotRemoveMainWorktree.into());
        }

        // Phase 3: Branch-level validation (applies to ALL paths).
        let branch_name = match &resolved {
            Resolved::Worktree { branch, .. } => branch.as_deref(),
            Resolved::BranchOnly { branch, .. } => Some(branch.as_str()),
        };
        if let Some(branch) = branch_name {
            check_not_default_branch(self, branch, &deletion_mode)?;
        }

        // Phase 4: Return BranchOnly early (after validation), or continue to
        // worktree-level checks. Branch-only removals have no pre-remove hook,
        // so their integration decision can be computed here.
        let (worktree_path, branch_name, is_current) = match resolved {
            Resolved::BranchOnly {
                pruned_from,
                branch,
            } => {
                // The missing-directory fallback reaches the same ref deletion a
                // worktree removal does, so it needs the same guard: a sibling
                // checkout whose directory is intact would be orphaned by it.
                let shared = pruned_from.as_deref().and_then(|target| {
                    live_sibling_checkout(worktrees, &branch, target)
                        .map(|sibling| SharedBranchCheckout::new(&sibling.path, &deletion_mode))
                });
                if let Some(shared) = shared {
                    return Ok(RemovalPlan::BranchOnly {
                        branch_name: branch,
                        deletion_mode: BranchDeletionMode::Keep,
                        prune_entry: pruned_from,
                        target_branch: None,
                        integration_reason: None,
                        branch_checked_out_at: Some(shared),
                    });
                }
                let default_branch = self.default_branch();
                let target = default_branch.as_deref().or(Some("HEAD"));
                let (integration_reason, target_branch) = compute_integration_reason(
                    self,
                    snapshot,
                    Some(&branch),
                    target,
                    deletion_mode,
                );
                return Ok(RemovalPlan::BranchOnly {
                    branch_name: branch,
                    deletion_mode,
                    prune_entry: pruned_from,
                    target_branch,
                    integration_reason,
                    branch_checked_out_at: None,
                });
            }
            Resolved::Worktree {
                path,
                branch,
                is_current,
            } => (path, branch, is_current),
        };

        // Phase 5: Remaining worktree-level validation.
        let target_wt = self.worktree_at(&worktree_path);

        // Ownership first: `ensure_clean` below runs `git status` in the
        // directory, so against a foreign occupant it reports that
        // repository's dirt as this worktree's and points at `--force`, the
        // one flag that would carry the removal through. Planning is also
        // upstream of the "Removing …" announcement, so the refusal arrives
        // before wt claims to be doing it.
        //
        // `stage_worktree_removal` asks again at the rename, and that is a
        // genuine re-check rather than a repeat of this one: the gate reads the
        // directory's `.git` entry every call, so a directory swapped in
        // between — across the approval prompt and the `pre-remove` hook — is
        // caught there.
        target_wt.ensure_holds_this_worktree()?;

        if !force_worktree {
            target_wt.ensure_clean("remove worktree", branch_name.as_deref(), true)?;
        }

        // main_path: where post-remove hooks run from and background removal
        // executes. Prefer the primary worktree for stability (the removed worktree
        // is gone, and cwd may itself be a removal candidate during prune).
        // Fall back to cwd when the primary worktree IS the one being removed
        // (bare repo only — normal repos guard this in Phase 2 above).
        // changed_directory: whether the user needs to cd away from cwd.
        let changed_directory = is_current;
        let main_path = if worktree_path == primary_path {
            current_path.to_path_buf()
        } else {
            primary_path
        };

        let branch_checked_out_at = branch_name.as_deref().and_then(|branch| {
            live_sibling_checkout(worktrees, branch, &worktree_path)
                .map(|sibling| SharedBranchCheckout::new(&sibling.path, &deletion_mode))
        });

        // Resolve target branch and integration verdict for display and
        // retention prediction. The actual branch deletion re-decides against
        // fresh refs (`delete_branch_if_safe`'s CAS), so this is display-only.
        //
        // A retained shared branch skips all of it: forcing `Keep` — the single
        // chokepoint every deletion path honors — settles the outcome, so an
        // integration verdict would only be computed to be ignored, and
        // reporting one alongside a branch that survives reads as a
        // contradiction.
        let (deletion_mode, target_branch, integration_reason) = if branch_checked_out_at.is_some()
        {
            (BranchDeletionMode::Keep, None, None)
        } else {
            let default_branch = self.default_branch();
            let target_branch = match (&default_branch, &branch_name) {
                (Some(db), Some(bn)) if db == bn => None,
                _ => default_branch,
            };
            let (integration_reason, target_branch) = match compute_integration_reason(
                self,
                snapshot,
                branch_name.as_deref(),
                target_branch.as_deref(),
                deletion_mode,
            ) {
                (reason, Some(effective_target)) => (reason, Some(effective_target)),
                (reason, None) => (reason, target_branch),
            };
            (deletion_mode, target_branch, integration_reason)
        };

        // Capture commit SHA before removal for post-remove hook template variables.
        // This ensures {{ commit }} references the removed worktree's state.
        let removed_commit = target_wt
            .run_command(&["rev-parse", "HEAD"])
            .ok()
            .map(|s| s.trim().to_string());

        // No `.config/wt.toml` snapshot: `pre-remove` / `post-remove` were
        // selected and frozen into the `ApprovedHookPlan` at the gate
        // (`remove.rs`'s `approve_remove`), anchored at this worktree's path,
        // so the executor needs no config — it runs only the frozen plan.
        Ok(RemovalPlan::Worktree {
            main_path,
            worktree_path,
            changed_directory,
            branch_name,
            deletion_mode,
            target_branch,
            integration_reason,
            force_worktree,
            removed_commit,
            branch_checked_out_at,
        })
    }

    fn ensure_no_target_conflicts(
        &self,
        target_worktree: Option<&PathBuf>,
        target_branch: &str,
    ) -> anyhow::Result<()> {
        let Some(wt_path) = target_worktree else {
            return Ok(());
        };

        // `-uall` lists individual files inside untracked directories — the
        // default collapses them to a single `dir/` entry, which can never
        // match a file path in the push range — and, being explicit, it
        // overrides a user's `status.showUntrackedFiles=no`. `-z` handles
        // filenames with spaces and renames ("XY path\0" for normal files,
        // "XY new_path\0old_path\0" for renames/copies).
        let wt = self.worktree_at(wt_path);
        let wt_status_output = wt.run_command(&["status", "--porcelain", "-z", "-uall"])?;
        if wt_status_output.trim().is_empty() {
            return Ok(());
        }

        let push_files = self.changed_files(target_branch, "HEAD")?;
        let wt_files: Vec<String> = parse_porcelain_z(&wt_status_output);

        let overlapping: Vec<String> = push_files
            .iter()
            .filter(|f| wt_files.contains(f))
            .cloned()
            .collect();

        if !overlapping.is_empty() {
            return Err(GitError::ConflictingChanges {
                target_branch: target_branch.to_string(),
                files: overlapping,
                worktree_path: wt_path.clone(),
            }
            .into());
        }

        Ok(())
    }

    fn is_rebased_onto(&self, target: &str) -> anyhow::Result<bool> {
        // Orphan branches have no common ancestor, so they can't be "rebased onto" target
        let Some(merge_base) = self.merge_base("HEAD", target)? else {
            return Ok(false);
        };
        // `merge_base` peels an annotated tag to the commit it points at; a bare
        // `rev-parse` returns the tag object's own SHA. Comparing the two forms
        // never matches, so an annotated-tag target would always be reported as
        // needing a rebase — and `wt step rebase <annotated-tag>` would replay
        // nothing while announcing "Rebased onto <tag>". Peel both sides.
        let target_sha = self
            .run_command(&[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{target}^{{commit}}"),
            ])?
            .trim()
            .to_string();

        if merge_base != target_sha {
            return Ok(false); // Target has advanced past merge-base
        }

        // Check for merge commits — if present, history is not linear
        let merge_commits = self
            .run_command(&[
                "rev-list",
                "--merges",
                "--end-of-options",
                &format!("{}..HEAD", target),
            ])?
            .trim()
            .to_string();

        Ok(merge_commits.is_empty())
    }
}

/// Check if the current worktree is the primary worktree (should not be removed).
///
/// Returns true for the main worktree in normal repos and the default branch
/// worktree in bare repos. Used by `wt merge` to skip removal silently, and
/// by `prepare_worktree_removal` Phase 2 (which errors instead of skipping).
pub(crate) fn is_primary_worktree(repo: &Repository) -> anyhow::Result<bool> {
    let current_root = repo.current_worktree().root()?;
    let primary = repo.primary_worktree()?;
    Ok(primary.as_deref() == Some(current_root.as_path()))
}

/// Compute integration reason and effective target for branch deletion.
///
/// Returns `(None, None)` if:
/// - `deletion_mode` is `ForceDelete` (skip integration check)
/// - `branch_name` is `None` (detached HEAD)
/// - `target_branch` is `None` (no target to check against)
///
/// When `Some`, the effective target may differ from the local default branch
/// (e.g., `origin/main` when upstream is ahead).
///
/// Note: Integration is computed even for `Keep` mode so we can inform the user
/// if the flag had an effect (branch was integrated) or not (branch was unmerged).
pub(crate) fn compute_integration_reason(
    repo: &Repository,
    snapshot: &RefSnapshot,
    branch_name: Option<&str>,
    target_branch: Option<&str>,
    deletion_mode: BranchDeletionMode,
) -> (Option<IntegrationReason>, Option<String>) {
    // Skip for force delete (we'll delete regardless of integration status)
    // But compute for keep mode so we can inform user if the flag had no effect
    if deletion_mode.is_force() {
        return (None, None);
    }
    let (branch, target) = match branch_name.zip(target_branch) {
        Some(pair) => pair,
        None => return (None, None),
    };
    // On error, return None (informational only)
    match repo.integration_reason(snapshot, branch, target) {
        Ok((effective_target, reason)) => (reason, Some(effective_target)),
        Err(_) => (None, None),
    }
}

/// The worktree, other than the one being removed, whose checkout of `branch`
/// deleting the ref would orphan.
///
/// A branch reaches two worktrees only through `git worktree add --force`,
/// which worktrunk never runs itself. Once it has, the ref is live in both, and
/// worktrunk deletes branches with `git update-ref -d` — git's compare-and-swap
/// primitive, which unlike `git branch -d` does not refuse a ref that is
/// checked out somewhere. Deleting it leaves the other checkout at a null OID
/// with an unresolvable `HEAD`, so every removal that could delete a branch
/// asks this first.
///
/// Only a live directory counts. A sibling entry whose directory is already
/// gone is stale metadata awaiting `git worktree prune`, not a checkout with
/// anything to lose — retaining a branch for it would strand the branch and
/// point the user at a directory that isn't there.
pub(crate) fn live_sibling_checkout<'a>(
    worktrees: &'a [WorktreeInfo],
    branch: &str,
    removing: &Path,
) -> Option<&'a WorktreeInfo> {
    worktrees.iter().find(|wt| {
        wt.branch.as_deref() == Some(branch)
            && !worktrunk::path::paths_match(&wt.path, removing)
            && wt.path.exists()
    })
}

/// Reject removing the default branch unless force-delete is set.
///
/// The default branch is the integration target — checking it against itself is
/// tautological (same logic as `wt list`'s `is_main` guard in
/// `check_integration_state`).
pub(crate) fn check_not_default_branch(
    repo: &Repository,
    branch: &str,
    deletion_mode: &BranchDeletionMode,
) -> anyhow::Result<()> {
    if !deletion_mode.is_force() && repo.default_branch().as_deref() == Some(branch) {
        return Err(GitError::CannotRemoveDefaultBranch {
            branch: branch.to_string(),
        }
        .into());
    }
    Ok(())
}

/// Warn about untracked files that will be auto-staged.
pub(crate) fn warn_about_untracked_files(status_output: &str) -> anyhow::Result<()> {
    let files = parse_untracked_files(status_output);
    if files.is_empty() {
        return Ok(());
    }

    let count = files.len();
    let path_word = if count == 1 { "path" } else { "paths" };
    eprintln!(
        "{}",
        warning_message(format!("Auto-staging {count} untracked {path_word}:"))
    );

    let joined_files = files.join("\n");
    eprintln!("{}", format_with_gutter(&joined_files, None));

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use worktrunk::git::{BranchDeletionOutcome, execute_branch_deletion};
    use worktrunk::testing::TestRepo;

    /// A branch-only plan is only a snapshot of topology. If another process
    /// checks the branch out before execution, the final safe-delete guard must
    /// retain the ref so the new worktree's HEAD stays resolvable.
    #[test]
    fn branch_only_execution_rechecks_worktree_topology() {
        let test = TestRepo::with_initial_commit();
        test.create_branch("feature");
        let repo = Repository::at(test.root_path()).unwrap();
        let current_path = test.root_path().to_path_buf();

        let plan = repo
            .prepare_worktree_removal(
                RemoveTarget::BranchOnly("feature".to_string()),
                BranchDeletionMode::SafeDelete,
                false,
                &current_path,
                None,
                None,
            )
            .unwrap();
        assert!(matches!(plan, RemovalPlan::BranchOnly { .. }));

        let checkout = test.home_path().join("repo.feature-raced-checkout");
        test.run_git(&["worktree", "add", checkout.to_str().unwrap(), "feature"]);

        let result = execute_branch_deletion(&repo, "feature", "main", false).unwrap();
        let BranchDeletionOutcome::RetainedCheckedOut { path } = result.outcome else {
            panic!("safe deletion must report the checkout added after planning");
        };
        assert!(
            worktrunk::path::paths_match(&path, &checkout),
            "retention should name the checkout: {} != {}",
            path.display(),
            checkout.display()
        );
        assert!(
            repo.run_command(&["rev-parse", "--verify", "refs/heads/feature"])
                .is_ok(),
            "the checked-out branch ref must survive"
        );
        assert!(
            repo.worktree_at(&checkout)
                .run_command(&["rev-parse", "--verify", "HEAD"])
                .is_ok(),
            "the new checkout must not be orphaned"
        );
    }

    /// Duplicate checkout detection excludes the worktree being removed by
    /// canonical path identity, not by its literal spelling, and still finds a
    /// separate checkout of the same branch.
    #[test]
    fn duplicate_checkout_detection_compares_paths_canonically() {
        let mut test = TestRepo::with_initial_commit();
        let removed = test.add_worktree("feature");
        let survivor = test.home_path().join("repo.feature-survivor");
        test.run_git(&[
            "worktree",
            "add",
            "--force",
            survivor.to_str().unwrap(),
            "feature",
        ]);

        let alias_anchor = removed.join("alias-anchor");
        std::fs::create_dir(&alias_anchor).unwrap();
        let removed_alias = alias_anchor.join("..");
        let repo = Repository::at(test.root_path()).unwrap();
        let worktrees = repo.list_worktrees().unwrap();

        let found = live_sibling_checkout(worktrees, "feature", &removed_alias).unwrap();
        assert!(
            worktrunk::path::paths_match(&found.path, &survivor),
            "only the canonical target path may be excluded"
        );
    }

    #[test]
    fn test_parse_porcelain_z_modified_staged() {
        // "M  file.txt\0" - staged modification
        let output = "M  file.txt\0";
        assert_eq!(parse_porcelain_z(output), vec!["file.txt"]);
    }

    #[test]
    fn test_parse_porcelain_z_modified_unstaged() {
        // " M file.txt\0" - unstaged modification (this was the bug case)
        let output = " M file.txt\0";
        assert_eq!(parse_porcelain_z(output), vec!["file.txt"]);
    }

    #[test]
    fn test_parse_porcelain_z_modified_both() {
        // "MM file.txt\0" - both staged and unstaged
        let output = "MM file.txt\0";
        assert_eq!(parse_porcelain_z(output), vec!["file.txt"]);
    }

    #[test]
    fn test_parse_porcelain_z_untracked() {
        // "?? new.txt\0" - untracked file
        let output = "?? new.txt\0";
        assert_eq!(parse_porcelain_z(output), vec!["new.txt"]);
    }

    #[test]
    fn test_parse_porcelain_z_rename() {
        // "R  new.txt\0old.txt\0" - rename includes both paths
        let output = "R  new.txt\0old.txt\0";
        let result = parse_porcelain_z(output);
        assert_eq!(result, vec!["new.txt", "old.txt"]);
    }

    #[test]
    fn test_parse_porcelain_z_copy() {
        // "C  copy.txt\0original.txt\0" - copy includes both paths
        let output = "C  copy.txt\0original.txt\0";
        let result = parse_porcelain_z(output);
        assert_eq!(result, vec!["copy.txt", "original.txt"]);
    }

    #[test]
    fn test_parse_porcelain_z_multiple_files() {
        // Multiple files with different statuses
        let output = " M file1.txt\0M  file2.txt\0?? untracked.txt\0R  new.txt\0old.txt\0";
        let result = parse_porcelain_z(output);
        assert_eq!(
            result,
            vec![
                "file1.txt",
                "file2.txt",
                "untracked.txt",
                "new.txt",
                "old.txt"
            ]
        );
    }

    #[test]
    fn test_parse_porcelain_z_filename_with_spaces() {
        // "M  file with spaces.txt\0"
        let output = "M  file with spaces.txt\0";
        assert_eq!(parse_porcelain_z(output), vec!["file with spaces.txt"]);
    }

    #[test]
    fn test_parse_porcelain_z_empty() {
        assert_eq!(parse_porcelain_z(""), Vec::<String>::new());
    }

    #[test]
    fn test_parse_porcelain_z_short_entry_skipped() {
        // Entry too short to have path (malformed, shouldn't happen in practice)
        let output = "M\0";
        assert_eq!(parse_porcelain_z(output), Vec::<String>::new());
    }

    #[test]
    fn test_parse_porcelain_z_rename_missing_old_path() {
        // Rename without old path (malformed, but should handle gracefully)
        let output = "R  new.txt\0";
        let result = parse_porcelain_z(output);
        // Should include new.txt, old path is simply not added
        assert_eq!(result, vec!["new.txt"]);
    }

    #[test]
    fn test_parse_untracked_files_single() {
        assert_eq!(parse_untracked_files("?? new.txt\0"), vec!["new.txt"]);
    }

    #[test]
    fn test_parse_untracked_files_multiple() {
        assert_eq!(
            parse_untracked_files("?? file1.txt\0?? file2.txt\0?? file3.txt\0"),
            vec!["file1.txt", "file2.txt", "file3.txt"]
        );
    }

    #[test]
    fn test_parse_untracked_files_ignores_modified() {
        // Only untracked files should be collected
        assert_eq!(
            parse_untracked_files(" M modified.txt\0?? untracked.txt\0"),
            vec!["untracked.txt"]
        );
    }

    #[test]
    fn test_parse_untracked_files_ignores_staged() {
        assert_eq!(
            parse_untracked_files("M  staged.txt\0?? untracked.txt\0"),
            vec!["untracked.txt"]
        );
    }

    #[test]
    fn test_parse_untracked_files_empty() {
        assert!(parse_untracked_files("").is_empty());
    }

    #[test]
    fn test_parse_untracked_files_skips_rename_old_path() {
        // Rename entries have old path as second NUL-separated field
        // Should only have untracked file, not the rename paths
        assert_eq!(
            parse_untracked_files("R  new.txt\0old.txt\0?? untracked.txt\0"),
            vec!["untracked.txt"]
        );
    }

    #[test]
    fn test_parse_untracked_files_with_spaces() {
        assert_eq!(
            parse_untracked_files("?? file with spaces.txt\0"),
            vec!["file with spaces.txt"]
        );
    }

    #[test]
    fn test_parse_untracked_files_no_untracked() {
        // All files are tracked (modified, staged, etc.)
        assert!(parse_untracked_files(" M file1.txt\0M  file2.txt\0").is_empty());
    }
}
