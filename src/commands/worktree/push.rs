//! Worktree push operations.
//!
//! Push changes to target branch with safety checks.

use color_print::cformat;
use worktrunk::git::{GitError, Repository};
use worktrunk::styling::{format_with_gutter, info_message, progress_message, success_message};

use super::types::MergeOperations;
use crate::commands::repository_ext::RepositoryCliExt;

/// Push changes to target branch
///
/// The `operations` parameter indicates which merge operations occurred (commit, squash, rebase).
/// Pass `None` for standalone push operations where these concepts don't apply.
///
/// During the push stage we temporarily `git stash` non-overlapping changes in the
/// target worktree (if present) so that concurrent edits there do not block the
/// fast-forward. The stash is restored afterward and we bail out early if any file
/// overlaps with the push range.
pub fn handle_push(
    target: Option<&str>,
    verb: &str,
    operations: Option<MergeOperations>,
) -> anyhow::Result<()> {
    let repo = Repository::current()?;

    // Get and validate target branch (must be a branch since we're updating it)
    let target_branch = repo.require_target_branch(target)?;

    // A worktree for the target branch is optional for push:
    // - If present, we use it to check for overlapping dirty files.
    // - If absent, we skip that safety step but still allow the push (git itself is fine).
    let target_worktree_path = repo.worktree_for_branch(&target_branch)?;

    // Check if it's a fast-forward
    if !repo.is_ancestor(&target_branch, "HEAD")? {
        // Get formatted commit log (commits in target that we don't have)
        let commits_formatted = repo
            .run_command(&[
                "log",
                "--color=always",
                "--graph",
                "--oneline",
                &format!("HEAD..{}", target_branch),
            ])?
            .trim()
            .to_string();

        return Err(GitError::NotFastForward {
            target_branch: target_branch.clone(),
            commits_formatted,
            in_merge_context: operations.is_some(),
        }
        .into());
    }

    // Check for conflicting changes in target worktree (auto-stash safe changes)
    // The stash guard auto-restores on drop (error paths), or explicitly via restore_now()
    let mut stash_guard =
        repo.prepare_target_worktree(target_worktree_path.as_ref(), &target_branch)?;

    // Count commits and show what will be pushed
    let commit_count = repo.count_commits(&target_branch, "HEAD")?;

    // Get diff statistics BEFORE push (will be needed for success message later)
    let stats_summary = if commit_count > 0 {
        repo.diff_stats_summary(&["diff", "--shortstat", &format!("{}..HEAD", target_branch)])
    } else {
        Vec::new()
    };

    // Build and show consolidated message with squash/rebase info
    if commit_count > 0 {
        let commit_text = if commit_count == 1 {
            "commit"
        } else {
            "commits"
        };
        let head_sha = repo.run_command(&["rev-parse", "--short", "HEAD"])?;
        let head_sha = head_sha.trim();

        let verb_ing = if verb.starts_with("Merged") {
            "Merging"
        } else {
            "Pushing"
        };

        // Build parenthetical showing which operations didn't happen and flags used
        let mut notes = Vec::new();

        // Skipped operations - only include if we're in merge workflow context
        if let Some(ops) = operations {
            let mut skipped_ops = Vec::new();
            if !ops.committed && !ops.squashed {
                // Neither commit nor squash happened - combine them
                skipped_ops.push("commit/squash");
            }
            if !ops.rebased {
                skipped_ops.push("rebase");
            }
            if !skipped_ops.is_empty() {
                notes.push(format!("no {} needed", skipped_ops.join("/")));
            }
        }

        let operations_note = if notes.is_empty() {
            String::new()
        } else {
            format!(" ({})", notes.join(", "))
        };

        crate::output::print(progress_message(cformat!(
            "{verb_ing} {commit_count} {commit_text} to <bold>{target_branch}</> @ <dim>{head_sha}</>{operations_note}"
        )))?;

        // Show the commit graph with color
        let log_output = repo.run_command(&[
            "log",
            "--color=always",
            "--graph",
            "--oneline",
            &format!("{}..HEAD", target_branch),
        ])?;
        crate::output::print(format_with_gutter(&log_output, None))?;

        // Show diff statistics
        crate::commands::show_diffstat(&repo, &format!("{}..HEAD", target_branch))?;
    }

    // Get git common dir for the push
    let git_common_dir = repo.git_common_dir();
    let git_common_dir_str = git_common_dir.to_string_lossy();

    // Perform the push - stash guard will auto-restore on any exit path
    // Use --receive-pack to pass config to the receiving end without permanently mutating repo config
    let push_target = format!("HEAD:{}", target_branch);
    repo.run_command(&[
        "push",
        "--receive-pack=git -c receive.denyCurrentBranch=updateInstead receive-pack",
        git_common_dir_str.as_ref(),
        &push_target,
    ])
    .map_err(|e| {
        // CommandFailed contains raw git output, wrap in PushFailed for proper formatting
        GitError::PushFailed {
            target_branch: target_branch.clone(),
            error: e.to_string(),
        }
    })?;

    // Restore stash before success message (Drop handles error paths automatically)
    if let Some(guard) = stash_guard.as_mut() {
        guard.restore_now();
    }

    // Show success message after push completes
    if commit_count > 0 {
        // Use the diff statistics captured earlier (before push)
        let mut summary_parts = vec![format!(
            "{} commit{}",
            commit_count,
            if commit_count == 1 { "" } else { "s" }
        )];
        summary_parts.extend(stats_summary);

        // Re-apply bright-black after stats (which end with a reset) so ) is also gray
        let stats_str = summary_parts.join(", ");
        let paren_close = cformat!("<bright-black>)</>"); // Separate to avoid cformat optimization
        crate::output::print(success_message(cformat!(
            "{verb} <bold>{target_branch}</> <bright-black>({stats_str}</>{}",
            paren_close
        )))?;
    } else {
        // For merge workflow context, explain why nothing was pushed
        let context = if let Some(ops) = operations {
            let mut notes = Vec::new();
            if !ops.committed && !ops.squashed {
                notes.push("no new commits");
            }
            if !ops.rebased {
                notes.push("no rebase needed");
            }
            if notes.is_empty() {
                String::new()
            } else {
                format!(" ({})", notes.join(", "))
            }
        } else {
            String::new()
        };

        // No action: nothing was pushed, just acknowledging state
        crate::output::print(info_message(cformat!(
            "Already up to date with <bold>{target_branch}</>{context}"
        )))?;
    }

    Ok(())
}
