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

    // Check if already up-to-date (linear extension of target, no merge commits)
    if repo.is_rebased_onto(&integration_target)? {
        return Ok(RebaseResult::UpToDate(integration_target));
    }

    // Check if this is a fast-forward or true rebase
    let merge_base = repo
        .merge_base("HEAD", &integration_target)?
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

    let rebase_result = repo.run_command(&["rebase", "--end-of-options", &integration_target]);

    // If rebase failed, classify the failure (interrupt vs conflict vs other).
    if let Err(e) = rebase_result {
        let is_rebasing = repo
            .worktree_state()?
            .is_some_and(|s| s.starts_with("REBASING"));
        return Err(classify_rebase_failure(e, is_rebasing, &integration_target));
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

/// Turn a failed `git rebase` invocation into the right typed error.
///
/// A forwarded Ctrl-C/SIGTERM kills git mid-rebase and leaves the worktree in
/// `REBASING` state, which is otherwise indistinguishable from a merge
/// conflict. The interrupt is surfaced as its exit code *before* the conflict
/// check, per the signal-handling policy in `CLAUDE.md`: otherwise a user who
/// aborts `wt merge` gets conflict-resolution guidance and a non-130 exit code
/// instead of a clean interrupt.
fn classify_rebase_failure(e: anyhow::Error, is_rebasing: bool, target: &str) -> anyhow::Error {
    if let Some(exit_code) = e.interrupt_exit_code() {
        return worktrunk::git::WorktrunkError::AlreadyDisplayed { exit_code }.into();
    }

    // Pull git's stderr from the typed leaf when present so we get the raw
    // conflict-marker bytes regardless of how many `.context(...)` layers wrap
    // the error.
    let detail = e.display_message();
    if is_rebasing {
        return worktrunk::git::GitError::RebaseConflict {
            target_branch: target.to_string(),
            git_output: detail,
        }
        .into();
    }
    worktrunk::git::GitError::Other {
        message: cformat!("Failed to rebase onto <bold>{target}</>: {detail}"),
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::classify_rebase_failure;
    use worktrunk::git::{ErrorExt, GitError, WorktrunkError};

    fn child_exit(code: i32, signal: Option<i32>) -> anyhow::Error {
        WorktrunkError::ChildProcessExited {
            code,
            message: "rebase failed".to_string(),
            signal,
        }
        .into()
    }

    #[test]
    fn interrupt_during_rebase_is_not_classified_as_conflict() {
        // SIGINT (2) forwarded to git leaves the worktree REBASING; it must
        // surface as a silent interrupt carrying exit 130, not a RebaseConflict
        // with resolution guidance. Before the interrupt check this returned a
        // GitError::RebaseConflict, whose exit_code() is None.
        let err = classify_rebase_failure(child_exit(130, Some(2)), true, "main");
        assert_eq!(err.exit_code(), Some(130));
        assert!(
            matches!(
                err.downcast_ref::<WorktrunkError>(),
                Some(WorktrunkError::AlreadyDisplayed { exit_code: 130 })
            ),
            "interrupt must surface as AlreadyDisplayed, not a rebase conflict"
        );
        assert!(
            !matches!(
                err.downcast_ref::<GitError>(),
                Some(GitError::RebaseConflict { .. })
            ),
            "interrupt must not be reclassified as a rebase conflict"
        );
    }

    #[test]
    fn conflict_in_rebasing_state_is_a_rebase_conflict() {
        let err = classify_rebase_failure(child_exit(1, None), true, "main");
        assert!(matches!(
            err.downcast_ref::<GitError>(),
            Some(GitError::RebaseConflict { .. })
        ));
    }

    #[test]
    fn non_rebasing_failure_is_other_error() {
        let err = classify_rebase_failure(child_exit(1, None), false, "main");
        assert!(matches!(
            err.downcast_ref::<GitError>(),
            Some(GitError::Other { .. })
        ));
    }
}
