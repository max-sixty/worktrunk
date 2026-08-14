//! Worktree push operations.
//!
//! Push changes to target branch with safety checks. Both fast-forward push and
//! `--no-ff` merge share common scaffolding (target resolution, fast-forward check,
//! conflict check, progress/success output) extracted into [`MergeContext`], and
//! both land through [`advance_target`].
//!
//! # Destination-worktree safety follows git
//!
//! Uncommitted changes in the target worktree stay in place. The worktree sync
//! is a two-tree merge (`read-tree -m -u`), which carries a dirty path the
//! push range doesn't touch — staged entries included — and refuses one it
//! does. The refusal is fronted by a check over `git status --porcelain` that
//! names the conflicting files before anything moves. Ignored files fall
//! outside that report, so a push overwrites one whose path the incoming
//! commits track.
//!
//! That is git's own line, not an oversight: the two-tree merge runs git's
//! unpack-trees checks, which refuse to clobber a modified or untracked file
//! and silently overwrite an ignored one — the same split a `git merge` run
//! there would produce, and the same update the `push-to-checkout` hook's
//! documented lenient policy performs (githooks(5)). Matching git is the
//! decision; don't add an ignored-file probe to make `wt` stricter than the
//! tool it wraps.

use std::path::PathBuf;

use anyhow::Context;
use color_print::cformat;
use worktrunk::git::{ErrorExt, GitError, Repository};
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{
    eprintln, format_with_gutter, info_message, progress_message, success_message, warning_message,
};

use super::types::MergeOperations;
use crate::commands::repository_ext::RepositoryCliExt;

/// Distinguishes a standalone push from a fast-forward push driven by `wt merge`.
///
/// Carried into [`handle_push`] so progress/success messages use the right verb
/// without sniffing a passed-in string.
#[derive(Debug, Clone, Copy)]
pub enum PushKind {
    /// `wt push` — no merge operations precede the push.
    Standalone,
    /// `wt merge` resolved to a fast-forward.
    MergeFastForward,
}

impl PushKind {
    fn verb_past(self) -> &'static str {
        match self {
            PushKind::Standalone => "Pushed to",
            PushKind::MergeFastForward => "Merged to",
        }
    }

    fn verb_progressive(self) -> &'static str {
        match self {
            PushKind::Standalone => "Pushing",
            PushKind::MergeFastForward => "Merging",
        }
    }

    fn reflog_message(self) -> &'static str {
        match self {
            PushKind::Standalone => "wt step push: fast-forward",
            PushKind::MergeFastForward => "wt merge: fast-forward",
        }
    }
}

/// Outcome of a push or no-ff merge, returned for JSON output.
pub struct PushResult {
    pub target: String,
    pub commit_count: usize,
    pub outcome: PushOutcome,
}

pub enum PushOutcome {
    /// Target was fast-forwarded to HEAD.
    FastForwarded,
    /// Target already contained HEAD; nothing to push.
    UpToDate,
    /// A new merge commit was created on the target branch.
    MergeCommit { merge_sha: String },
}

// ---------------------------------------------------------------------------
// Shared scaffolding
// ---------------------------------------------------------------------------

/// Pre-computed state shared by both fast-forward push and `--no-ff` merge.
///
/// Created by [`MergeContext::prepare`], which resolves the target branch,
/// verifies fast-forward, checks for conflicting target-worktree changes,
/// counts commits, and captures diff statistics — all steps that are identical
/// between the two strategies.
struct MergeContext {
    repo: Repository,
    target_branch: String,
    target_worktree_path: Option<PathBuf>,
    /// Snapshotted target SHA for TOCTOU-safe ref updates.
    target_tip: String,
    /// HEAD of the source worktree, resolved once for the ancestry check and
    /// the fast-forward ref update.
    head_sha: String,
    commit_count: usize,
    stats_summary: Vec<String>,
}

impl MergeContext {
    /// Resolve target, verify fast-forward, check conflicts, count commits, capture stats.
    fn prepare(target: Option<&str>, operations: Option<MergeOperations>) -> anyhow::Result<Self> {
        let repo = Repository::current()?;

        // Refuse before reading ancestry: mid-rebase HEAD is detached partway
        // through the replay, and it *is* a linear extension of the target, so
        // the fast-forward check below passes and the push carries the target
        // branch onto a half-replayed history whose worktree still holds
        // conflict markers — leaving the rebase open behind it.
        repo.ensure_no_operation_in_progress("push")?;

        let target_branch = repo.require_target_branch(target)?;
        // A registered worktree git calls prunable can't receive the sync in
        // `advance_target` — git dies trying to cd into it. Refusing upfront
        // gives a clear answer, and names the `git worktree prune` that clears
        // the registration.
        let target_worktree_path = repo.usable_worktree_for_branch(&target_branch)?;

        if let Some(path) = &target_worktree_path {
            // The target gets the same gate the source got above, for a
            // reason the source's comment doesn't cover: `advance_target`
            // syncs the target worktree with `read-tree -m -u`, which refuses
            // an unmerged index but not an open operation whose index is
            // momentarily clean. A target paused mid-rebase or mid-cherry-pick
            // between steps would have its files moved under it, and its
            // `--continue` would then commit the synced tree as the step's
            // result. The fast-forward's former
            // `receive.denyCurrentBranch=updateInstead` refused any unclean
            // target outright, so the plumbing used to supply this; asking
            // directly keeps the guarantee now that the sync is ours.
            if repo.worktree_at(path).operation_in_progress()?.is_some() {
                return Err(GitError::OperationInProgress {
                    action: "push".to_string(),
                    branch: Some(target_branch),
                }
                .into());
            }
        }

        // Snapshot target SHA early for TOCTOU safety (used by both strategies
        // for the fast-forward check; --no-ff also uses it for update-ref).
        let target_ref = format!("refs/heads/{}", target_branch);
        let target_tip = repo
            .run_command(&["rev-parse", &target_ref])?
            .trim()
            .to_string();

        // Fast-forward check (target must be ancestor of HEAD).
        // target_tip is already a SHA; resolve HEAD to one too so the
        // ancestry probe hits the SHA-keyed cache directly.
        let head_sha = repo.run_command(&["rev-parse", "HEAD"])?.trim().to_string();
        if !repo.is_ancestor_by_sha(&target_tip, &head_sha)? {
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

        // Refuse when uncommitted changes in the target worktree overlap the
        // push range. Non-overlapping changes stay in place: the two-tree
        // merge in `advance_target` carries them through untouched.
        repo.ensure_no_target_conflicts(target_worktree_path.as_ref(), &target_branch)?;

        // TODO(#3519 follow-up): when `target_branch` was behind its upstream
        // (see `Repository::span_upstream`), this count mixes the carried
        // fast-forwarded upstream commits in with the branch's own squash
        // commit, so the success line ("Merged to main (N commits, ...)")
        // overstates what the branch itself contributed. Splitting them needs
        // a carried-count threaded through `MergeContext`; deferred as
        // cosmetic.
        let commit_count = repo.count_commits(&target_branch, "HEAD")?;

        let stats_summary = if commit_count > 0 {
            repo.diff_stats_summary(&[
                "diff",
                "--shortstat",
                "--end-of-options",
                &format!("{}..HEAD", target_branch),
            ])
        } else {
            Vec::new()
        };

        Ok(Self {
            repo,
            target_branch,
            target_worktree_path,
            target_tip,
            head_sha,
            commit_count,
            stats_summary,
        })
    }

    /// Print progress message, commit graph, and diff statistics.
    ///
    /// `verb_progressive` is the present participle shown in the progress line
    /// (e.g. "Merging", "Pushing"). `extra_note` is appended after the SHA
    /// (e.g. " (--no-ff)").
    fn show_progress(
        &self,
        verb_progressive: &str,
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
        let head_sha = self.repo.short_sha(&self.head_sha)?;

        let operations_note = format_operations_note(operations);

        eprintln!(
            "{}",
            progress_message(cformat!(
                "{verb_progressive} {} {commit_text} to <bold>{}</> @ <dim>{head_sha}</>{extra_note}{operations_note}",
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
            "--end-of-options",
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
// Target update
// ---------------------------------------------------------------------------

/// Advance `target_branch` to `new_sha` and sync its checked-out worktree.
///
/// The worktrees share one object store, so nothing needs transferring: the
/// update is a compare-and-swap `update-ref` against `old_sha`, which fails
/// cleanly if another process advanced the target since
/// [`MergeContext::prepare`] snapshotted it.
///
/// The worktree sync is git's documented lenient `push-to-checkout` policy
/// (githooks(5)): `update-index --refresh` then a two-tree merge
/// (`read-tree -m -u`). The two-tree merge leaves uncommitted changes at paths
/// the update doesn't touch exactly where they are — staged entries stay
/// staged — and refuses the update when a dirty path overlaps it. (`reset
/// --keep` can't express this sync: HEAD already points at `new_sha` through
/// the branch, so it sees old == new and is a no-op.) On refusal the ref
/// update is rolled back with a second compare-and-swap, so the branch and
/// its worktree move together or not at all.
///
/// `reflog_message` labels the target branch's reflog entry — the recovery
/// affordance for the one operation that moves a branch the user isn't
/// standing on, where every other mover (`push`, `merge`, `reset`) names
/// itself.
fn advance_target(
    repo: &Repository,
    target_branch: &str,
    target_worktree_path: Option<&PathBuf>,
    old_sha: &str,
    new_sha: &str,
    reflog_message: &str,
) -> anyhow::Result<()> {
    let target_ref = format!("refs/heads/{target_branch}");
    repo.run_command(&[
        "update-ref",
        "-m",
        reflog_message,
        &target_ref,
        new_sha,
        old_sha,
    ])
    .map_err(|e| GitError::PushFailed {
        target_branch: target_branch.to_string(),
        error: format!("Failed to update ref: {}", e.display_message()),
    })?;

    let Some(wt_path) = target_worktree_path else {
        return Ok(());
    };
    let target_wt = repo.worktree_at(wt_path);
    // Refresh first: `read-tree -m -u` trusts index stat data, so an entry
    // that is merely stat-dirty would otherwise refuse as "not uptodate".
    // `submodule.recurse=false`: the sync moves the superproject only —
    // recursing into submodules re-introduces #1604's failure mode when the
    // user has `submodule.recurse=true` (read-tree is on git's always-recurse
    // list).
    let sync_result = target_wt
        .run_command(&["update-index", "-q", "--refresh"])
        .and_then(|_| {
            target_wt.run_command(&[
                "-c",
                "submodule.recurse=false",
                "read-tree",
                "-m",
                "-u",
                old_sha,
                new_sha,
            ])
        });
    let e = match sync_result {
        Ok(_) => {
            // The compare-and-swap guarded the ref update, not the sync that
            // follows it: a commit racing into the target worktree inside
            // that window is built against the pre-sync index, so its tree
            // reverts the pushed content while keeping it in the ancestry — a
            // state the old fast-forward path couldn't reach because
            // receive-pack re-verifies the expected old value when committing
            // the ref. Re-read the ref so that interleaving surfaces instead
            // of being reported as a clean merge. The push range is in the
            // tip's ancestry either way, so this warns rather than fails.
            let tip_after = repo.run_command(&["rev-parse", &target_ref])?;
            if tip_after.trim() != new_sha {
                eprintln!(
                    "{}",
                    warning_message(cformat!(
                        "<bold>{target_branch}</> moved while its worktree was being synced; check <bold>git -C {} status</>",
                        format_path_for_display(wt_path)
                    ))
                );
            }
            return Ok(());
        }
        Err(e) => e,
    };

    // The sync refused — a conflicting change appeared in the race window
    // after the upfront check, or the index is locked; git's own error names
    // the cause. unpack-trees runs its refusal checks before writing anything,
    // so the worktree is normally untouched — only a write error partway
    // through the update phase (ENOSPC, a read-only file) can leave a subset
    // of files already carrying the new content, and git's error says which
    // write failed. Either way, put the ref back so the branch and its
    // worktree stay consistent.
    let context = match repo.run_command(&[
        "update-ref",
        "-m",
        "wt: rollback (worktree sync failed)",
        &target_ref,
        old_sha,
        new_sha,
    ]) {
        Ok(_) => {
            format!("Syncing the {target_branch} worktree failed; the ref change was rolled back")
        }
        // Only a third writer moving the ref inside this window can land here;
        // report the state that remains rather than guessing at a fix.
        Err(rollback_err) => format!(
            "Syncing the {target_branch} worktree failed, and the ref change could not be \
             rolled back ({rollback_err}); {target_branch} has advanced but its worktree \
             was not synced"
        ),
    };
    Err(e).context(context)
}

// ---------------------------------------------------------------------------
// Fast-forward push
// ---------------------------------------------------------------------------

/// Push changes to target branch
///
/// The `operations` parameter indicates which merge operations occurred (commit, squash, rebase).
/// Pass `None` for standalone push operations where these concepts don't apply.
///
/// Uncommitted changes in the target worktree don't move: [`advance_target`]'s
/// two-tree merge carries them in place, and [`MergeContext::prepare`] already
/// refused any that overlap the push range.
pub fn handle_push(
    target: Option<&str>,
    kind: PushKind,
    operations: Option<MergeOperations>,
) -> anyhow::Result<PushResult> {
    let ctx = MergeContext::prepare(target, operations)?;

    ctx.show_progress(kind.verb_progressive(), "", operations)?;

    if ctx.commit_count == 0 {
        // The ancestry check passed and no commits are in target..HEAD, so
        // HEAD == target tip and there is nothing to advance.
        ctx.show_up_to_date_if_needed(operations);
        return Ok(PushResult {
            target: ctx.target_branch,
            commit_count: 0,
            outcome: PushOutcome::UpToDate,
        });
    }

    advance_target(
        &ctx.repo,
        &ctx.target_branch,
        ctx.target_worktree_path.as_ref(),
        &ctx.target_tip,
        &ctx.head_sha,
        kind.reflog_message(),
    )?;

    ctx.show_success(kind.verb_past(), "", "");
    Ok(PushResult {
        target: ctx.target_branch,
        commit_count: ctx.commit_count,
        outcome: PushOutcome::FastForwarded,
    })
}

// ---------------------------------------------------------------------------
// No-fast-forward merge
// ---------------------------------------------------------------------------

/// Merge to target branch using `--no-ff` (creates a merge commit).
///
/// Uses git plumbing (`commit-tree` + [`advance_target`]) to create a merge
/// commit on the target branch without needing to check it out. This is safe
/// because [`MergeContext::prepare`] verified that the target is an ancestor
/// of the feature tip, so the feature tree is the correct integration result.
/// The source may be rebased or may retain an explicitly preserved
/// merge-shaped graph.
pub fn handle_no_ff_merge(
    target: Option<&str>,
    operations: Option<MergeOperations>,
    feature_branch: &str,
) -> anyhow::Result<PushResult> {
    let ctx = MergeContext::prepare(target, operations)?;

    ctx.show_progress("Merging", " (--no-ff)", operations)?;

    if ctx.commit_count == 0 {
        ctx.show_up_to_date_if_needed(operations);
        return Ok(PushResult {
            target: ctx.target_branch,
            commit_count: 0,
            outcome: PushOutcome::UpToDate,
        });
    }

    // Create the merge commit using git plumbing.
    // The target-is-ancestor check makes HEAD's tree the correct merge result,
    // whether the source was rebased or explicitly preserved.
    let tree = ctx
        .repo
        .run_command(&["rev-parse", &format!("{}^{{tree}}", ctx.head_sha)])?
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
            &ctx.head_sha,
            "-m",
            &merge_message,
        ])
        .context("Failed to create merge commit")?
        .trim()
        .to_string();

    advance_target(
        &ctx.repo,
        &ctx.target_branch,
        ctx.target_worktree_path.as_ref(),
        &ctx.target_tip,
        &merge_sha,
        if operations.is_some() {
            "wt merge: no-ff"
        } else {
            "wt step push: no-ff"
        },
    )?;

    // Display uses `Repository::short_sha`; the JSON payload carries the full SHA.
    let merge_sha_short = ctx.repo.short_sha(&merge_sha)?;
    let sha_suffix = cformat!(" @ <dim>{merge_sha_short}</>");
    ctx.show_success("Merged to", &sha_suffix, ", --no-ff");

    Ok(PushResult {
        target: ctx.target_branch,
        commit_count: ctx.commit_count,
        outcome: PushOutcome::MergeCommit { merge_sha },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::worktree::types::MergeOperations;
    use worktrunk::testing::TestRepo;

    /// `advance_target` moves the branch and worktree together while leaving
    /// non-overlapping uncommitted changes — staged entries included — exactly
    /// in place, and refuses atomically otherwise: an overlapping dirty file
    /// rolls the ref update back, and a stale `old_sha` (a concurrently
    /// advanced target) fails the compare-and-swap before anything moves.
    /// These paths have no deterministic trigger through the CLI — the upfront
    /// conflict check catches everything slower than a race — so they are
    /// proven here.
    #[test]
    fn advance_target_carries_dirty_files_and_refuses_atomically() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();
        let root = test.root_path().to_path_buf();
        std::fs::write(root.join("x.txt"), "base-x").unwrap();
        test.run_git(&["add", "x.txt"]);
        test.run_git(&["commit", "-m", "add x"]);
        let old = test.git_output(&["rev-parse", "HEAD"]);

        // Build the incoming commit on a side branch, then return to main so
        // the worktree sits at `old` with a clean tree.
        test.run_git(&["switch", "-c", "incoming"]);
        std::fs::write(root.join("x.txt"), "incoming-x").unwrap();
        test.run_git(&["commit", "-am", "change x"]);
        let new = test.git_output(&["rev-parse", "HEAD"]);
        test.run_git(&["switch", "main"]);

        // Non-overlapping uncommitted state: an unstaged edit, a staged new
        // file, and an untracked file.
        std::fs::write(root.join("file.txt"), "unstaged-edit").unwrap();
        std::fs::write(root.join("staged.txt"), "staged").unwrap();
        test.run_git(&["add", "staged.txt"]);
        std::fs::write(root.join("untracked.txt"), "untracked").unwrap();

        advance_target(&repo, "main", Some(&root), &old, &new, "test").unwrap();
        assert_eq!(test.git_output(&["rev-parse", "main"]), new);
        assert_eq!(
            std::fs::read_to_string(root.join("x.txt")).unwrap(),
            "incoming-x"
        );
        // Unstaged edit still unstaged, staged entry still staged (and only
        // it), untracked file still untracked.
        assert_eq!(test.git_output(&["diff", "--name-only"]), "file.txt");
        assert_eq!(
            test.git_output(&["diff", "--cached", "--name-only"]),
            "staged.txt"
        );
        assert_eq!(
            test.git_output(&["ls-files", "--others", "--exclude-standard"]),
            "untracked.txt"
        );

        // Overlap: a dirty edit to a file the next update changes. The sync
        // refuses and the ref update is rolled back.
        test.run_git(&["switch", "-c", "incoming2", "incoming"]);
        std::fs::write(root.join("x.txt"), "incoming2-x").unwrap();
        test.run_git(&["commit", "-am", "change x again"]);
        let new2 = test.git_output(&["rev-parse", "HEAD"]);
        test.run_git(&["switch", "main"]);
        std::fs::write(root.join("x.txt"), "local-edit").unwrap();

        let err = advance_target(&repo, "main", Some(&root), &new, &new2, "test").unwrap_err();
        assert!(
            err.to_string().contains("rolled back"),
            "unexpected error: {err:#}"
        );
        assert_eq!(test.git_output(&["rev-parse", "main"]), new);
        assert_eq!(
            std::fs::read_to_string(root.join("x.txt")).unwrap(),
            "local-edit"
        );

        // Stale old_sha: the compare-and-swap fails before anything moves.
        let err = advance_target(&repo, "main", Some(&root), &old, &new2, "test").unwrap_err();
        assert!(
            err.to_string().contains("Can't push"),
            "unexpected error: {err:#}"
        );
        assert_eq!(test.git_output(&["rev-parse", "main"]), new);
    }

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
