//! Execute batch removal of pruned candidates.

use anyhow::{Context, Result};
use color_print::cformat;
use worktrunk::config::UserConfig;
use worktrunk::git::Repository;
use worktrunk::styling::eprintln;

use crate::commands::repository_ext::{RemoveTarget, RepositoryCliExt};
use crate::output::handle_remove_output;

use super::{PruneCandidate, PruneReason};

/// Result of executing prune operation.
#[derive(Debug)]
pub struct PruneResult {
    /// Successfully removed branches
    pub removed: Vec<String>,
    /// Failed removals with error messages
    pub failed: Vec<(String, String)>,
}

/// Execute batch removal of candidates.
pub fn execute_prune(candidates: Vec<PruneCandidate>, config: &UserConfig) -> Result<PruneResult> {
    let repo = Repository::current()?;
    let mut result = PruneResult {
        removed: Vec::new(),
        failed: Vec::new(),
    };

    eprintln!("\nRemoving {} items...", candidates.len());

    for candidate in candidates {
        match remove_candidate(&repo, &candidate, config) {
            Ok(()) => {
                result.removed.push(candidate.branch.clone());
                let reason_str = match &candidate.reason {
                    PruneReason::Integrated(r, t) => {
                        format!("{} ({})", t, reason_display(r))
                    }
                    PruneReason::Prunable => "pruned".to_string(),
                };
                eprintln!(
                    "{}",
                    cformat!("<green>✓</> {} {}", candidate.branch, reason_str)
                );
            }
            Err(e) => {
                result
                    .failed
                    .push((candidate.branch.clone(), e.to_string()));
                eprintln!("{}", cformat!("<red>✗</> {}: {}", candidate.branch, e));
            }
        }
    }

    Ok(result)
}

/// Remove a single candidate.
fn remove_candidate(
    repo: &Repository,
    candidate: &PruneCandidate,
    config: &UserConfig,
) -> Result<()> {
    use crate::commands::branch_deletion::delete_branch_if_safe;
    use crate::commands::worktree::BranchDeletionMode;

    // Prunable: just prune and delete branch if integrated
    if matches!(candidate.reason, PruneReason::Prunable) {
        repo.prune_worktrees()?;
        // Delete branch if it was integrated (we checked in collection phase)
        if candidate.integration_reason.is_some() {
            // Use the effective target from repo
            let target = repo
                .integration_target()
                .ok_or_else(|| anyhow::anyhow!("No default branch found"))?;
            // force_delete=false means only delete if integrated
            delete_branch_if_safe(repo, &candidate.branch, &target, false)?;
        }
        return Ok(());
    }

    // Normal removal: reuse prepare_worktree_removal + output handler
    let remove_result = repo
        .prepare_worktree_removal(
            RemoveTarget::Branch(&candidate.branch),
            BranchDeletionMode::SafeDelete,
            false, // force_worktree
            config,
        )
        .context(format!("Failed to prepare removal of {}", candidate.branch))?;

    // Execute removal via existing output handler
    // Use background=true (async) and verify=true (run hooks)
    handle_remove_output(&remove_result, true, true)?;

    Ok(())
}

/// Display integration reason in human-readable form.
fn reason_display(reason: &worktrunk::git::IntegrationReason) -> &'static str {
    use worktrunk::git::IntegrationReason;
    match reason {
        IntegrationReason::Ancestor => "ancestor",
        IntegrationReason::SameCommit => "same commit",
        IntegrationReason::TreesMatch => "trees match",
        IntegrationReason::NoAddedChanges => "no added changes",
        IntegrationReason::MergeAddsNothing => "merge adds nothing",
    }
}
