use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use super::worktree::{RemovalPlan, SharedBranchCheckout};
use anyhow::{Context, bail};
use color_print::cformat;
use worktrunk::git::{
    BranchDeletionMode, GitError, IntegrationReason, RefSnapshot, Repository, WorkingTree,
    WorktreeInfo, parse_porcelain_z, parse_untracked_files,
};
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{
    eprintln, format_with_gutter, progress_message, suggest_command, warning_message,
};

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

    /// Prepare the target worktree for push by auto-stashing non-overlapping changes when safe.
    ///
    /// The caller has already established that `target_worktree` exists on disk
    /// (`MergeContext::prepare` refuses a registered-but-missing worktree), so
    /// the status read here is free to fail if the directory is gone.
    ///
    /// Ignored files are deliberately out of scope — they're absent from the
    /// `git status --porcelain` read this works from, and matching git there is
    /// the decision, not an oversight. The module spec in
    /// `commands/worktree/push.rs` says why.
    fn prepare_target_worktree(
        &self,
        target_worktree: Option<&PathBuf>,
        target_branch: &str,
    ) -> anyhow::Result<Option<TargetWorktreeStash>>;

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
                // worktree has no branch to fall back to, so it proceeds and
                // surfaces the removal error.
                //
                // The recorded prune names this worktree rather than sweeping
                // the repo, so a sibling whose directory is merely absent
                // right now keeps its registration. `git worktree remove`
                // refuses a locked worktree where a repo-wide prune ignored
                // one, which needs no guard here: the lock check above already
                // returned for every locked entry in this arm.
                if let Some(branch) = wt.branch.as_deref()
                    && !wt.path.exists()
                {
                    Resolved::BranchOnly {
                        pruned_from: Some(wt.path.clone()),
                        branch: branch.to_string(),
                    }
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

    fn prepare_target_worktree(
        &self,
        target_worktree: Option<&PathBuf>,
        target_branch: &str,
    ) -> anyhow::Result<Option<TargetWorktreeStash>> {
        let Some(wt_path) = target_worktree else {
            return Ok(None);
        };

        let wt = self.worktree_at(wt_path);
        if !wt.is_dirty()? {
            return Ok(None);
        }

        let push_files = self.changed_files(target_branch, "HEAD")?;
        // Use -z for NUL-separated output: handles filenames with spaces and renames correctly
        // Format: "XY path\0" for normal files, "XY new_path\0old_path\0" for renames/copies
        let wt_status_output = wt.run_command(&["status", "--porcelain", "-z"])?;

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

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let stash_name = format!(
            "worktrunk autostash::{}::{}::{}",
            target_branch,
            process::id(),
            nanos
        );

        eprintln!(
            "{}",
            progress_message(cformat!(
                "Stashing changes in <bold>{}</>...",
                format_path_for_display(wt_path)
            ))
        );

        // Stash all changes including untracked files.
        // Note: git stash push returns exit code 0 whether or not anything was stashed.
        wt.run_command(&["stash", "push", "--include-untracked", "-m", &stash_name])?;

        // Verify stash was created by checking the stash list for our entry, and
        // capture its immutable commit SHA (`%H`) rather than the positional
        // reflog selector (`%gd`, i.e. `stash@{0}`). The selector is a position:
        // any stash pushed into the repo between now and the restore — a
        // concurrent `wt merge`, an editor integration — shifts the indices, so
        // popping `stash@{0}` later would restore (and drop) a different entry.
        // The commit SHA is addressed by content and never moves.
        let list_output = wt.run_command(&["stash", "list", "--format=%H%x00%gs%x00"])?;
        let mut parts = list_output.split('\0');
        while let Some(sha) = parts.next() {
            // git separates stash-list records with `\n`, so every SHA after the
            // first carries a leading newline; trim before use.
            let sha = sha.trim();
            if sha.is_empty() {
                continue;
            }
            if let Some(message) = parts.next()
                && (message == stash_name || message.ends_with(&stash_name))
            {
                return Ok(Some(TargetWorktreeStash::new(
                    wt_path,
                    sha.to_string(),
                    &stash_name,
                )));
            }
        }

        // Stash entry not found. Verify the worktree is now clean — if it's still
        // dirty, stashing may have failed silently or our lookup missed the entry.
        if wt.is_dirty()? {
            bail!(cformat!(
                "Failed to stash changes in {}; worktree still has uncommitted changes. \
                 Expected stash entry: <bold>{}</>. Check <bold>git stash list</>.",
                format_path_for_display(wt_path),
                stash_name
            ));
        }

        // Worktree is clean and no stash entry — nothing needed to be stashed
        Ok(None)
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

/// Stash guard that auto-restores on drop.
///
/// Created by `prepare_target_worktree()` when the target worktree has changes
/// that don't conflict with the push. Automatically restores the stash when
/// dropped, ensuring cleanup happens in both success and error paths.
#[must_use = "stash guard restores immediately if dropped; hold it until push completes"]
pub(crate) struct TargetWorktreeStash {
    /// Inner data wrapped in Option so we can take() in Drop.
    /// None means already restored (or disarmed).
    inner: Option<StashData>,
}

struct StashData {
    path: PathBuf,
    /// Immutable commit SHA of the stash entry. Restored by content, so a stash
    /// pushed into the repo since capture can't redirect the restore.
    stash_sha: String,
    /// The unique `-m` message recorded at capture. Used to re-locate the entry
    /// in the (possibly reordered) stash list when dropping it after apply.
    stash_name: String,
}

impl StashData {
    /// Restore the stash, printing progress and warning on failure.
    fn restore(self) {
        eprintln!(
            "{}",
            progress_message(cformat!(
                "Restoring stashed changes in <bold>{}</>...",
                format_path_for_display(&self.path)
            ))
        );

        if let Err(e) = self.restore_inner() {
            eprintln!(
                "{}",
                warning_message(cformat!(
                    "Failed to restore stashed changes in <bold>{path}</>; run <bold>git stash apply {sha}</> there to recover ({err})",
                    path = format_path_for_display(&self.path),
                    sha = self.stash_sha,
                    err = e,
                ))
            );
        }
    }

    /// Apply the stash by its immutable commit SHA, then drop the entry.
    ///
    /// `git stash apply <sha>` is content-addressed, so it restores this exact
    /// entry regardless of how the stash list has been reordered since capture —
    /// unlike `git stash pop stash@{0}`, which would pop whatever now sits at
    /// position 0. Because `apply` (unlike `pop`) leaves the entry in the list,
    /// we then re-locate it by its unique message and drop it. A failed drop
    /// only strands a stash entry — the working-tree content is already
    /// restored — so it is not treated as a restore failure.
    fn restore_inner(&self) -> anyhow::Result<()> {
        let repo = Repository::current()?;
        let wt = repo.worktree_at(&self.path);

        // Don't use --quiet so git shows conflicts if any.
        wt.run_command(&["stash", "apply", &self.stash_sha])?;

        if let Some(selector) = Self::locate_by_message(&wt, &self.stash_name)?
            && let Err(e) = wt.run_command(&["stash", "drop", &selector])
        {
            eprintln!(
                "{}",
                warning_message(cformat!(
                    "Restored changes but left the stash entry <bold>{selector}</> in <bold>{path}</>; drop it with <bold>git stash drop {selector}</> ({e})",
                    path = format_path_for_display(&self.path),
                ))
            );
        }

        Ok(())
    }

    /// Find the current `stash@{n}` selector for the entry with `stash_name`.
    ///
    /// The recorded selector from capture time is positional and may be stale by
    /// restore time, so the entry is re-located by its unique message. `git
    /// stash push -m X` records the subject as `On <branch>: X`, so both the
    /// exact message and the `On …:`-prefixed form are accepted.
    fn locate_by_message(wt: &WorkingTree, stash_name: &str) -> anyhow::Result<Option<String>> {
        let list_output = wt.run_command(&["stash", "list", "--format=%gd%x00%gs%x00"])?;
        let mut parts = list_output.split('\0');
        while let Some(selector) = parts.next() {
            // git separates stash-list records with `\n`, so every selector
            // after the first carries a leading newline; trim before use.
            let selector = selector.trim();
            if selector.is_empty() {
                continue;
            }
            if let Some(message) = parts.next()
                && (message == stash_name || message.ends_with(stash_name))
            {
                return Ok(Some(selector.to_string()));
            }
        }
        Ok(None)
    }
}

impl Drop for TargetWorktreeStash {
    fn drop(&mut self) {
        if let Some(data) = self.inner.take() {
            data.restore();
        }
    }
}

impl TargetWorktreeStash {
    pub(crate) fn new(path: &Path, stash_sha: String, stash_name: &str) -> Self {
        Self {
            inner: Some(StashData {
                path: path.to_path_buf(),
                stash_sha,
                stash_name: stash_name.to_string(),
            }),
        }
    }

    /// Explicitly restore the stash now, preventing Drop from restoring again.
    ///
    /// Use this when you need the restore to happen at a specific point
    /// (e.g., before a success message). Drop handles errors/early returns.
    pub(crate) fn restore_now(&mut self) {
        if let Some(data) = self.inner.take() {
            data.restore();
        }
    }
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

    /// `locate_by_message` re-finds a stash entry by its unique message so the
    /// restore path never trusts a positional selector a concurrent stash could
    /// have invalidated (#3683). It must find the entry after the list has been
    /// reordered — matching the `On <branch>: <msg>` subject that `git stash
    /// push -m` records — and report absence without matching an unrelated
    /// entry.
    #[test]
    fn locate_by_message_finds_entry_after_reorder_and_reports_absence() {
        let test = TestRepo::with_initial_commit();
        let root = test.root_path();

        // Two stashes with distinct messages. The second push lands at
        // stash@{0}, pushing the first to stash@{1} — the reorder the fix must
        // tolerate.
        std::fs::write(root.join("a.txt"), "a").unwrap();
        test.run_git(&["stash", "push", "-u", "-m", "worktrunk autostash::alpha"]);
        std::fs::write(root.join("b.txt"), "b").unwrap();
        test.run_git(&["stash", "push", "-u", "-m", "worktrunk autostash::beta"]);

        let repo = Repository::at(root).unwrap();
        let wt = repo.worktree_at(root);

        // The older entry is now at stash@{1}; located by message, not position.
        assert_eq!(
            StashData::locate_by_message(&wt, "worktrunk autostash::alpha")
                .unwrap()
                .as_deref(),
            Some("stash@{1}"),
        );

        // A message with no match scans every entry — including the trailing
        // record separator — and returns None rather than a false positive.
        assert!(
            StashData::locate_by_message(&wt, "worktrunk autostash::gamma")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn test_stash_guard_restore_now_clears_inner() {
        // Create a guard - note: this doesn't actually create a stash since we're not
        // in a real git repo with that stash ref. We're just testing the state machine.
        let mut guard = TargetWorktreeStash::new(
            std::path::Path::new("/tmp"),
            "0000000000000000000000000000000000000000".into(),
            "worktrunk autostash::test",
        );

        // Inner should be populated
        assert!(guard.inner.is_some());

        // restore_now() should clear inner (the restore itself will fail since no real repo,
        // but that's expected - we're testing the state transition)
        guard.restore_now();

        // Inner should now be None
        assert!(guard.inner.is_none());

        // Calling restore_now() again is a no-op
        guard.restore_now();
        assert!(guard.inner.is_none());
    }

    #[test]
    fn test_stash_guard_drop_clears_inner() {
        // Test that Drop also consumes the inner
        let guard = TargetWorktreeStash::new(
            std::path::Path::new("/tmp"),
            "0000000000000000000000000000000000000000".into(),
            "worktrunk autostash::test",
        );

        // Just drop it - the restore will fail (no real repo) but Drop shouldn't panic
        drop(guard);
        // If we get here, Drop worked without panicking
    }
}
