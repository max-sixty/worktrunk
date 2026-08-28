//! Types for worktree operations.
//!
//! Core data structures used by switch, remove, and push operations.

use std::path::{Path, PathBuf};

use worktrunk::git::{
    BranchDeletionMode, BranchDeletionOutcome, BranchDeletionResult, IntegrationReason, RefType,
};

/// Flags indicating which merge operations occurred
#[derive(Debug, Clone, Copy)]
pub struct MergeOperations {
    pub committed: bool,
    pub squashed: bool,
    pub rebased: bool,
}

/// Result of a worktree switch operation
pub enum SwitchResult {
    /// Already at the target worktree (no action taken)
    AlreadyAt(PathBuf),
    /// Switched to existing worktree at the given path
    Existing { path: PathBuf },
    /// Created new worktree at the given path
    Created {
        path: PathBuf,
        /// True if the user requested branch creation (--create flag)
        created_branch: bool,
        /// Base branch when creating new branch (e.g., "main")
        base_branch: Option<String>,
        /// Absolute path to base branch's worktree (POSIX format for shell compatibility)
        base_worktree_path: Option<String>,
        /// Remote tracking branch if auto-created from remote (e.g., "origin/feature")
        from_remote: Option<String>,
    },
}

impl SwitchResult {
    /// Get the worktree path
    pub fn path(&self) -> &PathBuf {
        match self {
            SwitchResult::AlreadyAt(path) => path,
            SwitchResult::Existing { path, .. } => path,
            SwitchResult::Created { path, .. } => path,
        }
    }
}

/// Branch state for a switch operation.
#[derive(Debug, Clone)]
pub struct SwitchBranchInfo {
    /// The branch being switched to. `None` for detached HEAD worktrees.
    pub branch: Option<String>,
}

/// How the worktree will be created.
#[derive(Debug)]
pub enum CreationMethod {
    /// Use `git worktree add` - handles existing branch, DWIM from remote, or -b for new
    Regular {
        /// True if using `-b` to create a new branch (--create flag)
        create_branch: bool,
        /// Base branch for creation (resolved, validated to exist)
        base_branch: Option<String>,
        /// When `--base pr:N` / `--base mr:N` (same-repo) is paired with `--create`,
        /// the user's intent is "create a new branch tracking the PR/MR's source
        /// branch on the remote", so `git push` from the new worktree pushes back
        /// to that PR/MR. `git worktree add -b new <bare-name>` doesn't set up
        /// tracking on its own (only `<remote>/<branch>` triggers DWIM, and even
        /// then we'd unset it via the issue-#713 safety check), so we capture the
        /// (remote, branch) pair here and configure tracking explicitly after
        /// `git worktree add` succeeds. `None` for any other base resolution.
        base_pr_upstream: Option<(String, String)>,
    },
    /// Fork PR/MR: fetch from refs/pull/N/head or refs/merge-requests/N/head,
    /// create branch, configure pushRemote.
    ///
    /// The remote is resolved during planning (before approval prompts) to ensure
    /// early failure if no matching remote exists.
    ForkRef {
        /// The reference type (PR or MR).
        ref_type: RefType,
        /// The PR/MR number.
        number: u32,
        /// The ref path (e.g., "pull/123/head" or "merge-requests/42/head").
        ref_path: String,
        /// URL to push to (the fork's URL). `None` when using a prefixed branch
        /// name (e.g., `contributor/main`) because push won't work.
        fork_push_url: Option<String>,
        /// Resolved remote name where PR/MR refs live (e.g., "origin", "upstream").
        remote: String,
    },
}

/// Identity of the PR/MR a `pr:N` / `mr:N` argument resolved to.
///
/// Resolved once, before `pre-switch` hooks run, then held by whoever needs it:
/// the `ResolvedTarget` `pre-switch` reads, the plan `pre-start` reads, and the
/// pipeline's own clone behind the post-* hooks — an `Existing` switch builds no
/// `SwitchPlan::Create` at all. So `pr_number` / `pr_url` reach every switch
/// hook whether the PR came from this repo or a fork. Reading it back off
/// [`CreationMethod::ForkRef`] used to be the only source, which silently left
/// same-repo PRs (the common case) with both variables unset.
#[derive(Debug, Clone)]
pub struct RefIdentity {
    /// The PR/MR number the user typed.
    pub number: u32,
    /// Web URL of the PR/MR.
    pub url: String,
}

/// Validated plan for a switch operation.
///
/// Created by `plan_switch()`, consumed by `execute_switch()`.
/// This separation allows validation to happen before approval prompts,
/// ensuring users aren't asked to approve hooks for operations that will fail.
#[derive(Debug)]
pub enum SwitchPlan {
    /// Branch already has a worktree - just switch to it (no git commands needed)
    Existing {
        path: PathBuf,
        /// The branch at this worktree. `None` for detached HEAD.
        branch: Option<String>,
        /// Branch to record as "previous" for `wt switch -`
        new_previous: Option<String>,
    },
    /// Need to create a new worktree
    Create {
        branch: String,
        worktree_path: PathBuf,
        /// How to create the worktree
        method: CreationMethod,
        /// The PR/MR this switch resolved from, when the argument was
        /// `pr:N` / `mr:N`. Source of the `pr_number` / `pr_url` hook
        /// variables.
        ref_identity: Option<RefIdentity>,
        /// True when a stale path occupies `worktree_path` and `--clobber` was
        /// given — `execute_switch` backs it up before creating the worktree.
        needs_clobber_backup: bool,
        /// Branch to record as "previous" for `wt switch -`
        new_previous: Option<String>,
    },
}

impl SwitchPlan {
    /// Get the worktree path for this plan.
    pub fn worktree_path(&self) -> &Path {
        match self {
            SwitchPlan::Existing { path, .. } => path,
            SwitchPlan::Create { worktree_path, .. } => worktree_path,
        }
    }

    /// Returns true if this plan will create a new worktree.
    pub fn is_create(&self) -> bool {
        matches!(self, SwitchPlan::Create { .. })
    }
}

/// A surviving checkout of the branch a removal would otherwise have deleted.
///
/// Present only when the branch is checked out in more than one worktree, which
/// takes a deliberate `git worktree add --force`. Its presence is what retains
/// the branch: `deletion_mode` is forced to [`BranchDeletionMode::Keep`], since
/// deleting a ref another worktree still holds leaves that worktree at a null
/// OID with an unresolvable `HEAD`.
pub struct SharedBranchCheckout {
    /// The surviving worktree, named in the output so the retention reads as a
    /// consequence of something the user can see rather than a refusal.
    pub path: PathBuf,
    /// The removal asked to force-delete the branch (`-D`) and was refused.
    /// Everywhere else `-D` is the override that wins, so a `-D` that doesn't
    /// delete is unexpected and says so at warning volume.
    pub refused_force_delete: bool,
}

impl SharedBranchCheckout {
    pub fn new(path: &Path, requested: &BranchDeletionMode) -> Self {
        Self {
            path: path.to_path_buf(),
            refused_force_delete: requested.is_force(),
        }
    }
}

/// What actually happened to a [`RemovalPlan`]'s branch by the time the
/// executor returned — the observed counterpart of the intent the plan
/// carries.
///
/// Consumers that report deletions (the prune summary, `--format=json`) read
/// this rather than the plan, so a deletion the CAS refused or an unmerged
/// branch SafeDelete declined is never reported as deleted. `Deferred` is the
/// one case with nothing to observe; [`deleted`](Self::deleted) reports the
/// plan's intent there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BranchFate {
    /// No deletion was attempted: the plan had no branch (detached worktree)
    /// or retained it (`deletion_mode` Keep — shared checkout, or
    /// `--no-delete-branch`).
    NotAttempted,
    /// The deletion ran and the branch is gone.
    Deleted,
    /// The deletion ran and the branch survives, for the carried reason.
    Retained(RetainedReason),
    /// The deletion was handed to a detached background process (the legacy
    /// `git worktree remove` fallback) whose outcome this process never sees.
    ///
    /// Only planned deletions defer — a `Keep` plan or a branchless worktree
    /// has no deletion command to hand off, and every fallback arm maps those
    /// to [`NotAttempted`](Self::NotAttempted) — so `Deferred` implies the
    /// plan's intent was deletion.
    Deferred,
}

/// Why a deletion attempt left the branch in place.
///
/// The distinctions matter to whoever is told about them: `Raced` is the one
/// an automated caller retries (re-read the ref and try again), `Unmerged` the
/// one a user resolves with `-D`, and the other two report state that has to
/// change elsewhere first. Collapsing them loses the retry signal: the
/// fail-closed compare-and-swap in `delete_branch_if_safe` detects a moved ref
/// precisely so the caller can act on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetainedReason {
    /// Not integrated into the target, and deletion wasn't forced.
    Unmerged,
    /// The final topology read found a live worktree with it checked out.
    CheckedOut,
    /// The compare-and-swap was refused: the ref moved between the integration
    /// check and the delete.
    Raced,
    /// The delete command itself failed (already narrated at the site that
    /// observed it).
    Failed,
}

impl BranchFate {
    /// Map a synchronous deletion attempt's result. `None` means no attempt
    /// was made.
    pub fn from_result(result: Option<&anyhow::Result<BranchDeletionResult>>) -> Self {
        match result {
            None => Self::NotAttempted,
            Some(Ok(r)) => Self::from_outcome(&r.outcome),
            Some(Err(_)) => Self::Retained(RetainedReason::Failed),
        }
    }

    /// Map a completed deletion's outcome. The one place the mapping lives, so
    /// the paths that hold an outcome without a `Result` around it
    /// (the background fast path's post-execution read) can't classify it
    /// differently from [`from_result`](Self::from_result).
    pub fn from_outcome(outcome: &BranchDeletionOutcome) -> Self {
        match outcome {
            BranchDeletionOutcome::Integrated(_) | BranchDeletionOutcome::ForceDeleted => {
                Self::Deleted
            }
            BranchDeletionOutcome::NotDeleted => Self::Retained(RetainedReason::Unmerged),
            BranchDeletionOutcome::RetainedCheckedOut { .. } => {
                Self::Retained(RetainedReason::CheckedOut)
            }
            BranchDeletionOutcome::RetainedRaced => Self::Retained(RetainedReason::Raced),
        }
    }

    /// Whether the branch was deleted, best knowledge: the observed outcome
    /// where one exists, the plan's intent for `Deferred` (which is always
    /// deletion — see the variant doc).
    pub fn deleted(&self) -> bool {
        matches!(self, Self::Deleted | Self::Deferred)
    }

    /// The `branch_outcome` value in `--format=json`.
    ///
    /// Names the observed outcome rather than summarising it as a boolean, so
    /// a caller can tell a race it should retry from a retention it asked for,
    /// and can tell a detached deletion nobody watched from a confirmed one.
    pub fn json_outcome(&self) -> &'static str {
        match self {
            Self::NotAttempted => "not_attempted",
            Self::Deleted => "deleted",
            Self::Deferred => "deferred",
            Self::Retained(RetainedReason::Unmerged) => "retained_unmerged",
            Self::Retained(RetainedReason::CheckedOut) => "retained_checked_out",
            Self::Retained(RetainedReason::Raced) => "retained_raced",
            Self::Retained(RetainedReason::Failed) => "retained_failed",
        }
    }
}

/// A validated, planned removal — what `prepare_worktree_removal` decided to
/// remove and how, before anything runs.
///
/// Produced by the planner, executed by `output::handle_remove_output`. The
/// deletion-relevant fields are intent, not fact: `deletion_mode` says what the
/// executor should attempt, and the actual safe deletion rechecks both worktree
/// topology and fresh refs (`delete_branch_if_safe`'s guard + CAS).
pub enum RemovalPlan {
    /// Remove a worktree, changing directory away from it first if it's current.
    Worktree {
        /// Stable working directory for post-removal execution: hooks run here,
        /// background removal spawns from here, and `cd` directs the shell here.
        /// Usually the primary worktree; falls back to cwd when removing the
        /// primary worktree itself (bare repo edge case), or the target branch's
        /// worktree in `wt merge`.
        main_path: PathBuf,
        worktree_path: PathBuf,
        changed_directory: bool,
        /// Branch name, if known. None for detached HEAD state.
        branch_name: Option<String>,
        deletion_mode: BranchDeletionMode,
        target_branch: Option<String>,
        /// Integration verdict at planning time, for display and retention
        /// prediction. The deletion itself re-decides against fresh refs via
        /// `delete_branch_if_safe`'s atomic CAS, so this is never a safety input.
        integration_reason: Option<IntegrationReason>,
        /// Force git worktree removal even with untracked files.
        force_worktree: bool,
        /// Commit SHA of the removed worktree's HEAD, captured before removal.
        /// Used for post-remove hook template variables so they reference the
        /// removed worktree's state, not the execution context.
        removed_commit: Option<String>,
        /// A surviving checkout of `branch_name`, when one exists. `Some`
        /// retains the branch and forces `deletion_mode` to `Keep`; see
        /// [`SharedBranchCheckout`].
        branch_checked_out_at: Option<SharedBranchCheckout>,
    },
    /// Branch exists but has no worktree directory - attempt branch deletion
    /// only, unregistering the stale worktree entry first when one remains.
    BranchOnly {
        branch_name: String,
        deletion_mode: BranchDeletionMode,
        /// Stale worktree entry to unregister, recorded when the plan fell
        /// back from a worktree whose directory was missing. Planning never
        /// mutates — execution performs the prune (`git worktree remove`,
        /// which validates before touching anything, so a directory that
        /// reappears at this path between planning and execution is handled
        /// by git: a clean reconnected worktree is removed, a dirty or
        /// foreign one refuses loudly). `None` when the branch has no
        /// worktree entry at all.
        prune_entry: Option<PathBuf>,
        /// Integration target for display. May be the effective target (e.g.,
        /// `origin/main` when upstream is ahead) or the local default branch.
        /// `None` when no default branch is configured.
        target_branch: Option<String>,
        /// Integration reason computed during preparation for display. The
        /// executor still rechecks topology and ref state before safe deletion.
        integration_reason: Option<worktrunk::git::IntegrationReason>,
        /// A surviving checkout of `branch_name`, when one exists. Only reachable
        /// on a pruned removal — the target's directory was gone, but a sibling
        /// checkout of the same branch survives the fallback to branch-only
        /// deletion. See [`SharedBranchCheckout`].
        branch_checked_out_at: Option<SharedBranchCheckout>,
    },
}

impl RemovalPlan {
    /// Path of the worktree this plan removes, if it removes one.
    ///
    /// `None` for branch-only deletions — they have no worktree, so no
    /// `pre-remove` hook runs and there's no worktree-local `.config/wt.toml`
    /// to consult.
    pub fn removed_worktree_path(&self) -> Option<&Path> {
        match self {
            RemovalPlan::Worktree { worktree_path, .. } => Some(worktree_path),
            RemovalPlan::BranchOnly { .. } => None,
        }
    }

    /// Branch named by this plan. `None` only for a detached-HEAD worktree;
    /// branch-only plans always return their branch.
    ///
    /// Only the `--reap` label needs this, which is Unix-only; gated to avoid a
    /// dead-code warning on Windows where nothing consumes it.
    #[cfg(unix)]
    pub fn branch_name(&self) -> Option<&str> {
        match self {
            RemovalPlan::Worktree { branch_name, .. } => branch_name.as_deref(),
            RemovalPlan::BranchOnly { branch_name, .. } => Some(branch_name),
        }
    }

    /// Post-removal working directory — where the user lands, and the worktree
    /// whose `.config/wt.toml` `post-switch` reads. `None` for branch-only
    /// deletions (no worktree was removed, so nothing was switched away from).
    /// See the `main_path` field docs on [`RemovalPlan::Worktree`].
    pub fn destination_path(&self) -> Option<&Path> {
        match self {
            RemovalPlan::Worktree { main_path, .. } => Some(main_path),
            RemovalPlan::BranchOnly { .. } => None,
        }
    }

    /// Whether this plan intends to delete a branch.
    ///
    /// False for a detached worktree, which has none, and false whenever a
    /// sibling checkout forced `deletion_mode` to [`BranchDeletionMode::Keep`]
    /// (see [`SharedBranchCheckout`]). Intent only: a `SafeDelete` still
    /// re-decides against fresh refs at execution, so anything reporting what
    /// happened reads [`BranchFate`]; this is what the scan-time predictions
    /// (prune's dry run) print, and the question the progress message answers
    /// when it says "worktree" rather than "worktree & branch".
    pub fn deletes_branch(&self) -> bool {
        match self {
            RemovalPlan::Worktree {
                branch_name,
                deletion_mode,
                ..
            } => branch_name.is_some() && !deletion_mode.should_keep(),
            RemovalPlan::BranchOnly { deletion_mode, .. } => !deletion_mode.should_keep(),
        }
    }

    /// Convert to a JSON value for structured output.
    ///
    /// `fate` is what execution reported back, and `branch_outcome` names it
    /// directly (via [`BranchFate::json_outcome`]) so the payload states what
    /// happened rather than what the plan hoped. It replaced a
    /// `branch_deleted` boolean, which could not distinguish a CAS the ref
    /// moved under from a retention the caller asked for, and reported a
    /// detached deletion nobody watched as an accomplished one.
    pub fn to_json(&self, fate: BranchFate) -> serde_json::Value {
        let branch_outcome = fate.json_outcome();
        match self {
            RemovalPlan::Worktree {
                worktree_path,
                branch_name,
                branch_checked_out_at,
                ..
            } => serde_json::json!({
                "kind": "worktree",
                "branch": branch_name,
                "path": worktree_path,
                "branch_outcome": branch_outcome,
                "branch_checked_out_at": branch_checked_out_at.as_ref().map(|c| &c.path),
            }),
            RemovalPlan::BranchOnly {
                branch_name,
                prune_entry,
                branch_checked_out_at,
                ..
            } => serde_json::json!({
                "kind": "branch_only",
                "branch": branch_name,
                "pruned": prune_entry.is_some(),
                "branch_outcome": branch_outcome,
                "branch_checked_out_at": branch_checked_out_at.as_ref().map(|c| &c.path),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn remove_result_branch_name_reads_both_variants() {
        let removed = RemovalPlan::Worktree {
            main_path: PathBuf::from("/main"),
            worktree_path: PathBuf::from("/wt"),
            changed_directory: false,
            branch_name: Some("feature".to_string()),
            deletion_mode: BranchDeletionMode::default(),
            target_branch: None,
            integration_reason: None,
            force_worktree: false,
            removed_commit: None,
            branch_checked_out_at: None,
        };
        assert_eq!(removed.branch_name(), Some("feature"));

        let branch_only = RemovalPlan::BranchOnly {
            branch_name: "solo".to_string(),
            deletion_mode: BranchDeletionMode::default(),
            prune_entry: None,
            target_branch: None,
            integration_reason: None,
            branch_checked_out_at: None,
        };
        assert_eq!(branch_only.branch_name(), Some("solo"));
    }

    /// The fate→deleted mapping: an observed deletion counts, a deferral
    /// reports the intent it carried (always deletion), and a raced or
    /// declined deletion (`Retained`) reports false even though the plan
    /// meant to delete — that divergence is the whole point of tracking fate
    /// separately.
    #[test]
    fn branch_fate_deleted_mapping() {
        let cases = [
            (BranchFate::Deleted, true),
            (BranchFate::Deferred, true),
            (BranchFate::Retained(RetainedReason::Raced), false),
            (BranchFate::NotAttempted, false),
        ];
        for (fate, deleted) in cases {
            assert_eq!(fate.deleted(), deleted, "{fate:?}");
        }
    }

    /// The `--format=json` vocabulary, which external callers branch on. Every
    /// fate gets its own value: a race the caller should retry never reads as
    /// a retention it asked for, and a detached deletion nobody watched never
    /// reads as a confirmed one.
    #[test]
    fn branch_fate_json_outcome_is_distinct_per_fate() {
        let cases = [
            (BranchFate::NotAttempted, "not_attempted"),
            (BranchFate::Deleted, "deleted"),
            (BranchFate::Deferred, "deferred"),
            (
                BranchFate::Retained(RetainedReason::Unmerged),
                "retained_unmerged",
            ),
            (
                BranchFate::Retained(RetainedReason::CheckedOut),
                "retained_checked_out",
            ),
            (
                BranchFate::Retained(RetainedReason::Raced),
                "retained_raced",
            ),
            (
                BranchFate::Retained(RetainedReason::Failed),
                "retained_failed",
            ),
        ];
        for (fate, expected) in cases {
            assert_eq!(fate.json_outcome(), expected, "{fate:?}");
        }
        let names: std::collections::BTreeSet<_> = cases.iter().map(|(_, name)| *name).collect();
        assert_eq!(names.len(), cases.len(), "outcome names must be distinct");
    }

    /// Synchronous deletion results map onto fates: deletions count, each
    /// refusal keeps the reason that distinguishes it, an error reads as the
    /// delete having failed, and absence as never attempted.
    #[test]
    fn branch_fate_from_result_mapping() {
        use worktrunk::git::IntegrationReason;

        let fate = |outcome| {
            BranchFate::from_result(Some(&Ok(BranchDeletionResult {
                outcome,
                integration_target: "main".to_string(),
            })))
        };
        assert_eq!(
            fate(BranchDeletionOutcome::Integrated(
                IntegrationReason::SameCommit
            )),
            BranchFate::Deleted
        );
        assert_eq!(
            fate(BranchDeletionOutcome::ForceDeleted),
            BranchFate::Deleted
        );
        assert_eq!(
            fate(BranchDeletionOutcome::NotDeleted),
            BranchFate::Retained(RetainedReason::Unmerged)
        );
        assert_eq!(
            fate(BranchDeletionOutcome::RetainedCheckedOut {
                path: PathBuf::from("/tmp/feature"),
            }),
            BranchFate::Retained(RetainedReason::CheckedOut)
        );
        assert_eq!(
            fate(BranchDeletionOutcome::RetainedRaced),
            BranchFate::Retained(RetainedReason::Raced)
        );
        assert_eq!(
            BranchFate::from_result(Some(&Err(anyhow::anyhow!("boom")))),
            BranchFate::Retained(RetainedReason::Failed)
        );
        assert_eq!(BranchFate::from_result(None), BranchFate::NotAttempted);
    }

    #[test]
    fn test_switch_result_path() {
        let cases = [
            (
                "already at worktree",
                SwitchResult::AlreadyAt(PathBuf::from("/test/path")),
                PathBuf::from("/test/path"),
            ),
            (
                "existing worktree",
                SwitchResult::Existing {
                    path: PathBuf::from("/test/existing"),
                },
                PathBuf::from("/test/existing"),
            ),
            (
                "created branch",
                SwitchResult::Created {
                    path: PathBuf::from("/test/created"),
                    created_branch: true,
                    base_branch: Some("main".to_string()),
                    base_worktree_path: Some("/test/main".to_string()),
                    from_remote: None,
                },
                PathBuf::from("/test/created"),
            ),
            (
                "created from remote",
                SwitchResult::Created {
                    path: PathBuf::from("/test/remote"),
                    created_branch: false,
                    base_branch: None,
                    base_worktree_path: None,
                    from_remote: Some("origin/feature".to_string()),
                },
                PathBuf::from("/test/remote"),
            ),
        ];

        for (name, result, expected) in cases {
            assert_eq!(result.path(), &expected, "{name}");
        }
    }

    #[test]
    fn test_remove_result_removed_worktree() {
        let result = RemovalPlan::Worktree {
            main_path: PathBuf::from("/main"),
            worktree_path: PathBuf::from("/worktree"),
            changed_directory: true,
            branch_name: Some("feature".to_string()),
            deletion_mode: BranchDeletionMode::SafeDelete,
            target_branch: Some("main".to_string()),
            integration_reason: None,
            force_worktree: false,
            removed_commit: Some("abc1234567890".to_string()),
            branch_checked_out_at: None,
        };
        match result {
            RemovalPlan::Worktree {
                main_path,
                worktree_path,
                changed_directory,
                branch_name,
                deletion_mode,
                target_branch,
                integration_reason: _,
                force_worktree,
                removed_commit,
                branch_checked_out_at,
            } => {
                assert_eq!(main_path.to_str().unwrap(), "/main");
                assert_eq!(worktree_path.to_str().unwrap(), "/worktree");
                assert!(changed_directory);
                assert_eq!(branch_name.as_deref(), Some("feature"));
                assert!(!deletion_mode.should_keep());
                assert!(!deletion_mode.is_force());
                assert_eq!(target_branch.as_deref(), Some("main"));
                assert!(!force_worktree);
                assert_eq!(removed_commit.as_deref(), Some("abc1234567890"));
                assert!(branch_checked_out_at.is_none());
            }
            _ => panic!("Expected Worktree variant"),
        }
    }

    #[test]
    fn test_remove_result_branch_only() {
        let result = RemovalPlan::BranchOnly {
            branch_name: "stale-branch".to_string(),
            deletion_mode: BranchDeletionMode::Keep,
            prune_entry: None,
            target_branch: None,
            integration_reason: None,
            branch_checked_out_at: None,
        };
        match result {
            RemovalPlan::BranchOnly {
                branch_name,
                deletion_mode,
                prune_entry,
                target_branch,
                integration_reason,
                branch_checked_out_at,
            } => {
                assert_eq!(branch_name, "stale-branch");
                assert!(deletion_mode.should_keep());
                assert!(!deletion_mode.is_force());
                assert!(prune_entry.is_none());
                assert!(target_branch.is_none());
                assert!(integration_reason.is_none());
                assert!(branch_checked_out_at.is_none());
            }
            _ => panic!("Expected BranchOnly variant"),
        }
    }

    #[test]
    fn test_remove_result_branch_only_pruned() {
        let result = RemovalPlan::BranchOnly {
            branch_name: "pruned-branch".to_string(),
            deletion_mode: BranchDeletionMode::SafeDelete,
            prune_entry: Some(PathBuf::from("/stale")),
            target_branch: Some("main".to_string()),
            integration_reason: None,
            branch_checked_out_at: None,
        };
        match result {
            RemovalPlan::BranchOnly {
                branch_name,
                deletion_mode,
                prune_entry,
                target_branch,
                integration_reason,
                branch_checked_out_at,
            } => {
                assert_eq!(branch_name, "pruned-branch");
                assert!(!deletion_mode.should_keep());
                assert_eq!(prune_entry.as_deref(), Some(Path::new("/stale")));
                assert_eq!(target_branch.as_deref(), Some("main"));
                assert!(integration_reason.is_none());
                assert!(branch_checked_out_at.is_none());
            }
            _ => panic!("Expected BranchOnly variant"),
        }
    }

    #[test]
    fn test_remove_result_with_force_delete() {
        let result = RemovalPlan::Worktree {
            main_path: PathBuf::from("/main"),
            worktree_path: PathBuf::from("/worktree"),
            changed_directory: false,
            branch_name: None, // Detached HEAD
            deletion_mode: BranchDeletionMode::ForceDelete,
            target_branch: None,
            integration_reason: None,
            force_worktree: true,
            removed_commit: None, // Detached HEAD may not have meaningful commit
            branch_checked_out_at: None,
        };
        match result {
            RemovalPlan::Worktree {
                branch_name,
                deletion_mode,
                force_worktree,
                ..
            } => {
                assert!(branch_name.is_none());
                assert!(deletion_mode.is_force());
                assert!(force_worktree);
            }
            _ => panic!("Expected Worktree variant"),
        }
    }
}
