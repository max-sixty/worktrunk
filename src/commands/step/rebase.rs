//! `wt step rebase` — rebase onto target branch (also used by `wt merge`).

use anyhow::Context;
use color_print::cformat;
use worktrunk::git::{ErrorExt, Repository};
use worktrunk::styling::{eprintln, progress_message, success_message};

use super::super::repository_ext::RepositoryCliExt;

/// Result of a rebase operation
pub enum RebaseResult {
    /// Rebase occurred. `fast_forward` distinguishes the two flavors.
    Rebased { target: String, fast_forward: bool },
    /// Already up-to-date with target branch
    UpToDate(String),
}

/// Handle shared rebase workflow (used by `wt step rebase` and `wt merge`)
pub fn handle_rebase(target: Option<&str>) -> anyhow::Result<RebaseResult> {
    let repo = Repository::current()?;

    // Get and validate target ref (any commit-ish for rebase)
    let integration_target = repo.require_target_ref(target)?;
    let git_target = match target {
        Some("@") | Some("-") | Some("^") | None => integration_target.clone(),
        Some(_) => {
            let exact_target = repo.disambiguate_remote_ref(&integration_target)?;
            repo.require_target_ref(Some(&exact_target))?
        }
    };
    handle_rebase_target(&repo, integration_target, git_target)
}

/// Rebase onto an already-validated local branch, preserving its local identity.
pub fn handle_rebase_onto_local_branch(target_branch: &str) -> anyhow::Result<RebaseResult> {
    let repo = Repository::current()?;
    let git_target = format!("refs/heads/{target_branch}");
    repo.require_target_ref(Some(&git_target))?;
    handle_rebase_target(&repo, target_branch.to_string(), git_target)
}

fn handle_rebase_target(
    repo: &Repository,
    integration_target: String,
    git_target: String,
) -> anyhow::Result<RebaseResult> {
    // Check if already up-to-date (linear extension of target, no merge commits)
    if repo.is_rebased_onto(&git_target)? {
        return Ok(RebaseResult::UpToDate(integration_target));
    }

    // Check if this is a fast-forward or true rebase
    let merge_base = repo
        .merge_base("HEAD", &git_target)?
        .context("Cannot rebase: no common ancestor with target branch")?;
    let head_sha = repo.run_command(&["rev-parse", "HEAD"])?.trim().to_string();
    let is_fast_forward = merge_base == head_sha;

    // Only show progress for true rebases (fast-forwards are instant)
    if !is_fast_forward {
        eprintln!(
            "{}",
            progress_message(cformat!("Rebasing onto <bold>{integration_target}</>..."))
        );
    }

    let rebase_result = repo.run_command(&["rebase", "--end-of-options", &git_target]);

    // If rebase failed, check if it's due to conflicts
    if let Err(e) = rebase_result {
        // Check if it's a rebase conflict
        let is_rebasing = repo
            .worktree_state()?
            .is_some_and(|s| s.starts_with("REBASING"));
        // Pull git's stderr from the typed leaf when present so we get the
        // raw conflict-marker bytes regardless of how many `.context(...)`
        // layers wrap the error.
        let detail = e.display_message();
        if is_rebasing {
            return Err(worktrunk::git::GitError::RebaseConflict {
                target_branch: integration_target,
                git_output: detail,
            }
            .into());
        }
        // Not a rebase conflict, return original error
        return Err(worktrunk::git::GitError::Other {
            message: cformat!(
                "Failed to rebase onto <bold>{}</>: {}",
                integration_target,
                detail
            ),
        }
        .into());
    }

    // Verify rebase completed successfully (safety check for edge cases)
    if repo.worktree_state()?.is_some() {
        return Err(worktrunk::git::GitError::RebaseConflict {
            target_branch: integration_target,
            git_output: String::new(),
        }
        .into());
    }

    // Success
    let msg = if is_fast_forward {
        cformat!("Fast-forwarded to <bold>{integration_target}</>")
    } else {
        cformat!("Rebased onto <bold>{integration_target}</>")
    };
    eprintln!("{}", success_message(msg));

    Ok(RebaseResult::Rebased {
        target: integration_target,
        fast_forward: is_fast_forward,
    })
}
