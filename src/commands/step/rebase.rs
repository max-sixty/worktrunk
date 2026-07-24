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
    // #3519: when the branch's history extends past the local target into the
    // target's upstream, rebase onto the upstream instead — replaying the span
    // onto the stale local ref would duplicate commits the upstream already
    // contains under new SHAs.
    let integration_target = repo
        .span_upstream(&integration_target)?
        .unwrap_or(integration_target);

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
/// conflict. The interrupt is surfaced as `Interrupted` *before* the conflict
/// check, per the signal-handling policy in `CLAUDE.md`: otherwise a user who
/// aborts `wt merge` gets conflict-resolution guidance and a non-130 exit code
/// instead of a clean interrupt. When the kill left the worktree mid-rebase,
/// the interrupt carries a recovery hint — git ran in capture mode, so none
/// of its output was shown, and without the hint the otherwise-clean exit
/// would hide the `REBASING` state left behind.
fn classify_rebase_failure(e: anyhow::Error, is_rebasing: bool, target: &str) -> anyhow::Error {
    if let Some(signal) = e.interrupt_signal() {
        let hint = is_rebasing.then(|| {
            cformat!(
                "Rebase onto <underline>{target}</> left in progress; to abort, run <underline>git rebase --abort</>"
            )
        });
        return worktrunk::git::WorktrunkError::Interrupted { signal, hint }.into();
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
    use worktrunk::git::WorktrunkError::Interrupted;
    use worktrunk::git::{CommandError, ErrorExt, GitError, WorktrunkError};

    fn child_exit(code: i32, signal: Option<i32>) -> anyhow::Error {
        WorktrunkError::ChildProcessExited {
            code,
            message: "rebase failed".to_string(),
            signal,
        }
        .into()
    }

    /// The error shape `run_command` (capture mode) actually returns on a
    /// signal-killed git: a `CommandError` carrying `status.signal()`, wrapped
    /// by `run_command`'s `.context("Failed to execute: git …")` layer.
    fn capture_signal_failure(signal: i32) -> anyhow::Error {
        use anyhow::Context;
        Err::<(), _>(CommandError {
            program: "git".into(),
            args: vec!["rebase".into()],
            stderr: String::new(),
            stdout: String::new(),
            exit_code: None,
            signal: Some(signal),
        })
        .context("Failed to execute: git rebase")
        .unwrap_err()
    }

    #[test]
    fn capture_mode_interrupt_is_not_classified_as_conflict() {
        // This is the real path: `repo.run_command(["rebase", …])` returns a
        // CommandError (not ChildProcessExited), so the classifier must recover
        // the signal from the CommandError's `signal` field. With `is_rebasing`
        // true (git left the worktree REBASING), the old code fell through to
        // RebaseConflict; the interrupt must win.
        let err = classify_rebase_failure(capture_signal_failure(2), true, "main");
        assert_eq!(err.exit_code(), Some(130));
        // Downcasts are bound (and the variant imported) so each `matches!`
        // stays on one line: a passing positive `matches!` never runs its
        // generated false arm, and with a multi-line opener that region makes
        // the opener line read as missed in patch coverage.
        let wt_err = err.downcast_ref::<WorktrunkError>();
        assert!(
            matches!(wt_err, Some(Interrupted { signal: 2, .. })),
            "capture-mode interrupt must surface as Interrupted, not a rebase conflict"
        );
        // Mid-rebase, the interrupt carries the recovery hint for the
        // REBASING state it leaves behind.
        let Some(Interrupted {
            hint: Some(hint), ..
        }) = wt_err
        else {
            panic!("mid-rebase interrupt must carry a recovery hint, got {wt_err:?}");
        };
        assert!(
            hint.contains("git rebase --abort"),
            "hint must name the recovery command, got: {hint}"
        );
        let git_err = err.downcast_ref::<GitError>();
        assert!(
            !matches!(git_err, Some(GitError::RebaseConflict { .. })),
            "capture-mode interrupt must not be reclassified as a rebase conflict"
        );
    }

    #[test]
    fn capture_mode_crash_signal_stays_a_visible_error() {
        // SIGSEGV/SIGKILL can't be wt's own doing in capture mode — no signal
        // forwarding or escalation exists there — so the child crashed or was
        // killed externally. That must surface as a visible error (mid-rebase,
        // the REBASING state makes it a RebaseConflict, whose rendering
        // carries the --abort hint), never a silent interrupt exit.
        let crashed = classify_rebase_failure(capture_signal_failure(11), true, "main");
        let git_err = crashed.downcast_ref::<GitError>();
        assert!(
            matches!(git_err, Some(GitError::RebaseConflict { .. })),
            "a crashed git mid-rebase must surface, not exit silently"
        );

        let killed = classify_rebase_failure(capture_signal_failure(9), false, "main");
        assert!(matches!(
            killed.downcast_ref::<GitError>(),
            Some(GitError::Other { .. })
        ));
    }

    #[test]
    fn interrupt_during_rebase_is_not_classified_as_conflict() {
        // SIGINT (2) forwarded to git leaves the worktree REBASING; it must
        // surface as an interrupt carrying exit 130, not a RebaseConflict
        // with resolution guidance. Before the interrupt check this returned a
        // GitError::RebaseConflict, whose exit_code() is None.
        let err = classify_rebase_failure(child_exit(130, Some(2)), true, "main");
        assert_eq!(err.exit_code(), Some(130));
        let wt_err = err.downcast_ref::<WorktrunkError>();
        assert!(
            matches!(wt_err, Some(Interrupted { signal: 2, .. })),
            "interrupt must surface as Interrupted, not a rebase conflict"
        );
        let git_err = err.downcast_ref::<GitError>();
        assert!(
            !matches!(git_err, Some(GitError::RebaseConflict { .. })),
            "interrupt must not be reclassified as a rebase conflict"
        );

        // Outside REBASING state there's nothing to recover — no hint.
        let err = classify_rebase_failure(child_exit(130, Some(2)), false, "main");
        let wt_err = err.downcast_ref::<WorktrunkError>();
        assert!(
            matches!(wt_err, Some(Interrupted { hint: None, .. })),
            "interrupt outside REBASING must carry no hint, got {wt_err:?}"
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
