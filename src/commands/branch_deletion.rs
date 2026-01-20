//! Branch deletion logic for worktree operations.
//!
//! This module handles the decision-making around whether a branch can be safely
//! deleted after its worktree is removed. It checks if the branch's content has
//! been integrated into the target branch.

use worktrunk::git::{IntegrationReason, Repository};

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
    pub integration_target: String,
}

/// Attempt to delete a branch if it's integrated or force_delete is set.
///
/// Returns `BranchDeletionResult` with:
/// - `outcome`: Whether/why deletion occurred
/// - `integration_target`: The ref checked against (may be upstream if ahead of local)
pub fn delete_branch_if_safe(
    repo: &Repository,
    branch_name: &str,
    target: &str,
    force_delete: bool,
) -> anyhow::Result<BranchDeletionResult> {
    let (effective_target, reason) = repo.integration_reason(branch_name, target)?;

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
        integration_target: effective_target,
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
