//! VCS-agnostic workspace abstraction.
//!
//! This module provides the [`Workspace`] trait that captures the operations
//! commands need, independent of the underlying VCS (git, jj, etc.).
//!
//! The git implementation is on [`Repository`](crate::git::Repository) directly.
//! The jj implementation ([`JjWorkspace`]) shells out to `jj` CLI commands.
//! Commands that need git-specific features can downcast via
//! `workspace.as_any().downcast_ref::<Repository>()`.
//!
//! Use [`detect_vcs`] to determine which VCS manages a given path.

pub(crate) mod detect;
mod git;
pub(crate) mod jj;
pub mod types;

use std::any::Any;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::config::StageMode;
use crate::git::WorktreeInfo;
pub use types::{IntegrationReason, LineDiff, LocalPushDisplay, LocalPushResult, path_dir_name};

pub use detect::detect_vcs;
pub use jj::JjWorkspace;

/// Outcome of a rebase operation on the VCS level.
#[derive(Debug)]
pub enum RebaseOutcome {
    /// True rebase (history rewritten).
    Rebased,
    /// Fast-forward (HEAD moved forward, no rewrite).
    FastForward,
}

/// Outcome of a squash operation on the VCS level.
pub enum SquashOutcome {
    /// Commits were squashed into one. Contains the new commit identifier (short SHA or change ID).
    Squashed(String),
    /// Squash completed but resulted in no net changes (commits canceled out).
    /// Git-only: detected after `reset --soft` when staging area is empty.
    NoNetChanges,
}

/// Version control system type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VcsKind {
    Git,
    Jj,
}

/// VCS-agnostic workspace item (worktree in git, workspace in jj).
#[derive(Debug, Clone)]
pub struct WorkspaceItem {
    /// Filesystem path to the workspace root.
    pub path: PathBuf,
    /// Workspace name. In git: derived from branch name (or directory name for
    /// detached HEAD). In jj: the native workspace name.
    pub name: String,
    /// Commit identifier. In git: commit SHA. In jj: change ID.
    pub head: String,
    /// Branch name (git) or bookmark name (jj). None for detached HEAD (git)
    /// or workspaces without bookmarks (jj).
    pub branch: Option<String>,
    /// Whether this is the default/primary workspace.
    pub is_default: bool,
    /// Lock reason, if locked.
    pub locked: Option<String>,
    /// Prunable reason, if prunable (directory deleted but VCS still tracks it).
    pub prunable: Option<String>,
}

impl WorkspaceItem {
    /// Create a `WorkspaceItem` from a git [`WorktreeInfo`].
    ///
    /// The `name` field uses the branch name when available, falling back
    /// to the directory name for detached HEAD worktrees.
    pub fn from_worktree(wt: WorktreeInfo, is_default: bool) -> Self {
        let name = wt
            .branch
            .clone()
            .unwrap_or_else(|| path_dir_name(&wt.path).to_string());

        Self {
            path: wt.path,
            name,
            head: wt.head,
            branch: wt.branch,
            is_default,
            locked: wt.locked,
            prunable: wt.prunable,
        }
    }
}

/// VCS-agnostic workspace operations.
///
/// Captures what commands need at the workspace-operation level, not the
/// VCS-command level. Each VCS implementation translates these operations
/// into the appropriate commands.
pub trait Workspace: Send + Sync {
    /// Which VCS backs this workspace.
    fn kind(&self) -> VcsKind;

    // ====== Discovery ======

    /// List all workspaces in the repository.
    fn list_workspaces(&self) -> anyhow::Result<Vec<WorkspaceItem>>;

    /// Resolve a workspace name to its filesystem path.
    fn workspace_path(&self, name: &str) -> anyhow::Result<PathBuf>;

    /// Path to the default/primary workspace.
    fn default_workspace_path(&self) -> anyhow::Result<Option<PathBuf>>;

    /// Name of the default/trunk branch. Returns `None` if unknown.
    /// Git: detected from remote or local heuristics. Jj: from config or `trunk()` revset.
    fn default_branch_name(&self) -> Option<String>;

    /// Override the default branch name (persisted to VCS-specific config).
    fn set_default_branch(&self, name: &str) -> anyhow::Result<()>;

    /// Clear the configured default branch override. Returns `true` if a value was cleared.
    fn clear_default_branch(&self) -> anyhow::Result<bool>;

    // ====== Status per workspace ======

    /// Whether the workspace has uncommitted changes.
    fn is_dirty(&self, path: &Path) -> anyhow::Result<bool>;

    /// Line-level diff of uncommitted changes.
    fn working_diff(&self, path: &Path) -> anyhow::Result<LineDiff>;

    // ====== Comparison against trunk ======

    /// Commits ahead/behind between two refs.
    fn ahead_behind(&self, base: &str, head: &str) -> anyhow::Result<(usize, usize)>;

    /// Check if content identified by `id` is integrated into `target`.
    /// Returns the integration reason if integrated, `None` if not.
    fn is_integrated(&self, id: &str, target: &str) -> anyhow::Result<Option<IntegrationReason>>;

    /// Line-level diff stats between two refs (committed changes only).
    fn branch_diff_stats(&self, base: &str, head: &str) -> anyhow::Result<LineDiff>;

    // ====== Mutations ======

    /// Create a new workspace.
    /// - `name`: workspace/branch name
    /// - `base`: starting point (branch, commit, or None for default)
    /// - `path`: filesystem path for the new workspace
    fn create_workspace(&self, name: &str, base: Option<&str>, path: &Path) -> anyhow::Result<()>;

    /// Remove a workspace by name.
    fn remove_workspace(&self, name: &str) -> anyhow::Result<()>;

    // ====== Rebase ======

    /// Resolve the integration target (branch/bookmark to rebase onto).
    /// Git: validates ref exists, falls back to default branch.
    /// Jj: uses `default_branch_name()` detection chain (config, trunk, local bookmarks).
    fn resolve_integration_target(&self, target: Option<&str>) -> anyhow::Result<String>;

    /// Whether the current workspace is already rebased onto `target`.
    /// Git: merge-base == target SHA, no merge commits between.
    /// Jj: target is ancestor of feature tip.
    fn is_rebased_onto(&self, target: &str, path: &Path) -> anyhow::Result<bool>;

    /// Rebase the current workspace onto `target`.
    /// Returns the outcome (Rebased vs FastForward).
    /// Implementations emit their own progress message when appropriate.
    fn rebase_onto(&self, target: &str, path: &Path) -> anyhow::Result<RebaseOutcome>;

    // ====== Identity ======

    /// Root path of the repository (git dir or jj repo root).
    fn root_path(&self) -> anyhow::Result<PathBuf>;

    /// Filesystem path of the current workspace/worktree.
    ///
    /// Git: uses `current_worktree().path()` (respects `-C` flag via cwd).
    /// Jj: uses `current_workspace().path` (found via cwd).
    fn current_workspace_path(&self) -> anyhow::Result<PathBuf>;

    /// Current workspace/branch name at the given path.
    /// Returns `None` for detached HEAD (git) or workspaces without bookmarks (jj).
    fn current_name(&self, path: &Path) -> anyhow::Result<Option<String>>;

    /// Project identifier for approval/hook scoping.
    /// Uses remote URL if available, otherwise the canonical repository path.
    fn project_identifier(&self) -> anyhow::Result<String>;

    // ====== Commit ======

    /// Prepare the working tree for commit.
    ///
    /// Git: warns about untracked files if `mode` is [`StageMode::All`],
    ///   then stages files (`git add -A`, `git add -u`, or nothing).
    /// Jj: no-op (jj auto-snapshots the working copy).
    fn prepare_commit(&self, path: &Path, mode: StageMode) -> anyhow::Result<()>;

    /// Commit staged/working changes with the given message.
    /// Returns the new commit identifier (SHA for git, change ID for jj).
    fn commit(&self, message: &str, path: &Path) -> anyhow::Result<String>;

    /// Subject lines of commits between `base` and `head`.
    fn commit_subjects(&self, base: &str, head: &str) -> anyhow::Result<Vec<String>>;

    // ====== Push ======

    /// Push current branch/bookmark to remote, fast-forward only.
    /// `target` is the branch/bookmark to update on the remote.
    fn push_to_target(&self, target: &str, path: &Path) -> anyhow::Result<()>;

    /// Advance the target branch ref to include current feature commits (local only).
    ///
    /// "Local push" means moving the target branch pointer forward — no remote
    /// interaction. This is the git term for advancing a ref locally via
    /// `git push <local-path>`.
    ///
    /// Git: fast-forward the target branch to HEAD, with auto-stash/restore of
    ///   non-conflicting changes in the target worktree. Emits progress messages
    ///   (commit graph, diffstat) to stderr.
    /// Jj: `jj bookmark set` to move the target bookmark to the feature tip.
    ///
    /// Returns a [`LocalPushResult`] with commit count and optional stats for
    /// the command handler to format the final success message.
    fn local_push(
        &self,
        target: &str,
        path: &Path,
        display: LocalPushDisplay<'_>,
    ) -> anyhow::Result<LocalPushResult>;

    // ====== Squash ======

    /// The current feature head reference for squash/diff operations.
    ///
    /// Git: `"HEAD"` (literal string — git resolves it).
    /// Jj: the feature tip change ID (@ if non-empty, @- otherwise).
    fn feature_head(&self, path: &Path) -> anyhow::Result<String>;

    /// Produce a diff and diffstat between two refs, suitable for LLM prompt consumption.
    ///
    /// Returns `(raw_diff, diffstat)`.
    /// Git: `git diff base..head` with consistent prefix settings.
    /// Jj: `jj diff --from base --to head`.
    fn diff_for_prompt(
        &self,
        base: &str,
        head: &str,
        path: &Path,
    ) -> anyhow::Result<(String, String)>;

    /// Recent commit subjects for LLM style reference.
    ///
    /// Returns up to `count` recent commit subjects, starting from `start_ref` if given.
    /// Returns `None` if no commits are available.
    fn recent_subjects(&self, start_ref: Option<&str>, count: usize) -> Option<Vec<String>>;

    /// Squash all commits between target and HEAD/feature-tip into a single commit.
    ///
    /// Git: `git reset --soft <merge-base> && git commit -m <message>`
    /// Jj: `jj new {target} && jj squash --from '{target}..{tip}' --into @ -m <message>`
    ///
    /// The message is generated by the command handler (LLM, template, or fallback).
    /// Returns `SquashOutcome::Squashed(id)` on success, or `NoNetChanges` if the
    /// commits cancel out (git-only: empty staging area after soft reset).
    fn squash_commits(
        &self,
        target: &str,
        message: &str,
        path: &Path,
    ) -> anyhow::Result<SquashOutcome>;

    // ====== Commit prompt ======

    /// Diff and diffstat of changes that would be committed, for LLM prompt consumption.
    ///
    /// Returns `(raw_diff, diffstat)` representing "what's about to be committed."
    /// Git: staged changes (`git diff --staged`).
    /// Jj: working-copy changes (`jj diff -r @`).
    fn committable_diff_for_prompt(&self, path: &Path) -> anyhow::Result<(String, String)>;

    // ====== Copy-ignored ======

    /// List ignored (by `.gitignore`) entries in the given workspace directory.
    ///
    /// Returns `(absolute_path, is_directory)` pairs. Uses directory-level
    /// granularity — stops at directory boundaries so `target/` is one entry,
    /// not thousands of files.
    ///
    /// Git: `git ls-files --ignored --exclude-standard -o --directory`
    /// Jj: same command with explicit `--git-dir` pointing to the git backend.
    fn list_ignored_entries(&self, path: &Path) -> anyhow::Result<Vec<(PathBuf, bool)>>;

    // ====== Capabilities ======

    /// Whether this VCS has a staging area (index).
    /// Git: true. Jj: false.
    fn has_staging_area(&self) -> bool;

    // ====== Hooks & Configuration ======

    /// Load project configuration (`.config/wt.toml`) from the repository root.
    fn load_project_config(&self) -> anyhow::Result<Option<crate::config::ProjectConfig>>;

    /// Directory for background hook log files.
    /// Git: `.git/wt-logs/`. Jj: `.jj/wt-logs/`.
    fn wt_logs_dir(&self) -> PathBuf;

    /// Previously-switched-from workspace name (for `wt switch -`).
    fn switch_previous(&self) -> Option<String>;

    /// Record the current workspace name before switching away.
    fn set_switch_previous(&self, name: Option<&str>) -> anyhow::Result<()>;

    /// Clear the previously-switched-from workspace name. Returns `true` if cleared.
    fn clear_switch_previous(&self) -> anyhow::Result<bool>;

    // ====== Markers (per-branch state) ======

    /// Get the marker for a branch/workspace name.
    ///
    /// Markers are stored as JSON: `{"marker": "text", "set_at": unix_timestamp}`.
    fn branch_marker(&self, name: &str) -> Option<String>;

    /// Set a marker for a branch/workspace name (stored as JSON with timestamp).
    fn set_branch_marker(&self, name: &str, marker: &str, timestamp: u64) -> anyhow::Result<()>;

    /// Clear the marker for a branch/workspace name. Returns true if it existed.
    fn clear_branch_marker(&self, name: &str) -> bool;

    /// List all branch markers. Returns `(branch, marker_text, set_at_timestamp)` tuples.
    fn list_all_markers(&self) -> Vec<(String, String, u64)>;

    /// Clear all markers. Returns the number cleared.
    fn clear_all_markers(&self) -> usize;

    // ====== Hints (shown-once messages) ======

    /// Check if a hint has been shown in this repo.
    fn has_shown_hint(&self, name: &str) -> bool;

    /// Mark a hint as shown in this repo.
    fn mark_hint_shown(&self, name: &str) -> anyhow::Result<()>;

    /// Clear a hint so it will show again.
    fn clear_hint(&self, name: &str) -> anyhow::Result<bool>;

    /// List all hints that have been shown in this repo.
    fn list_shown_hints(&self) -> Vec<String>;

    /// Clear all hints so they will show again. Returns the number cleared.
    fn clear_all_hints(&self) -> anyhow::Result<usize>;

    // ====== Downcast ======

    /// Downcast to concrete type for VCS-specific operations.
    fn as_any(&self) -> &dyn Any;
}

/// Build a branch→path lookup map from workspace items.
///
/// Used by `expand_template` for the `worktree_path_of_branch()` template function.
/// Maps both workspace names and branch names to their filesystem paths, so lookups
/// work with either identifier.
pub fn build_worktree_map(workspace: &dyn Workspace) -> HashMap<String, PathBuf> {
    workspace
        .list_workspaces()
        .unwrap_or_default()
        .into_iter()
        .flat_map(|ws| {
            let mut entries = vec![(ws.name.clone(), ws.path.clone())];
            if let Some(branch) = &ws.branch
                && branch != &ws.name
            {
                entries.push((branch.clone(), ws.path.clone()));
            }
            entries
        })
        .collect()
}

/// Detect VCS and open the appropriate workspace for the current directory.
///
/// The `-C` flag is handled by `std::env::set_current_dir()` in main.rs before
/// any commands run, so `current_dir()` already reflects the right path.
///
/// Falls back to `Repository::current()` when filesystem markers aren't found,
/// which handles bare repos and other non-standard layouts that git itself can discover.
pub fn open_workspace() -> anyhow::Result<Box<dyn Workspace>> {
    let detect_path = std::env::current_dir()?;
    match detect_vcs(&detect_path) {
        Some(VcsKind::Jj) => Ok(Box::new(JjWorkspace::from_current_dir()?)),
        Some(VcsKind::Git) => {
            let repo = crate::git::Repository::current()?;
            Ok(Box::new(repo))
        }
        None => {
            // Fallback: try git discovery (handles bare repos, -C flag, etc.)
            match crate::git::Repository::current() {
                Ok(repo) => Ok(Box::new(repo)),
                Err(_) => anyhow::bail!("Not in a repository"),
            }
        }
    }
}
