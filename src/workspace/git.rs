//! Git implementation of the [`Workspace`] trait.
//!
//! Implements [`Workspace`] directly on [`Repository`], mapping git-specific
//! types to the VCS-agnostic [`WorkspaceItem`] and [`Workspace`] interface.
//!
//! Commands that need git-specific features (staging, interactive squash)
//! can downcast via `workspace.as_any().downcast_ref::<Repository>()`.

use std::any::Any;
use std::path::{Path, PathBuf};

use anyhow::Context;
use color_print::cformat;

use crate::git::{
    GitError, Repository, check_integration, compute_integration_lazy, parse_porcelain_z,
};
use crate::path::format_path_for_display;

use super::types::{IntegrationReason, LineDiff, path_dir_name};
use crate::styling::{
    GUTTER_OVERHEAD, eprintln, format_with_gutter, get_terminal_width, progress_message,
    warning_message,
};

use super::{PushResult, RebaseOutcome, SquashOutcome, VcsKind, Workspace, WorkspaceItem};

impl Workspace for Repository {
    fn kind(&self) -> VcsKind {
        VcsKind::Git
    }

    fn list_workspaces(&self) -> anyhow::Result<Vec<WorkspaceItem>> {
        let worktrees = self.list_worktrees()?;
        let primary_path = self.primary_worktree()?;

        Ok(worktrees
            .into_iter()
            .map(|wt| {
                let is_default = primary_path
                    .as_ref()
                    .is_some_and(|primary| *primary == wt.path);
                WorkspaceItem::from_worktree(wt, is_default)
            })
            .collect())
    }

    fn workspace_path(&self, name: &str) -> anyhow::Result<PathBuf> {
        // Single pass: list worktrees once, check both branch name and dir name
        let worktrees = self.list_worktrees()?;

        // Prefer branch name match
        if let Some(wt) = worktrees
            .iter()
            .find(|wt| wt.branch.as_deref() == Some(name))
        {
            return Ok(wt.path.clone());
        }

        // Fall back to directory name match
        worktrees
            .iter()
            .find(|wt| path_dir_name(&wt.path) == name)
            .map(|wt| wt.path.clone())
            .ok_or_else(|| anyhow::anyhow!("No workspace found for name: {name}"))
    }

    fn default_workspace_path(&self) -> anyhow::Result<Option<PathBuf>> {
        self.primary_worktree()
    }

    fn default_branch_name(&self) -> Option<String> {
        self.default_branch()
    }

    fn is_dirty(&self, path: &Path) -> anyhow::Result<bool> {
        self.worktree_at(path).is_dirty()
    }

    fn working_diff(&self, path: &Path) -> anyhow::Result<LineDiff> {
        self.worktree_at(path).working_tree_diff_stats()
    }

    fn ahead_behind(&self, base: &str, head: &str) -> anyhow::Result<(usize, usize)> {
        // Note: this calls Repository::ahead_behind inherent method directly
        Repository::ahead_behind(self, base, head)
    }

    fn is_integrated(&self, id: &str, target: &str) -> anyhow::Result<Option<IntegrationReason>> {
        let signals = compute_integration_lazy(self, id, target)?;
        Ok(check_integration(&signals))
    }

    fn branch_diff_stats(&self, base: &str, head: &str) -> anyhow::Result<LineDiff> {
        Repository::branch_diff_stats(self, base, head)
    }

    fn create_workspace(&self, name: &str, base: Option<&str>, path: &Path) -> anyhow::Result<()> {
        self.create_worktree(name, base, path)
    }

    fn remove_workspace(&self, name: &str) -> anyhow::Result<()> {
        let path = Workspace::workspace_path(self, name)?;
        self.remove_worktree(&path, false)
    }

    fn resolve_integration_target(&self, target: Option<&str>) -> anyhow::Result<String> {
        self.require_target_ref(target)
    }

    fn is_rebased_onto(&self, target: &str, _path: &Path) -> anyhow::Result<bool> {
        // Call the inherent method via fully-qualified syntax
        Repository::is_rebased_onto(self, target)
    }

    fn rebase_onto(&self, target: &str, _path: &Path) -> anyhow::Result<RebaseOutcome> {
        // Detect fast-forward: merge-base == HEAD means HEAD is behind target
        let merge_base = self
            .merge_base("HEAD", target)?
            .context("Cannot rebase: no common ancestor with target branch")?;
        let head_sha = self.run_command(&["rev-parse", "HEAD"])?.trim().to_string();
        let is_fast_forward = merge_base == head_sha;

        // Only show progress for true rebases (fast-forwards are instant)
        if !is_fast_forward {
            eprintln!(
                "{}",
                progress_message(cformat!("Rebasing onto <bold>{target}</>..."))
            );
        }

        let rebase_result = self.run_command(&["rebase", target]);

        if let Err(e) = rebase_result {
            let is_rebasing = self
                .worktree_state()?
                .is_some_and(|s| s.starts_with("REBASING"));
            if is_rebasing {
                let git_output = e.to_string();
                return Err(crate::git::GitError::RebaseConflict {
                    target_branch: target.to_string(),
                    git_output,
                }
                .into());
            }
            return Err(crate::git::GitError::Other {
                message: format!("Failed to rebase onto {target}: {e}"),
            }
            .into());
        }

        // Verify rebase completed successfully
        if self.worktree_state()?.is_some() {
            return Err(crate::git::GitError::RebaseConflict {
                target_branch: target.to_string(),
                git_output: String::new(),
            }
            .into());
        }

        if is_fast_forward {
            Ok(RebaseOutcome::FastForward)
        } else {
            Ok(RebaseOutcome::Rebased)
        }
    }

    fn root_path(&self) -> anyhow::Result<PathBuf> {
        Ok(self.repo_path().to_path_buf())
    }

    fn current_workspace_path(&self) -> anyhow::Result<PathBuf> {
        Ok(self.current_worktree().path().to_path_buf())
    }

    fn current_name(&self, path: &Path) -> anyhow::Result<Option<String>> {
        self.worktree_at(path).branch()
    }

    fn project_identifier(&self) -> anyhow::Result<String> {
        // Call the inherent method via fully-qualified syntax
        Repository::project_identifier(self)
    }

    fn commit(&self, message: &str, path: &Path) -> anyhow::Result<String> {
        self.worktree_at(path)
            .run_command(&["commit", "-m", message])
            .context("Failed to create commit")?;
        let sha = self
            .worktree_at(path)
            .run_command(&["rev-parse", "HEAD"])?
            .trim()
            .to_string();
        Ok(sha)
    }

    fn commit_subjects(&self, base: &str, head: &str) -> anyhow::Result<Vec<String>> {
        let range = format!("{base}..{head}");
        let output = self.run_command(&["log", "--format=%s", &range])?;
        Ok(output.lines().map(|l| l.to_string()).collect())
    }

    fn push_to_target(&self, target: &str, _path: &Path) -> anyhow::Result<()> {
        self.run_command(&["push", "origin", &format!("HEAD:{target}")])?;
        Ok(())
    }

    fn advance_and_push(&self, target: &str, _path: &Path) -> anyhow::Result<PushResult> {
        // Check fast-forward
        if !self.is_ancestor(target, "HEAD")? {
            let commits_formatted = self
                .run_command(&[
                    "log",
                    "--color=always",
                    "--graph",
                    "--oneline",
                    &format!("HEAD..{target}"),
                ])?
                .trim()
                .to_string();
            return Err(GitError::NotFastForward {
                target_branch: target.to_string(),
                commits_formatted,
                in_merge_context: false,
            }
            .into());
        }

        let commit_count = self.count_commits(target, "HEAD")?;
        if commit_count == 0 {
            return Ok(PushResult {
                commit_count: 0,
                stats_summary: Vec::new(),
            });
        }

        // Collect display data before push (diffstat, stats summary)
        let range = format!("{target}..HEAD");
        let stats_summary = self.diff_stats_summary(&["diff", "--shortstat", &range]);

        // Auto-stash non-conflicting changes in the target worktree (if present)
        let target_wt_path = self.worktree_for_branch(target)?;
        let stash_info = stash_target_if_dirty(self, target_wt_path.as_ref(), target)?;

        // Show progress message, commit graph, and diffstat (between stash and restore)
        show_push_preview(self, target, commit_count, &range);

        // Local push to advance the target branch
        let git_common_dir = self.git_common_dir();
        let push_target = format!("HEAD:{target}");
        let push_result = self.run_command(&[
            "push",
            "--receive-pack=git -c receive.denyCurrentBranch=updateInstead receive-pack",
            &git_common_dir.to_string_lossy(),
            &push_target,
        ]);

        // Restore stash regardless of push result
        if let Some((wt_path, stash_ref)) = stash_info {
            restore_stash(self, &wt_path, &stash_ref);
        }

        push_result.map_err(|e| GitError::PushFailed {
            target_branch: target.to_string(),
            error: e.to_string(),
        })?;

        Ok(PushResult {
            commit_count,
            stats_summary,
        })
    }

    fn feature_head(&self, _path: &Path) -> anyhow::Result<String> {
        Ok("HEAD".to_string())
    }

    fn diff_for_prompt(
        &self,
        base: &str,
        head: &str,
        _path: &Path,
    ) -> anyhow::Result<(String, String)> {
        let diff = self.run_command(&[
            "-c",
            "diff.noprefix=false",
            "-c",
            "diff.mnemonicPrefix=false",
            "--no-pager",
            "diff",
            base,
            head,
        ])?;
        let stat = self.run_command(&["--no-pager", "diff", base, head, "--stat"])?;
        Ok((diff, stat))
    }

    fn recent_subjects(&self, start_ref: Option<&str>, count: usize) -> Option<Vec<String>> {
        self.recent_commit_subjects(start_ref, count)
    }

    fn squash_commits(
        &self,
        target: &str,
        message: &str,
        _path: &Path,
    ) -> anyhow::Result<SquashOutcome> {
        let merge_base = self
            .merge_base("HEAD", target)?
            .context("Cannot squash: no common ancestor with target branch")?;

        self.run_command(&["reset", "--soft", &merge_base])
            .context("Failed to reset to merge base")?;

        // Check if there are actually any changes to commit (commits may cancel out)
        if !self.current_worktree().has_staged_changes()? {
            return Ok(SquashOutcome::NoNetChanges);
        }

        self.run_command(&["commit", "-m", message])
            .context("Failed to create squash commit")?;

        let sha = self
            .run_command(&["rev-parse", "--short", "HEAD"])?
            .trim()
            .to_string();

        Ok(SquashOutcome::Squashed(sha))
    }

    fn has_staging_area(&self) -> bool {
        true
    }

    fn load_project_config(&self) -> anyhow::Result<Option<crate::config::ProjectConfig>> {
        Repository::load_project_config(self)
    }

    fn wt_logs_dir(&self) -> PathBuf {
        Repository::wt_logs_dir(self)
    }

    fn switch_previous(&self) -> Option<String> {
        Repository::switch_previous(self)
    }

    fn set_switch_previous(&self, name: Option<&str>) -> anyhow::Result<()> {
        Repository::set_switch_previous(self, name)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Print push progress: commit count, graph, and diffstat.
///
/// Emits the `◎ Pushing N commit(s) to TARGET @ SHA` progress message,
/// followed by the commit graph and diffstat with gutter formatting.
fn show_push_preview(repo: &Repository, target: &str, commit_count: usize, range: &str) {
    let commit_text = if commit_count == 1 {
        "commit"
    } else {
        "commits"
    };
    let head_sha = repo
        .run_command(&["rev-parse", "--short", "HEAD"])
        .unwrap_or_default();
    let head_sha = head_sha.trim();

    eprintln!(
        "{}",
        progress_message(cformat!(
            "Pushing {commit_count} {commit_text} to <bold>{target}</> @ <dim>{head_sha}</>"
        ))
    );

    // Commit graph
    if let Ok(log_output) =
        repo.run_command(&["log", "--color=always", "--graph", "--oneline", range])
    {
        eprintln!("{}", format_with_gutter(&log_output, None));
    }

    // Diffstat
    let term_width = get_terminal_width();
    let stat_width = term_width.saturating_sub(GUTTER_OVERHEAD);
    if let Ok(diff_stat) = repo.run_command(&[
        "diff",
        "--color=always",
        "--stat",
        &format!("--stat-width={stat_width}"),
        range,
    ]) {
        let diff_stat = diff_stat.trim_end();
        if !diff_stat.is_empty() {
            eprintln!("{}", format_with_gutter(diff_stat, None));
        }
    }
}

/// Auto-stash non-conflicting dirty changes in the target worktree.
///
/// Returns `Some((path, stash_ref))` if changes were stashed, `None` otherwise.
/// Errors if the target worktree has dirty files that overlap with the push range.
fn stash_target_if_dirty(
    repo: &Repository,
    target_wt_path: Option<&PathBuf>,
    target: &str,
) -> anyhow::Result<Option<(PathBuf, String)>> {
    let Some(wt_path) = target_wt_path else {
        return Ok(None);
    };
    if !wt_path.exists() {
        return Ok(None);
    }

    let wt = repo.worktree_at(wt_path);
    if !wt.is_dirty()? {
        return Ok(None);
    }

    // Check for overlapping files
    let push_files = repo.changed_files(target, "HEAD")?;
    let wt_status = wt.run_command(&["status", "--porcelain", "-z"])?;
    let wt_files = parse_porcelain_z(&wt_status);

    let overlapping: Vec<String> = push_files
        .iter()
        .filter(|f| wt_files.contains(f))
        .cloned()
        .collect();

    if !overlapping.is_empty() {
        return Err(GitError::ConflictingChanges {
            target_branch: target.to_string(),
            files: overlapping,
            worktree_path: wt_path.clone(),
        }
        .into());
    }

    // Stash non-conflicting changes
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let stash_name = format!(
        "worktrunk autostash::{}::{}::{}",
        target,
        std::process::id(),
        nanos
    );

    eprintln!(
        "{}",
        progress_message(cformat!(
            "Stashing changes in <bold>{}</>...",
            format_path_for_display(wt_path)
        ))
    );

    wt.run_command(&["stash", "push", "--include-untracked", "-m", &stash_name])?;

    // Find the stash ref
    let list_output = wt.run_command(&["stash", "list", "--format=%gd%x00%gs%x00"])?;
    let mut parts = list_output.split('\0');
    while let Some(id) = parts.next() {
        if id.is_empty() {
            continue;
        }
        if let Some(message) = parts.next()
            && (message == stash_name || message.ends_with(&stash_name))
        {
            return Ok(Some((wt_path.clone(), id.to_string())));
        }
    }

    // Stash entry not found — verify worktree is clean (stash may have been empty)
    if wt.is_dirty()? {
        anyhow::bail!(
            "Failed to stash changes in {}; worktree still has uncommitted changes",
            format_path_for_display(wt_path)
        );
    }

    // Worktree is clean and no stash entry — nothing needed to be stashed
    Ok(None)
}

/// Restore a previously created stash (best-effort).
fn restore_stash(repo: &Repository, wt_path: &Path, stash_ref: &str) {
    eprintln!(
        "{}",
        progress_message(cformat!(
            "Restoring stashed changes in <bold>{}</>...",
            format_path_for_display(wt_path)
        ))
    );

    let success = repo
        .worktree_at(wt_path)
        .run_command(&["stash", "pop", stash_ref])
        .is_ok();

    if !success {
        eprintln!(
            "{}",
            warning_message(cformat!(
                "Failed to restore stash <bold>{stash_ref}</>; run <bold>git stash pop {stash_ref}</> in <bold>{path}</>",
                path = format_path_for_display(wt_path),
            ))
        );
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::git::WorktreeInfo;

    use super::super::WorkspaceItem;

    #[test]
    fn test_from_worktree_with_branch() {
        let wt = WorktreeInfo {
            path: PathBuf::from("/repos/myrepo.feature"),
            head: "abc123".into(),
            branch: Some("feature".into()),
            bare: false,
            detached: false,
            locked: None,
            prunable: None,
        };

        let item = WorkspaceItem::from_worktree(wt, false);

        assert_eq!(item.name, "feature");
        assert_eq!(item.head, "abc123");
        assert_eq!(item.branch, Some("feature".into()));
        assert_eq!(item.path, PathBuf::from("/repos/myrepo.feature"));
        assert!(!item.is_default);
    }

    #[test]
    fn test_from_worktree_detached() {
        let wt = WorktreeInfo {
            path: PathBuf::from("/repos/myrepo.detached"),
            head: "def456".into(),
            branch: None,
            bare: false,
            detached: true,
            locked: None,
            prunable: None,
        };

        let item = WorkspaceItem::from_worktree(wt, true);

        // Falls back to directory name when no branch
        assert_eq!(item.name, "myrepo.detached");
        assert_eq!(item.head, "def456");
        assert_eq!(item.branch, None);
        assert!(item.is_default);
    }

    #[test]
    fn test_from_worktree_locked() {
        let wt = WorktreeInfo {
            path: PathBuf::from("/repos/myrepo.locked"),
            head: "789abc".into(),
            branch: Some("locked-branch".into()),
            bare: false,
            detached: false,
            locked: Some("in use".into()),
            prunable: None,
        };

        let item = WorkspaceItem::from_worktree(wt, false);

        assert_eq!(item.locked, Some("in use".into()));
        assert_eq!(item.prunable, None);
    }

    #[test]
    fn test_from_worktree_prunable() {
        let wt = WorktreeInfo {
            path: PathBuf::from("/repos/myrepo.gone"),
            head: "000000".into(),
            branch: Some("gone-branch".into()),
            bare: false,
            detached: false,
            locked: None,
            prunable: Some("directory missing".into()),
        };

        let item = WorkspaceItem::from_worktree(wt, false);

        assert_eq!(item.prunable, Some("directory missing".into()));
    }

    /// Exercise all `Workspace` trait methods on a real git repository.
    ///
    /// This covers the `Workspace for Repository` implementation which
    /// maps `Repository` methods into the VCS-agnostic trait.
    #[test]
    fn test_workspace_trait_on_real_repo() {
        use std::process::Command;

        use super::super::{VcsKind, Workspace};
        use crate::git::Repository;

        let temp = tempfile::tempdir().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();

        let git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(&repo_path)
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "test@test.com")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "test@test.com")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };

        git(&["init", "-b", "main"]);
        std::fs::write(repo_path.join("file.txt"), "hello\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "initial"]);

        let repo = Repository::at(&repo_path).unwrap();
        let ws: &dyn Workspace = &repo;

        // kind
        assert_eq!(ws.kind(), VcsKind::Git);

        // has_staging_area
        assert!(ws.has_staging_area());

        // list_workspaces
        let items = ws.list_workspaces().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "main");
        assert!(items[0].is_default);

        // default_workspace_path
        assert!(ws.default_workspace_path().unwrap().is_some());

        // default_branch_name
        let _ = ws.default_branch_name();

        // is_dirty — clean state
        assert!(!ws.is_dirty(&repo_path).unwrap());

        // working_diff — clean
        let diff = ws.working_diff(&repo_path).unwrap();
        assert_eq!(diff.added, 0);
        assert_eq!(diff.deleted, 0);

        // Make dirty
        std::fs::write(repo_path.join("file.txt"), "modified\n").unwrap();
        assert!(ws.is_dirty(&repo_path).unwrap());
        let diff = ws.working_diff(&repo_path).unwrap();
        assert!(diff.added > 0 || diff.deleted > 0);
        git(&["checkout", "--", "."]);

        // Create feature branch with a commit
        git(&["checkout", "-b", "feature"]);
        std::fs::write(repo_path.join("feature.txt"), "feature\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "feature commit"]);

        // ahead_behind
        let (ahead, behind) = ws.ahead_behind("main", "feature").unwrap();
        assert_eq!(ahead, 1);
        assert_eq!(behind, 0);

        // is_integrated — feature is NOT an ancestor of main
        let integrated = ws.is_integrated("feature", "main").unwrap();
        assert!(integrated.is_none());

        // branch_diff_stats
        let diff = ws.branch_diff_stats("main", "feature").unwrap();
        assert!(diff.added > 0);

        // Switch back to main for workspace mutation tests
        git(&["checkout", "main"]);

        // create_workspace (also covers Repository::create_worktree)
        let wt_path = temp.path().join("test-wt");
        ws.create_workspace("test-branch", None, &wt_path).unwrap();

        // workspace_path — found by branch name
        let path = ws.workspace_path("test-branch").unwrap();
        assert_eq!(
            dunce::canonicalize(&path).unwrap(),
            dunce::canonicalize(&wt_path).unwrap()
        );

        // remove_workspace
        ws.remove_workspace("test-branch").unwrap();

        // workspace_path — not found
        assert!(ws.workspace_path("nonexistent").is_err());

        // create_workspace with base revision (covers the `if let Some(base_ref)` branch)
        let wt_path2 = temp.path().join("test-wt2");
        ws.create_workspace("from-feature", Some("feature"), &wt_path2)
            .unwrap();
        ws.remove_workspace("from-feature").unwrap();

        // as_any downcast
        let repo_ref = ws.as_any().downcast_ref::<Repository>();
        assert!(repo_ref.is_some());
    }
}
