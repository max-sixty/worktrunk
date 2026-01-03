//! Branch deletion logic for worktree operations.
//!
//! This module handles the decision-making around whether a branch can be safely
//! deleted after its worktree is removed. It checks if the branch's content has
//! been integrated into the target branch.

use worktrunk::git::{IntegrationReason, Repository};

/// Result of an integration check, including which target was used.
pub struct IntegrationResult {
    pub reason: Option<IntegrationReason>,
    /// The target that was actually checked against (may be upstream if ahead of local)
    pub effective_target: String,
}

/// Outcome of a branch deletion attempt.
pub enum BranchDeletionOutcome {
    /// Branch was not deleted (not integrated and not forced)
    NotDeleted,
    /// Branch was force-deleted without integration check
    ForceDeleted,
    /// Branch was deleted because it was integrated
    Integrated(IntegrationReason),
}

/// Result of a branch deletion attempt.
pub struct BranchDeletionResult {
    pub outcome: BranchDeletionOutcome,
    /// The target that was actually checked against (may be upstream if ahead of local)
    pub effective_target: String,
}

/// Check if a branch's content has been integrated into the target.
///
/// Returns the reason if the branch is safe to delete (ordered by check cost):
/// - `SameCommit`: Branch HEAD is literally the same commit as target
/// - `NoAddedChanges`: Branch has no file changes beyond merge-base (empty three-dot diff)
/// - `TreesMatch`: The branch's tree SHA matches the target's tree SHA (squash merge/rebase)
/// - `MergeAddsNothing`: Merge simulation shows branch would add nothing (squash + target advanced)
///
/// Also returns the effective target used (may be upstream if it's ahead of local).
///
/// Returns None reason if no condition is met, or if an error occurs (e.g., invalid refs).
/// This fail-safe default prevents accidental branch deletion when integration cannot
/// be determined.
pub fn get_integration_reason(
    repo: &Repository,
    branch_name: &str,
    target: &str,
) -> IntegrationResult {
    let effective_target = repo.effective_integration_target(target);

    let reason = check_integration_against(repo, branch_name, &effective_target);

    IntegrationResult {
        reason,
        effective_target,
    }
}

/// Check integration against a specific target ref.
fn check_integration_against(
    repo: &Repository,
    branch_name: &str,
    target: &str,
) -> Option<IntegrationReason> {
    // Use lazy provider for short-circuit evaluation.
    // Expensive checks (would_merge_add) are skipped if cheaper ones succeed.
    let mut provider = worktrunk::git::LazyGitIntegration::new(repo, branch_name, target);
    worktrunk::git::check_integration(&mut provider)
}

/// Attempt to delete a branch if it's integrated or force_delete is set.
///
/// Returns `BranchDeletionResult` with:
/// - `outcome`: Whether/why deletion occurred
/// - `effective_target`: The ref checked against (may be upstream if ahead of local)
pub fn delete_branch_if_safe(
    repo: &Repository,
    branch_name: &str,
    target: &str,
    force_delete: bool,
) -> anyhow::Result<BranchDeletionResult> {
    let IntegrationResult {
        reason,
        effective_target,
    } = get_integration_reason(repo, branch_name, target);

    // Determine outcome based on integration and force flag
    let outcome = match (reason, force_delete) {
        (Some(r), _) => {
            repo.run_command(&["branch", "-D", branch_name])?;
            BranchDeletionOutcome::Integrated(r)
        }
        (None, true) => {
            repo.run_command(&["branch", "-D", branch_name])?;
            BranchDeletionOutcome::ForceDeleted
        }
        (None, false) => BranchDeletionOutcome::NotDeleted,
    };

    Ok(BranchDeletionResult {
        outcome,
        effective_target,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_branch_deletion_outcome_matching() {
        // Ensure the match patterns work correctly
        let outcomes = [
            (BranchDeletionOutcome::NotDeleted, false),
            (BranchDeletionOutcome::ForceDeleted, true),
            (
                BranchDeletionOutcome::Integrated(IntegrationReason::SameCommit),
                true,
            ),
        ];
        for (outcome, expected_deleted) in outcomes {
            let deleted = matches!(
                outcome,
                BranchDeletionOutcome::ForceDeleted | BranchDeletionOutcome::Integrated(_)
            );
            assert_eq!(deleted, expected_deleted);
        }
    }
}
