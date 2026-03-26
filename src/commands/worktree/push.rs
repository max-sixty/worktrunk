//! Worktree push operations.
//!
//! Push changes to target branch with safety checks. Both fast-forward push and
//! `--no-ff` merge share common scaffolding (target resolution, fast-forward check,
//! stash guard, progress/success output) extracted into [`MergeContext`].

use std::path::PathBuf;

use anyhow::Context;
use color_print::cformat;
use worktrunk::git::{GitError, Repository};
use worktrunk::styling::{
    eprintln, format_with_gutter, info_message, progress_message, success_message, warning_message,
};

use super::types::MergeOperations;
use crate::commands::repository_ext::{RepositoryCliExt, TargetWorktreeStash};

// ---------------------------------------------------------------------------
// Shared scaffolding
// ---------------------------------------------------------------------------

/// Pre-computed state shared by both fast-forward push and `--no-ff` merge.
///
/// Created by [`MergeContext::prepare`], which resolves the target branch,
/// verifies fast-forward, sets up the stash guard, counts commits, and captures
/// diff statistics — all steps that are identical between the two strategies.
struct MergeContext {
    repo: Repository,
    target_branch: String,
    target_worktree_path: Option<PathBuf>,
    /// Snapshotted target SHA for TOCTOU-safe ref updates.
    target_tip: String,
    stash_guard: Option<TargetWorktreeStash>,
    commit_count: usize,
    stats_summary: Vec<String>,
}

impl MergeContext {
    /// Resolve target, verify fast-forward, stash guard, count commits, capture stats.
    fn prepare(target: Option<&str>, operations: Option<MergeOperations>) -> anyhow::Result<Self> {
        let repo = Repository::current()?;

        let target_branch = repo.require_target_branch(target)?;
        let target_worktree_path = repo.worktree_for_branch(&target_branch)?;

        // Snapshot target SHA early for TOCTOU safety (used by both strategies
        // for the fast-forward check; --no-ff also uses it for update-ref).
        let target_ref = format!("refs/heads/{}", target_branch);
        let target_tip = repo
            .run_command(&["rev-parse", &target_ref])?
            .trim()
            .to_string();

        // Fast-forward check (target must be ancestor of HEAD)
        if !repo.is_ancestor(&target_tip, "HEAD")? {
            let commits_formatted = repo
                .run_command(&[
                    "log",
                    "--color=always",
                    "--graph",
                    "--oneline",
                    &format!("HEAD..{}", target_tip),
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

        // Auto-stash non-overlapping changes in target worktree
        let stash_guard =
            repo.prepare_target_worktree(target_worktree_path.as_ref(), &target_branch)?;

        let commit_count = repo.count_commits(&target_branch, "HEAD")?;

        let stats_summary = if commit_count > 0 {
            repo.diff_stats_summary(&["diff", "--shortstat", &format!("{}..HEAD", target_branch)])
        } else {
            Vec::new()
        };

        Ok(Self {
            repo,
            target_branch,
            target_worktree_path,
            target_tip,
            stash_guard,
            commit_count,
            stats_summary,
        })
    }

    /// Print progress message, commit graph, and diff statistics.
    ///
    /// `verb_ing` is the gerund shown in the progress line (e.g. "Merging", "Pushing").
    /// `extra_note` is appended after the SHA (e.g. " (--no-ff)").
    fn show_progress(
        &self,
        verb_ing: &str,
        extra_note: &str,
        operations: Option<MergeOperations>,
    ) -> anyhow::Result<()> {
        if self.commit_count == 0 {
            return Ok(());
        }

        let commit_text = if self.commit_count == 1 {
            "commit"
        } else {
            "commits"
        };
        let head_sha = self.repo.run_command(&["rev-parse", "--short", "HEAD"])?;
        let head_sha = head_sha.trim();

        let operations_note = format_operations_note(operations);

        eprintln!(
            "{}",
            progress_message(cformat!(
                "{verb_ing} {} {commit_text} to <bold>{}</> @ <dim>{head_sha}</>{extra_note}{operations_note}",
                self.commit_count,
                self.target_branch,
            ))
        );

        // Commit graph
        let log_output = self.repo.run_command(&[
            "log",
            "--color=always",
            "--graph",
            "--oneline",
            &format!("{}..HEAD", self.target_branch),
        ])?;
        eprintln!("{}", format_with_gutter(&log_output, None));

        // Diff statistics
        crate::commands::show_diffstat(&self.repo, &format!("{}..HEAD", self.target_branch))?;

        Ok(())
    }

    /// Print "Already up to date" info message and return `true` if commit_count == 0.
    fn show_up_to_date_if_needed(&self, operations: Option<MergeOperations>) -> bool {
        if self.commit_count > 0 {
            return false;
        }

        let context = format_up_to_date_context(operations);
        eprintln!(
            "{}",
            info_message(cformat!(
                "Already up to date with <bold>{}</>{context}",
                self.target_branch,
            ))
        );
        true
    }

    /// Explicitly restore the stash guard (before success message).
    fn restore_stash(&mut self) {
        if let Some(guard) = self.stash_guard.as_mut() {
            guard.restore_now();
        }
    }

    /// Print success message with commit/file stats.
    ///
    /// `verb` is the past-tense action (e.g. "Merged to", "Pushed to").
    /// `sha_suffix` is an optional pre-formatted ANSI string shown after the branch
    /// (e.g. ` @ <dim>a1b2c3d</>`). Use `cformat!` at the call site.
    /// `extra_stats` are appended inside the stats parentheses (e.g. ", --no-ff").
    fn show_success(&self, verb: &str, sha_suffix: &str, extra_stats: &str) {
        let mut summary_parts = vec![format!(
            "{} commit{}",
            self.commit_count,
            if self.commit_count == 1 { "" } else { "s" }
        )];
        summary_parts.extend(self.stats_summary.clone());

        let stats_str = summary_parts.join(", ");
        let target_branch = &self.target_branch;
        let paren_close = cformat!("<bright-black>)</>"); // Separate to avoid cformat optimization
        eprintln!(
            "{}",
            success_message(cformat!(
                "{verb} <bold>{target_branch}</>{sha_suffix} <bright-black>({stats_str}{extra_stats}</>{paren_close}",
            ))
        );
    }
}

/// Format the "(no commit/squash/rebase needed)" parenthetical for progress messages.
fn format_operations_note(operations: Option<MergeOperations>) -> String {
    let Some(ops) = operations else {
        return String::new();
    };
    let mut skipped = Vec::new();
    if !ops.committed && !ops.squashed {
        skipped.push("commit/squash");
    }
    if !ops.rebased {
        skipped.push("rebase");
    }
    if skipped.is_empty() {
        String::new()
    } else {
        format!(" (no {} needed)", skipped.join("/"))
    }
}

/// Format the context string for "Already up to date" messages.
fn format_up_to_date_context(operations: Option<MergeOperations>) -> String {
    let Some(ops) = operations else {
        return String::new();
    };
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
}

// ---------------------------------------------------------------------------
// Fast-forward push
// ---------------------------------------------------------------------------

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
    let mut ctx = MergeContext::prepare(target, operations)?;

    let verb_ing = if verb.starts_with("Merged") {
        "Merging"
    } else {
        "Pushing"
    };
    ctx.show_progress(verb_ing, "", operations)?;

    // Perform the push via --receive-pack (atomically updates ref + working tree)
    let git_common_dir = ctx.repo.git_common_dir();
    let git_common_dir_str = git_common_dir.to_string_lossy();
    let push_target = format!("HEAD:{}", ctx.target_branch);

    ctx.repo
        .run_command(&[
            "push",
            "--recurse-submodules=no",
            "--receive-pack=git -c receive.denyCurrentBranch=updateInstead receive-pack",
            git_common_dir_str.as_ref(),
            &push_target,
        ])
        .map_err(|e| GitError::PushFailed {
            target_branch: ctx.target_branch.clone(),
            error: e.to_string(),
        })?;

    ctx.restore_stash();

    if ctx.commit_count > 0 {
        ctx.show_success(verb, "", "");
    } else {
        ctx.show_up_to_date_if_needed(operations);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// No-fast-forward merge
// ---------------------------------------------------------------------------

/// Merge to target branch using `--no-ff` (creates a merge commit).
///
/// Uses git plumbing (`commit-tree` + `update-ref`) to create a merge commit
/// on the target branch without needing to check it out. This is safe because
/// rebase has already run, so the feature branch tree IS the correct merge result.
///
/// If the target branch has a checked-out worktree, its working tree is synced
/// via `read-tree -m -u` after the ref update. We use a two-tree merge
/// (`read-tree -m -u <old> <new>`) instead of `reset --hard` because it refuses
/// to overwrite tracked files with local modifications (TOCTOU safety), while
/// `reset --hard` would silently discard them. (`reset --keep` can't be used
/// here because after `update-ref`, HEAD already points to the merge commit, so
/// `reset --keep HEAD` sees old==new and is a no-op.) The stash guard pattern
/// from `handle_push` is reused for dirty target worktrees.
pub fn handle_no_ff_merge(
    target: Option<&str>,
    operations: Option<MergeOperations>,
    feature_branch: &str,
) -> anyhow::Result<()> {
    let mut ctx = MergeContext::prepare(target, operations)?;

    ctx.show_progress("Merging", " (--no-ff)", operations)?;

    if ctx.show_up_to_date_if_needed(operations) {
        return Ok(());
    }

    // Create the merge commit using git plumbing.
    // Since rebase has already run, HEAD's tree is the correct merge result.
    let tree = ctx
        .repo
        .run_command(&["rev-parse", "HEAD^{tree}"])?
        .trim()
        .to_string();

    let feature_tip = ctx
        .repo
        .run_command(&["rev-parse", "HEAD"])?
        .trim()
        .to_string();

    let merge_message = format!(
        "Merge branch '{}' into {}",
        feature_branch, ctx.target_branch
    );

    let merge_sha = ctx
        .repo
        .run_command(&[
            "commit-tree",
            &tree,
            "-p",
            &ctx.target_tip,
            "-p",
            &feature_tip,
            "-m",
            &merge_message,
        ])
        .context("Failed to create merge commit")?
        .trim()
        .to_string();

    // Atomically update the target branch ref (with old-value check for safety)
    let target_ref = format!("refs/heads/{}", ctx.target_branch);
    ctx.repo
        .run_command(&["update-ref", &target_ref, &merge_sha, &ctx.target_tip])
        .map_err(|e| GitError::PushFailed {
            target_branch: ctx.target_branch.clone(),
            error: format!("Failed to update ref: {e}"),
        })?;

    // Sync the target worktree's working tree if it exists.
    // Use `read-tree -m -u` (two-tree merge) instead of `reset --hard` so that
    // any tracked-file modifications added between the pre-push dirty check and
    // now cause the sync to fail rather than silently discarding work (TOCTOU
    // safety). Note: `reset --keep` can't be used here because after
    // `update-ref`, HEAD already equals the merge commit, making it a no-op.
    // The merge is already done (ref updated), so treat sync failure as a warning.
    if let Some(wt_path) = &ctx.target_worktree_path
        && wt_path.exists()
    {
        let target_wt = ctx.repo.worktree_at(wt_path);
        if let Err(e) =
            target_wt.run_command(&["read-tree", "-m", "-u", &ctx.target_tip, &merge_sha])
        {
            eprintln!(
                "{}",
                warning_message(cformat!(
                    "Failed to sync target worktree; run <bold>git -C {} reset --hard HEAD</> manually",
                    worktrunk::path::format_path_for_display(wt_path)
                ))
            );
            log::warn!("Failed to sync target worktree: {e}");
        }
    }

    ctx.restore_stash();

    let merge_sha_short = &merge_sha[..merge_sha.len().min(7)];
    let sha_suffix = cformat!(" @ <dim>{merge_sha_short}</>");
    ctx.show_success("Merged to", &sha_suffix, ", --no-ff");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::worktree::types::MergeOperations;

    #[test]
    fn test_format_operations_note() {
        // None → empty
        assert_eq!(format_operations_note(None), "");

        // All operations happened → empty (nothing skipped)
        assert_eq!(
            format_operations_note(Some(MergeOperations {
                committed: true,
                squashed: true,
                rebased: true,
            })),
            ""
        );

        // Nothing happened → both skipped
        assert_eq!(
            format_operations_note(Some(MergeOperations {
                committed: false,
                squashed: false,
                rebased: false,
            })),
            " (no commit/squash/rebase needed)"
        );

        // Only rebase skipped
        assert_eq!(
            format_operations_note(Some(MergeOperations {
                committed: true,
                squashed: false,
                rebased: false,
            })),
            " (no rebase needed)"
        );

        // Only commit/squash skipped
        assert_eq!(
            format_operations_note(Some(MergeOperations {
                committed: false,
                squashed: false,
                rebased: true,
            })),
            " (no commit/squash needed)"
        );
    }

    #[test]
    fn test_format_up_to_date_context() {
        // None → empty
        assert_eq!(format_up_to_date_context(None), "");

        // All operations happened → empty
        assert_eq!(
            format_up_to_date_context(Some(MergeOperations {
                committed: true,
                squashed: true,
                rebased: true,
            })),
            ""
        );

        // Nothing happened → both noted
        assert_eq!(
            format_up_to_date_context(Some(MergeOperations {
                committed: false,
                squashed: false,
                rebased: false,
            })),
            " (no new commits, no rebase needed)"
        );

        // Only rebase not needed
        assert_eq!(
            format_up_to_date_context(Some(MergeOperations {
                committed: true,
                squashed: false,
                rebased: false,
            })),
            " (no rebase needed)"
        );

        // Only no new commits
        assert_eq!(
            format_up_to_date_context(Some(MergeOperations {
                committed: false,
                squashed: false,
                rebased: true,
            })),
            " (no new commits)"
        );
    }
}
