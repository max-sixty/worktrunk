//! Progressive worktree collection with parallel git operations.
//!
//! This module provides a typed task framework for cell-by-cell progressive rendering.
//! Git operations run in parallel and send results as they complete.
//!
//! ## Architecture
//!
//! The framework guarantees that every spawned task is registered in `ExpectedResults`
//! and sends exactly one `TaskResult`:
//!
//! - `Task` trait: Each task type implements `compute()` returning a `TaskResult`
//! - `TaskSpawner`: Ties together registration + spawn + send in a single operation
//!
//! This eliminates the "spawn but forget to register" failure mode from the old design.

use crossbeam_channel::Sender;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use worktrunk::git::{LineDiff, Repository, Worktree};

use super::ci_status::PrStatus;
use super::collect::{ExpectedResults, TaskError, TaskKind, TaskResult, detect_git_operation};
use super::model::{
    AheadBehind, BranchDiffTotals, CommitDetails, UpstreamStatus, WorkingTreeStatus,
};

// ============================================================================
// Options and Context
// ============================================================================

/// Options for controlling what data to collect.
///
/// Uses a skip set to control which tasks are spawned. Tasks not in the set
/// will be computed; tasks in the set will be skipped.
#[derive(Clone, Default)]
pub struct CollectOptions {
    /// Tasks to skip (not compute). Empty set means compute everything.
    ///
    /// This controls both:
    /// - Task spawning (in `collect_worktree_progressive`/`collect_branch_progressive`)
    /// - Column visibility (layout filters columns via `ColumnSpec::requires_task`)
    pub skip_tasks: std::collections::HashSet<super::collect::TaskKind>,

    /// URL template from project config (e.g., "http://localhost:{{ branch | hash_port }}").
    /// Expanded per-item in task spawning (post-skeleton) to minimize time-to-skeleton.
    pub url_template: Option<String>,
}

/// Context for task computation. Cloned and moved into spawned threads.
///
/// Contains all data needed by any task.
#[derive(Clone)]
pub struct TaskContext {
    pub repo_path: PathBuf,
    pub commit_sha: String,
    pub branch: Option<String>,
    /// Local default branch for informational stats (ahead/behind, branch diff).
    /// Always the local ref (e.g., "main"), providing stable comparisons.
    pub default_branch: Option<String>,
    /// Effective target for integration checks (status symbols, safe deletion).
    /// May be upstream (e.g., "origin/main") if it's ahead of local, catching remotely-merged branches.
    pub target: Option<String>,
    pub item_idx: usize,
    /// Expanded URL for this item (from project config template).
    /// UrlStatusTask uses this to check if the port is listening.
    pub item_url: Option<String>,
}

impl TaskContext {
    fn repo(&self) -> Repository {
        Repository::at(&self.repo_path)
    }

    fn error(&self, kind: TaskKind, message: impl Display) -> TaskError {
        TaskError::new(self.item_idx, kind, message.to_string())
    }

    fn require_default_branch(&self, kind: TaskKind) -> Result<&str, TaskError> {
        self.default_branch
            .as_deref()
            .ok_or_else(|| self.error(kind, "no default branch"))
    }

    fn require_target(&self, kind: TaskKind) -> Result<&str, TaskError> {
        self.target
            .as_deref()
            .ok_or_else(|| self.error(kind, "no target branch"))
    }
}

// ============================================================================
// Task Trait and Spawner
// ============================================================================

/// A task that computes a single `TaskResult`.
///
/// Each task type has a compile-time `KIND` that determines which `TaskResult`
/// variant it produces. The `compute()` function receives a cloned context and
/// returns a Result - either the successful result or an error.
///
/// Tasks should propagate errors via `?` rather than swallowing them.
/// The drain layer handles defaults and collects errors for display.
pub trait Task: Send + Sync + 'static {
    /// The kind of result this task produces (compile-time constant).
    const KIND: TaskKind;

    /// Compute the task result. Called in a spawned thread.
    /// Returns Ok(result) on success, Err(TaskError) on failure.
    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError>;
}

/// Spawner that ties together registration + spawn + send.
///
/// Using `TaskSpawner::spawn<T>()` is the only way to run a task, and it
/// automatically registers the expected result kind before spawning.
pub struct TaskSpawner {
    tx: Sender<Result<TaskResult, TaskError>>,
    expected: Arc<ExpectedResults>,
}

impl TaskSpawner {
    pub fn new(tx: Sender<Result<TaskResult, TaskError>>, expected: Arc<ExpectedResults>) -> Self {
        Self { tx, expected }
    }

    /// Spawn a task, registering its expected result and sending on completion.
    ///
    /// This is the only way to run a `Task`. It guarantees:
    /// 1. The expected result is registered before the task runs
    /// 2. Exactly one result (Ok or Err) is sent when the task completes
    pub fn spawn<'scope, T: Task>(
        &self,
        scope: &'scope std::thread::Scope<'scope, '_>,
        ctx: &TaskContext,
    ) {
        // 1. Register expectation
        self.expected.expect(ctx.item_idx, T::KIND);

        // 2. Clone for the spawned thread
        let tx = self.tx.clone();
        let ctx = ctx.clone();

        // 3. Spawn the work
        scope.spawn(move || {
            let result = T::compute(ctx);
            if let Ok(ref task_result) = result {
                debug_assert_eq!(TaskKind::from(task_result), T::KIND);
            }
            let _ = tx.send(result);
        });
    }

    fn spawn_core_tasks<'scope>(
        &self,
        scope: &'scope std::thread::Scope<'scope, '_>,
        ctx: &TaskContext,
    ) {
        self.spawn::<CommitDetailsTask>(scope, ctx);
        self.spawn::<AheadBehindTask>(scope, ctx);
        self.spawn::<CommittedTreesMatchTask>(scope, ctx);
        self.spawn::<HasFileChangesTask>(scope, ctx);
        self.spawn::<IsAncestorTask>(scope, ctx);
        self.spawn::<UpstreamTask>(scope, ctx);
    }

    fn spawn_worktree_only_tasks<'scope>(
        &self,
        scope: &'scope std::thread::Scope<'scope, '_>,
        ctx: &TaskContext,
        skip: &std::collections::HashSet<TaskKind>,
    ) {
        self.spawn::<WorkingTreeDiffTask>(scope, ctx);
        self.spawn::<GitOperationTask>(scope, ctx);
        self.spawn::<UserMarkerTask>(scope, ctx);
        // Working tree conflict check only with --full
        if !skip.contains(&TaskKind::WorkingTreeConflicts) {
            self.spawn::<WorkingTreeConflictsTask>(scope, ctx);
        }
    }

    fn spawn_optional_tasks<'scope>(
        &self,
        scope: &'scope std::thread::Scope<'scope, '_>,
        ctx: &TaskContext,
        skip: &std::collections::HashSet<TaskKind>,
    ) {
        if !skip.contains(&TaskKind::BranchDiff) {
            self.spawn::<BranchDiffTask>(scope, ctx);
        }
        if !skip.contains(&TaskKind::MergeTreeConflicts) {
            self.spawn::<MergeTreeConflictsTask>(scope, ctx);
        }
        if !skip.contains(&TaskKind::CiStatus) {
            self.spawn::<CiStatusTask>(scope, ctx);
        }
        if !skip.contains(&TaskKind::WouldMergeAdd) {
            self.spawn::<WouldMergeAddTask>(scope, ctx);
        }
        // URL status only runs if this item has a URL
        if !skip.contains(&TaskKind::UrlStatus) && ctx.item_url.is_some() {
            self.spawn::<UrlStatusTask>(scope, ctx);
        }
    }
}

// ============================================================================
// Task Implementations
// ============================================================================

/// Task 1: Commit details (timestamp, message)
pub struct CommitDetailsTask;

impl Task for CommitDetailsTask {
    const KIND: TaskKind = TaskKind::CommitDetails;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        let repo = ctx.repo();
        let timestamp = repo
            .commit_timestamp(&ctx.commit_sha)
            .map_err(|e| ctx.error(Self::KIND, e))?;
        let commit_message = repo
            .commit_message(&ctx.commit_sha)
            .map_err(|e| ctx.error(Self::KIND, e))?;
        Ok(TaskResult::CommitDetails {
            item_idx: ctx.item_idx,
            commit: CommitDetails {
                timestamp,
                commit_message,
            },
        })
    }
}

/// Task 2: Ahead/behind counts vs local default branch (informational stats)
pub struct AheadBehindTask;

impl Task for AheadBehindTask {
    const KIND: TaskKind = TaskKind::AheadBehind;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        let base = ctx.require_default_branch(Self::KIND)?;
        let repo = ctx.repo();
        let (ahead, behind) = repo
            .ahead_behind(base, &ctx.commit_sha)
            .map_err(|e| ctx.error(Self::KIND, e))?;
        Ok(TaskResult::AheadBehind {
            item_idx: ctx.item_idx,
            counts: AheadBehind { ahead, behind },
        })
    }
}

/// Task 3: Tree identity check (does the item's commit tree match integration target's tree?)
///
/// Uses target for integration detection (squash merge, rebase).
pub struct CommittedTreesMatchTask;

impl Task for CommittedTreesMatchTask {
    const KIND: TaskKind = TaskKind::CommittedTreesMatch;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        let base = ctx.require_target(Self::KIND)?;
        let repo = ctx.repo();
        // Use ctx.commit_sha (the item's commit) instead of HEAD,
        // since for branches without worktrees, HEAD is the main worktree's HEAD
        let committed_trees_match = repo
            .trees_match(&ctx.commit_sha, base)
            .map_err(|e| ctx.error(Self::KIND, e))?;
        Ok(TaskResult::CommittedTreesMatch {
            item_idx: ctx.item_idx,
            committed_trees_match,
        })
    }
}

/// Task 3b: File changes check (does branch have file changes beyond merge-base?)
///
/// Uses three-dot diff (`target...branch`) to detect if the branch has any file
/// changes relative to the merge-base with target. Returns false when the diff
/// is empty, indicating the branch content is already integrated.
///
/// This catches branches where commits exist (ahead > 0) but those commits
/// don't add any file changes - e.g., squash-merged branches, merge commits
/// that pulled in main, or commits whose changes were reverted.
///
/// Uses target for integration detection.
pub struct HasFileChangesTask;

impl Task for HasFileChangesTask {
    const KIND: TaskKind = TaskKind::HasFileChanges;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        // No branch name (detached HEAD) - return conservative default (assume has changes)
        let Some(branch) = ctx.branch.as_deref() else {
            return Ok(TaskResult::HasFileChanges {
                item_idx: ctx.item_idx,
                has_file_changes: true,
            });
        };
        let base = ctx.require_target(Self::KIND)?;
        let repo = ctx.repo();
        let has_file_changes = repo
            .has_added_changes(branch, base)
            .map_err(|e| ctx.error(Self::KIND, e))?;
        Ok(TaskResult::HasFileChanges {
            item_idx: ctx.item_idx,
            has_file_changes,
        })
    }
}

/// Task 3b: Merge simulation
///
/// Checks if merging the branch into target would add any changes by simulating
/// the merge with `git merge-tree --write-tree`. Returns false when the merge
/// result equals target's tree, indicating the branch is already integrated.
///
/// This catches branches where target has advanced past the squash-merge point -
/// the three-dot diff might show changes, but those changes are already in target
/// via the squash merge.
///
/// Uses target for integration detection.
pub struct WouldMergeAddTask;

impl Task for WouldMergeAddTask {
    const KIND: TaskKind = TaskKind::WouldMergeAdd;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        // No branch name (detached HEAD) - return conservative default (assume would add)
        let Some(branch) = ctx.branch.as_deref() else {
            return Ok(TaskResult::WouldMergeAdd {
                item_idx: ctx.item_idx,
                would_merge_add: true,
            });
        };
        let base = ctx.require_target(Self::KIND)?;
        let repo = ctx.repo();
        let would_merge_add = repo
            .would_merge_add_to_target(branch, base)
            .map_err(|e| ctx.error(Self::KIND, e))?;
        Ok(TaskResult::WouldMergeAdd {
            item_idx: ctx.item_idx,
            would_merge_add,
        })
    }
}

/// Task 3c: Ancestor check (is branch HEAD an ancestor of integration target?)
///
/// Checks if branch is an ancestor of target - runs `git merge-base --is-ancestor`.
/// Returns true when the branch HEAD is in target's history (merged via fast-forward
/// or rebase).
///
/// Uses target (target) for the Ancestor integration reason in `⊂`.
/// The `_` symbol uses ahead/behind counts (vs default_branch) instead.
pub struct IsAncestorTask;

impl Task for IsAncestorTask {
    const KIND: TaskKind = TaskKind::IsAncestor;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        let base = ctx.require_target(Self::KIND)?;
        let repo = ctx.repo();
        let is_ancestor = repo
            .is_ancestor(&ctx.commit_sha, base)
            .map_err(|e| ctx.error(Self::KIND, e))?;
        Ok(TaskResult::IsAncestor {
            item_idx: ctx.item_idx,
            is_ancestor,
        })
    }
}

/// Task 4: Branch diff stats vs local default branch (informational stats)
pub struct BranchDiffTask;

impl Task for BranchDiffTask {
    const KIND: TaskKind = TaskKind::BranchDiff;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        let base = ctx.require_default_branch(Self::KIND)?;
        let repo = ctx.repo();
        let diff = repo
            .branch_diff_stats(base, &ctx.commit_sha)
            .map_err(|e| ctx.error(Self::KIND, e))?;
        Ok(TaskResult::BranchDiff {
            item_idx: ctx.item_idx,
            branch_diff: BranchDiffTotals { diff },
        })
    }
}

/// Task 5 (worktree only): Working tree diff + status flags
pub struct WorkingTreeDiffTask;

impl Task for WorkingTreeDiffTask {
    const KIND: TaskKind = TaskKind::WorkingTreeDiff;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        let repo = ctx.repo();
        let status_output = repo
            .run_command(&["status", "--porcelain"])
            .map_err(|e| ctx.error(Self::KIND, e))?;

        let (working_tree_status, is_dirty, has_conflicts) =
            parse_working_tree_status(&status_output);

        let working_tree_diff = if is_dirty {
            repo.working_tree_diff_stats()
                .map_err(|e| ctx.error(Self::KIND, e))?
        } else {
            LineDiff::default()
        };

        // Use default_branch (local default branch) for informational display
        let working_tree_diff_with_main = repo
            .working_tree_diff_with_base(ctx.default_branch.as_deref(), is_dirty)
            .map_err(|e| ctx.error(Self::KIND, e))?;

        Ok(TaskResult::WorkingTreeDiff {
            item_idx: ctx.item_idx,
            working_tree_diff,
            working_tree_diff_with_main,
            working_tree_status,
            has_conflicts,
        })
    }
}

/// Task 6: Potential merge conflicts check (merge-tree vs local main)
///
/// Uses default_branch (local main) for consistency with other Main subcolumn symbols.
/// Shows whether merging to your local main would conflict.
pub struct MergeTreeConflictsTask;

impl Task for MergeTreeConflictsTask {
    const KIND: TaskKind = TaskKind::MergeTreeConflicts;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        let base = ctx.require_default_branch(Self::KIND)?;
        let repo = ctx.repo();
        let has_merge_tree_conflicts = repo
            .has_merge_conflicts(base, &ctx.commit_sha)
            .map_err(|e| ctx.error(Self::KIND, e))?;
        Ok(TaskResult::MergeTreeConflicts {
            item_idx: ctx.item_idx,
            has_merge_tree_conflicts,
        })
    }
}

/// Task 6b (worktree only, --full only): Working tree conflict check
///
/// For dirty worktrees, uses `git stash create` to get a tree object that
/// includes uncommitted changes, then runs merge-tree against that.
/// Returns None if working tree is clean (caller should fall back to MergeTreeConflicts).
pub struct WorkingTreeConflictsTask;

impl Task for WorkingTreeConflictsTask {
    const KIND: TaskKind = TaskKind::WorkingTreeConflicts;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        let base = ctx.require_default_branch(Self::KIND)?;
        let repo = ctx.repo();

        // Quick check if working tree is dirty via git status
        let status_output = repo
            .run_command(&["status", "--porcelain"])
            .map_err(|e| ctx.error(Self::KIND, e))?;

        let is_dirty = !status_output.trim().is_empty();

        if !is_dirty {
            // Clean working tree - return None to signal "use commit-based check"
            return Ok(TaskResult::WorkingTreeConflicts {
                item_idx: ctx.item_idx,
                has_working_tree_conflicts: None,
            });
        }

        // Dirty working tree - create a temporary tree object via stash create
        // `git stash create` returns a commit SHA without modifying refs
        //
        // Note: stash create fails when there are unmerged files (merge conflict in progress).
        // In that case, fall back to the commit-based check.
        let stash_result = repo.run_command(&["stash", "create"]);

        let stash_sha = match stash_result {
            Ok(sha) => sha,
            Err(_) => {
                // Stash create failed (likely unmerged files during rebase/merge)
                // Fall back to commit-based check
                return Ok(TaskResult::WorkingTreeConflicts {
                    item_idx: ctx.item_idx,
                    has_working_tree_conflicts: None,
                });
            }
        };

        let stash_sha = stash_sha.trim();

        // If stash create returns empty, working tree is clean (shouldn't happen but handle it)
        if stash_sha.is_empty() {
            return Ok(TaskResult::WorkingTreeConflicts {
                item_idx: ctx.item_idx,
                has_working_tree_conflicts: None,
            });
        }

        // Run merge-tree with the stash commit
        let has_conflicts = repo
            .has_merge_conflicts(base, stash_sha)
            .map_err(|e| ctx.error(Self::KIND, e))?;

        Ok(TaskResult::WorkingTreeConflicts {
            item_idx: ctx.item_idx,
            has_working_tree_conflicts: Some(has_conflicts),
        })
    }
}

/// Task 7 (worktree only): Git operation state detection (rebase/merge)
pub struct GitOperationTask;

impl Task for GitOperationTask {
    const KIND: TaskKind = TaskKind::GitOperation;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        let repo = ctx.repo();
        let git_operation = detect_git_operation(&repo);
        Ok(TaskResult::GitOperation {
            item_idx: ctx.item_idx,
            git_operation,
        })
    }
}

/// Task 8 (worktree only): User-defined status from git config
pub struct UserMarkerTask;

impl Task for UserMarkerTask {
    const KIND: TaskKind = TaskKind::UserMarker;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        let repo = ctx.repo();
        let user_marker = repo.user_marker(ctx.branch.as_deref());
        Ok(TaskResult::UserMarker {
            item_idx: ctx.item_idx,
            user_marker,
        })
    }
}

/// Task 9: Upstream tracking status
pub struct UpstreamTask;

impl Task for UpstreamTask {
    const KIND: TaskKind = TaskKind::Upstream;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        let repo = ctx.repo();

        // No branch means no upstream
        let Some(branch) = ctx.branch.as_deref() else {
            return Ok(TaskResult::Upstream {
                item_idx: ctx.item_idx,
                upstream: UpstreamStatus::default(),
            });
        };

        // Get upstream branch (None is valid - just means no upstream configured)
        let upstream_branch = repo
            .upstream_branch(branch)
            .map_err(|e| ctx.error(Self::KIND, e))?;
        let Some(upstream_branch) = upstream_branch else {
            return Ok(TaskResult::Upstream {
                item_idx: ctx.item_idx,
                upstream: UpstreamStatus::default(),
            });
        };

        let remote = upstream_branch.split_once('/').map(|(r, _)| r.to_string());
        let (ahead, behind) = repo
            .ahead_behind(&upstream_branch, &ctx.commit_sha)
            .map_err(|e| ctx.error(Self::KIND, e))?;

        Ok(TaskResult::Upstream {
            item_idx: ctx.item_idx,
            upstream: UpstreamStatus {
                remote,
                ahead,
                behind,
            },
        })
    }
}

/// Task 10: CI/PR status
///
/// Always checks for open PRs/MRs regardless of upstream tracking.
/// For branch workflow/pipeline fallback (no PR), requires upstream tracking
/// to prevent false matches from similarly-named branches on the remote.
pub struct CiStatusTask;

impl Task for CiStatusTask {
    const KIND: TaskKind = TaskKind::CiStatus;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        let repo = ctx.repo();
        let repo_path = repo
            .worktree_root()
            .ok()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| ctx.repo_path.clone());

        let pr_status = ctx.branch.as_deref().and_then(|branch| {
            let has_upstream = repo.upstream_branch(branch).ok().flatten().is_some();
            PrStatus::detect(branch, &ctx.commit_sha, &repo_path, has_upstream)
        });

        Ok(TaskResult::CiStatus {
            item_idx: ctx.item_idx,
            pr_status,
        })
    }
}

/// Task 13: URL health check (port availability).
///
/// The URL itself is sent immediately after template expansion (in spawning code)
/// so it appears in normal styling right away. This task only checks if the port
/// is listening, and if not, the URL dims.
pub struct UrlStatusTask;

impl Task for UrlStatusTask {
    const KIND: TaskKind = TaskKind::UrlStatus;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        use std::net::{SocketAddr, TcpStream};
        use std::time::Duration;

        // URL already sent in spawning code; this task only checks port availability
        let Some(ref url) = ctx.item_url else {
            return Ok(TaskResult::UrlStatus {
                item_idx: ctx.item_idx,
                url: None,
                active: None,
            });
        };

        // Parse port from URL and check if it's listening
        // Skip health check in tests to avoid flaky results from random local processes
        let active = if std::env::var("WORKTRUNK_TEST_SKIP_URL_HEALTH_CHECK").is_ok() {
            Some(false)
        } else {
            parse_port_from_url(url).map(|port| {
                // Quick TCP connect check with 50ms timeout
                let addr = SocketAddr::from(([127, 0, 0, 1], port));
                TcpStream::connect_timeout(&addr, Duration::from_millis(50)).is_ok()
            })
        };

        // Return only active status (url=None to avoid overwriting the already-sent URL)
        Ok(TaskResult::UrlStatus {
            item_idx: ctx.item_idx,
            url: None,
            active,
        })
    }
}

/// Parse port number from a URL string (e.g., "http://localhost:12345" -> 12345)
pub(crate) fn parse_port_from_url(url: &str) -> Option<u16> {
    // Strip scheme
    let url = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    // Extract host:port (before path, query, or fragment)
    let host_port = url.split(&['/', '?', '#'][..]).next()?;
    let (_host, port_str) = host_port.rsplit_once(':')?;
    port_str.parse().ok()
}

// ============================================================================
// Collection Entry Points
// ============================================================================

fn collect_progressive(
    ctx: TaskContext,
    include_worktree_tasks: bool,
    options: &CollectOptions,
    tx: Sender<Result<TaskResult, TaskError>>,
    expected_results: &Arc<ExpectedResults>,
) {
    let spawner = TaskSpawner::new(tx, expected_results.clone());
    let skip = &options.skip_tasks;

    std::thread::scope(|s| {
        // Core tasks (always run)
        spawner.spawn_core_tasks(s, &ctx);
        if include_worktree_tasks {
            spawner.spawn_worktree_only_tasks(s, &ctx, skip);
        }
        spawner.spawn_optional_tasks(s, &ctx, skip);
    });
}

/// Collect worktree data progressively, sending results as each task completes.
///
/// Spawns parallel git operations (up to 10). Each task sends a TaskResult when it
/// completes, enabling progressive UI updates. Tasks in `options.skip_tasks` are not spawned.
///
/// # Parameters
/// - `default_branch`: Local default branch for informational stats (ahead/behind, branch diff)
/// - `target`: Effective target for integration checks (may be upstream if ahead)
pub fn collect_worktree_progressive(
    wt: &Worktree,
    item_idx: usize,
    default_branch: &str,
    target: &str,
    options: &CollectOptions,
    tx: Sender<Result<TaskResult, TaskError>>,
    expected_results: &Arc<ExpectedResults>,
) {
    // Expand URL template for this item (deferred from pre-skeleton)
    let item_url = options.url_template.as_ref().and_then(|template| {
        wt.branch.as_ref().and_then(|branch| {
            let mut vars = std::collections::HashMap::new();
            vars.insert("branch", branch.as_str());
            worktrunk::config::expand_template(template, &vars, false).ok()
        })
    });

    // Send URL immediately (before health check) so it appears in normal styling right away.
    // The health check task will later send url_active to dim if inactive.
    if let Some(ref url) = item_url {
        let _ = tx.send(Ok(TaskResult::UrlStatus {
            item_idx,
            url: Some(url.clone()),
            active: None,
        }));
    }

    let ctx = TaskContext {
        repo_path: wt.path.clone(),
        commit_sha: wt.head.clone(),
        branch: wt.branch.clone(),
        default_branch: Some(default_branch.to_string()),
        target: Some(target.to_string()),
        item_idx,
        item_url,
    };

    collect_progressive(ctx, true, options, tx, expected_results);
}

/// Collect branch data progressively, sending results as each task completes.
///
/// Spawns parallel git operations (up to 7, similar to worktrees but without working
/// tree operations). Tasks in `options.skip_tasks` are not spawned.
///
/// # Parameters
/// - `default_branch`: Local default branch for informational stats (ahead/behind, branch diff)
/// - `target`: Effective target for integration checks (may be upstream if ahead)
#[allow(clippy::too_many_arguments)]
pub fn collect_branch_progressive(
    branch_name: &str,
    commit_sha: &str,
    repo_path: &std::path::Path,
    item_idx: usize,
    default_branch: &str,
    target: &str,
    options: &CollectOptions,
    tx: Sender<Result<TaskResult, TaskError>>,
    expected_results: &Arc<ExpectedResults>,
) {
    // Expand URL template for this item (deferred from pre-skeleton)
    let item_url = options.url_template.as_ref().and_then(|template| {
        let mut vars = std::collections::HashMap::new();
        vars.insert("branch", branch_name);
        worktrunk::config::expand_template(template, &vars, false).ok()
    });

    // Send URL immediately (before health check) so it appears in normal styling right away.
    // The health check task will later send url_active to dim if inactive.
    if let Some(ref url) = item_url {
        let _ = tx.send(Ok(TaskResult::UrlStatus {
            item_idx,
            url: Some(url.clone()),
            active: None,
        }));
    }

    let ctx = TaskContext {
        repo_path: repo_path.to_path_buf(),
        commit_sha: commit_sha.to_string(),
        branch: Some(branch_name.to_string()),
        default_branch: Some(default_branch.to_string()),
        target: Some(target.to_string()),
        item_idx,
        item_url,
    };

    collect_progressive(ctx, false, options, tx, expected_results);
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Parse git status output to extract working tree status and conflict state.
/// Returns (WorkingTreeStatus, is_dirty, has_conflicts).
fn parse_working_tree_status(status_output: &str) -> (WorkingTreeStatus, bool, bool) {
    let mut has_untracked = false;
    let mut has_modified = false;
    let mut has_staged = false;
    let mut has_renamed = false;
    let mut has_deleted = false;
    let mut has_conflicts = false;

    for line in status_output.lines() {
        if line.len() < 2 {
            continue;
        }

        let bytes = line.as_bytes();
        let index_status = bytes[0] as char;
        let worktree_status = bytes[1] as char;

        if index_status == '?' && worktree_status == '?' {
            has_untracked = true;
        }

        // Worktree changes: M = modified, A = intent-to-add (git add -N), T = type change (file↔symlink)
        if matches!(worktree_status, 'M' | 'A' | 'T') {
            has_modified = true;
        }

        // Index changes: A = added, M = modified, C = copied, T = type change (file↔symlink)
        if matches!(index_status, 'A' | 'M' | 'C' | 'T') {
            has_staged = true;
        }

        if index_status == 'R' {
            has_renamed = true;
        }

        if index_status == 'D' || worktree_status == 'D' {
            has_deleted = true;
        }

        // Detect unmerged/conflicting paths (porcelain v1 two-letter codes)
        // Only U codes and AA/DD indicate actual merge conflicts.
        // AD/DA are normal staging states (staged then deleted, or deleted then restored).
        let is_unmerged_pair = matches!(
            (index_status, worktree_status),
            ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D')
        );
        if is_unmerged_pair {
            has_conflicts = true;
        }
    }

    let working_tree_status = WorkingTreeStatus::new(
        has_staged,
        has_modified,
        has_untracked,
        has_renamed,
        has_deleted,
    );

    let is_dirty = working_tree_status.is_dirty();

    (working_tree_status, is_dirty, has_conflicts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_status_ad_not_conflict() {
        // AD = added to index, deleted from worktree (not a conflict)
        let (status, is_dirty, has_conflicts) = parse_working_tree_status("AD file.txt\n");
        assert!(!has_conflicts, "AD should not be treated as conflict");
        assert!(is_dirty);
        assert!(status.staged, "AD should show staged symbol");
    }

    #[test]
    fn test_parse_status_da_not_conflict() {
        // DA = deleted from index, then restored to worktree (not a conflict)
        let (_, is_dirty, has_conflicts) = parse_working_tree_status("DA file.txt\n");
        assert!(!has_conflicts, "DA should not be treated as conflict");
        assert!(is_dirty);
    }

    #[test]
    fn test_parse_status_uu_is_conflict() {
        // UU = both modified (actual conflict)
        let (_, _, has_conflicts) = parse_working_tree_status("UU file.txt\n");
        assert!(has_conflicts, "UU should be treated as conflict");
    }

    #[test]
    fn test_parse_status_aa_is_conflict() {
        // AA = both added (actual conflict)
        let (_, _, has_conflicts) = parse_working_tree_status("AA file.txt\n");
        assert!(has_conflicts, "AA should be treated as conflict");
    }

    #[test]
    fn test_parse_status_dd_is_conflict() {
        // DD = both deleted (actual conflict)
        let (_, _, has_conflicts) = parse_working_tree_status("DD file.txt\n");
        assert!(has_conflicts, "DD should be treated as conflict");
    }

    #[test]
    fn test_parse_status_u_variants_are_conflicts() {
        // All U codes indicate conflicts
        for code in ["AU", "UA", "DU", "UD"] {
            let input = format!("{} file.txt\n", code);
            let (_, _, has_conflicts) = parse_working_tree_status(&input);
            assert!(has_conflicts, "{} should be treated as conflict", code);
        }
    }

    #[test]
    fn test_parse_status_md_not_conflict() {
        // MD = modified in index, deleted from worktree (not a conflict)
        let (_, is_dirty, has_conflicts) = parse_working_tree_status("MD file.txt\n");
        assert!(!has_conflicts, "MD should not be treated as conflict");
        assert!(is_dirty);
    }

    #[test]
    fn test_parse_status_intent_to_add() {
        // " A" = intent-to-add (git add -N): file recorded but content not staged
        let (status, is_dirty, has_conflicts) = parse_working_tree_status(" A file.txt\n");
        assert!(
            !has_conflicts,
            "intent-to-add should not be treated as conflict"
        );
        assert!(is_dirty, "intent-to-add should be dirty");
        assert!(status.modified, "intent-to-add should show modified symbol");
    }

    #[test]
    fn test_parse_status_type_change() {
        // " T" = type change in worktree (e.g., file changed to symlink)
        let (status, is_dirty, has_conflicts) = parse_working_tree_status(" T file.txt\n");
        assert!(
            !has_conflicts,
            "type change should not be treated as conflict"
        );
        assert!(is_dirty, "type change should be dirty");
        assert!(status.modified, "type change should show modified symbol");
    }

    #[test]
    fn test_parse_status_staged_type_change() {
        // "T " = type change in index (staged), no worktree changes
        // Example: file changed to symlink and the change is staged
        let (status, is_dirty, has_conflicts) = parse_working_tree_status("T  file.txt\n");
        assert!(
            !has_conflicts,
            "staged type change should not be treated as conflict"
        );
        assert!(is_dirty, "staged type change should be dirty");
        assert!(
            status.staged,
            "staged type change should show staged symbol (+)"
        );
    }

    #[test]
    fn test_parse_status_staged_type_change_with_worktree_mod() {
        // "TM" = type change in index, then modified in worktree
        let (status, is_dirty, has_conflicts) = parse_working_tree_status("TM file.txt\n");
        assert!(!has_conflicts);
        assert!(is_dirty, "should be dirty");
        assert!(
            status.staged,
            "should show staged symbol for index type change"
        );
        assert!(
            status.modified,
            "should show modified symbol for worktree modification"
        );
    }

    #[test]
    fn test_parse_port_from_url_basic() {
        assert_eq!(parse_port_from_url("http://localhost:12345"), Some(12345));
        assert_eq!(parse_port_from_url("https://localhost:8080"), Some(8080));
    }

    #[test]
    fn test_parse_port_from_url_with_path() {
        assert_eq!(
            parse_port_from_url("http://localhost:12345/path/to/page"),
            Some(12345)
        );
        assert_eq!(parse_port_from_url("http://localhost:3000/"), Some(3000));
    }

    #[test]
    fn test_parse_port_from_url_no_port() {
        assert_eq!(parse_port_from_url("http://localhost"), None);
        assert_eq!(parse_port_from_url("http://localhost/path"), None);
    }

    #[test]
    fn test_parse_port_from_url_no_scheme() {
        // Without http:// or https:// prefix, returns None
        assert_eq!(parse_port_from_url("localhost:8080"), None);
    }

    #[test]
    fn test_parse_port_from_url_edge_cases() {
        assert_eq!(parse_port_from_url("http://127.0.0.1:9000"), Some(9000));
        assert_eq!(parse_port_from_url("http://0.0.0.0:5000"), Some(5000));
    }
}
