//! Filter candidates based on safety rules and user criteria.

use anyhow::Result;
use worktrunk::git::Repository;

use super::{PruneCandidate, PruneOptions, PruneReason};

/// Apply safety protections and user criteria to filter candidates.
pub fn apply_filters(
    candidates: Vec<PruneCandidate>,
    opts: &PruneOptions,
    repo: &Repository,
) -> Result<Vec<PruneCandidate>> {
    let current_branch = repo.current_worktree().branch()?;
    let default_branch = repo.default_branch();
    let worktrees = repo.list_worktrees()?;

    let filtered = candidates
        .into_iter()
        .filter(|c| {
            // Safety: Never remove current branch
            if Some(&c.branch) == current_branch.as_ref() {
                return false;
            }

            // Safety: Never remove default branch
            if let Some(ref db) = default_branch
                && c.branch == *db
            {
                return false;
            }

            // Safety: Skip locked worktrees
            if worktrees
                .iter()
                .any(|wt| wt.branch.as_ref() == Some(&c.branch) && wt.locked.is_some())
            {
                return false;
            }

            // Pattern filtering
            if let Some(ref pattern) = opts.pattern
                && let Ok(glob_pattern) = glob::Pattern::new(pattern)
                && !glob_pattern.matches(&c.branch)
            {
                return false;
            }

            // Exclude patterns
            for exclude in &opts.exclude {
                if let Ok(glob_pattern) = glob::Pattern::new(exclude)
                    && glob_pattern.matches(&c.branch)
                {
                    return false;
                }
            }

            // Force mode: include all (that passed safety checks)
            // Default: only integrated or prunable
            opts.force
                || matches!(
                    c.reason,
                    PruneReason::Integrated(..) | PruneReason::Prunable
                )
        })
        .collect();

    Ok(filtered)
}
