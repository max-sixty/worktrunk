//! Collect candidates for pruning.

use anyhow::Result;
use worktrunk::git::Repository;

use super::{PruneCandidate, PruneOptions, PruneReason};

/// Collect all integrated and prunable worktree candidates.
pub fn collect_candidates(repo: &Repository, opts: &PruneOptions) -> Result<Vec<PruneCandidate>> {
    let target = effective_target(repo, opts)?;
    let worktrees = repo.list_worktrees()?;
    let mut candidates = Vec::new();

    for wt in worktrees {
        // Skip if no branch (detached HEAD in linked worktree)
        let Some(branch) = &wt.branch else {
            continue;
        };

        // Prunable worktree (directory missing)
        if !wt.path.exists() {
            // Check if branch is integrated to decide whether to delete it
            let integration_reason = repo.integration_reason(branch, &target)?.1;
            candidates.push(PruneCandidate {
                branch: branch.clone(),
                worktree_path: None,
                reason: PruneReason::Prunable,
                integration_reason,
            });
            continue;
        }

        // Check integration
        if let (effective_target, Some(reason)) = repo.integration_reason(branch, &target)? {
            candidates.push(PruneCandidate {
                branch: branch.clone(),
                worktree_path: Some(wt.path.clone()),
                reason: PruneReason::Integrated(reason, effective_target.clone()),
                integration_reason: Some(reason),
            });
        }
    }

    Ok(candidates)
}

/// Get the effective integration target from options or repository default.
fn effective_target(repo: &Repository, opts: &PruneOptions) -> Result<String> {
    if let Some(ref target) = opts.target {
        return Ok(target.clone());
    }
    // Use default integration target
    repo.integration_target()
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("No default branch found"))
}
