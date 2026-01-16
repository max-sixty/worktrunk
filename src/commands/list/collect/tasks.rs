//! Task trait and implementations.
//!
//! Contains the `Task` trait interface and all 15 task implementations that
//! compute various git operations for worktrees and branches.

use worktrunk::git::{LineDiff, Repository};

use super::super::ci_status::PrStatus;
use super::super::model::{
    AheadBehind, BranchDiffTotals, CommitDetails, GitOperationState, UpstreamStatus,
    WorkingTreeStatus,
};
use super::types::{ErrorCause, TaskError, TaskKind, TaskResult};

// ============================================================================
// Task Context
// ============================================================================

/// Context for task computation. Cloned and moved into spawned threads.
///
/// Contains all data needed by any task. The `repo` field shares its cache
/// across all clones via `Arc<RepoCache>`, so parallel tasks benefit from
/// cached merge-base results, ahead/behind counts, default branch, and
/// integration target.
#[derive(Clone)]
pub struct TaskContext {
    /// Shared repository handle. All clones share the same cache via Arc.
    pub repo: Repository,
    /// The branch this task operates on. Contains branch name, commit SHA,
    /// and optional worktree path.
    ///
    /// For worktree-specific operations, use `self.worktree()` which returns
    /// `Some(WorkingTree)` only when this ref has a worktree path.
    pub branch_ref: worktrunk::git::BranchRef,
    pub item_idx: usize,
    /// Expanded URL for this item (from project config template).
    /// UrlStatusTask uses this to check if the port is listening.
    pub item_url: Option<String>,
}

impl TaskContext {
    pub(super) fn error(&self, kind: TaskKind, err: &anyhow::Error) -> TaskError {
        // Check if any error in the chain is a timeout
        let is_timeout = err.chain().any(|e| {
            e.downcast_ref::<std::io::Error>()
                .is_some_and(|io_err| io_err.kind() == std::io::ErrorKind::TimedOut)
        });

        let cause = if is_timeout {
            let kind_str: &'static str = kind.into();
            let sha = &self.branch_ref.commit_sha;
            let short_sha = &sha[..sha.len().min(8)];
            let branch = self.branch_ref.branch.as_deref().unwrap_or(short_sha);
            log::debug!("Task {} timed out for {}", kind_str, branch);
            ErrorCause::Timeout
        } else {
            ErrorCause::Other
        };
        TaskError::new(self.item_idx, kind, err.to_string(), cause)
    }

    /// Get the default branch (cached in Repository).
    ///
    /// Used for informational stats (ahead/behind, branch diff).
    /// Returns None if default branch cannot be determined.
    pub(super) fn default_branch(&self) -> Option<String> {
        self.repo.default_branch()
    }

    /// Get the integration target (cached in Repository).
    ///
    /// Used for integration checks (status symbols, safe deletion).
    /// Returns None if default branch cannot be determined.
    pub(super) fn integration_target(&self) -> Option<String> {
        self.repo.integration_target()
    }
}

// ============================================================================
// Task Trait
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

// ============================================================================
// Task Implementations
// ============================================================================

/// Task 1: Commit details (timestamp, message)
pub struct CommitDetailsTask;

impl Task for CommitDetailsTask {
    const KIND: TaskKind = TaskKind::CommitDetails;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        let repo = &ctx.repo;
        let (timestamp, commit_message) = repo
            .commit_details(&ctx.branch_ref.commit_sha)
            .map_err(|e| ctx.error(Self::KIND, &e))?;
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
        // When default_branch is None, return zero counts (cells show empty)
        let Some(base) = ctx.default_branch() else {
            return Ok(TaskResult::AheadBehind {
                item_idx: ctx.item_idx,
                counts: AheadBehind::default(),
                is_orphan: false,
            });
        };
        let repo = &ctx.repo;

        // Check for orphan branch (no common ancestor with default branch).
        // merge_base() is cached, so this is cheap after first call.
        let is_orphan = repo
            .merge_base(&base, &ctx.branch_ref.commit_sha)
            .map_err(|e| ctx.error(Self::KIND, &e))?
            .is_none();

        if is_orphan {
            return Ok(TaskResult::AheadBehind {
                item_idx: ctx.item_idx,
                counts: AheadBehind::default(),
                is_orphan: true,
            });
        }

        // Check cache first (populated by batch_ahead_behind if it ran).
        // Cache lookup has minor overhead (rev-parse for cache key + allocations),
        // but saves the expensive ahead_behind computation on cache hit.
        let (ahead, behind) = if let Some(branch) = ctx.branch_ref.branch.as_deref() {
            if let Some(counts) = repo.get_cached_ahead_behind(&base, branch) {
                counts
            } else {
                repo.ahead_behind(&base, &ctx.branch_ref.commit_sha)
                    .map_err(|e| ctx.error(Self::KIND, &e))?
            }
        } else {
            repo.ahead_behind(&base, &ctx.branch_ref.commit_sha)
                .map_err(|e| ctx.error(Self::KIND, &e))?
        };

        Ok(TaskResult::AheadBehind {
            item_idx: ctx.item_idx,
            counts: AheadBehind { ahead, behind },
            is_orphan: false,
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
        // When integration_target is None, return false (conservative: don't mark as integrated)
        let Some(base) = ctx.integration_target() else {
            return Ok(TaskResult::CommittedTreesMatch {
                item_idx: ctx.item_idx,
                committed_trees_match: false,
            });
        };
        let repo = &ctx.repo;
        // Use the item's commit instead of HEAD, since for branches without
        // worktrees, HEAD is the main worktree's HEAD.
        let committed_trees_match = repo
            .trees_match(&ctx.branch_ref.commit_sha, &base)
            .map_err(|e| ctx.error(Self::KIND, &e))?;
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
        let Some(branch) = ctx.branch_ref.branch.as_deref() else {
            return Ok(TaskResult::HasFileChanges {
                item_idx: ctx.item_idx,
                has_file_changes: true,
            });
        };
        // When integration_target is None, return true (conservative: assume has changes)
        let Some(target) = ctx.integration_target() else {
            return Ok(TaskResult::HasFileChanges {
                item_idx: ctx.item_idx,
                has_file_changes: true,
            });
        };
        let repo = &ctx.repo;
        let has_file_changes = repo
            .has_added_changes(branch, &target)
            .map_err(|e| ctx.error(Self::KIND, &e))?;

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
        let Some(branch) = ctx.branch_ref.branch.as_deref() else {
            return Ok(TaskResult::WouldMergeAdd {
                item_idx: ctx.item_idx,
                would_merge_add: true,
            });
        };
        // When integration_target is None, return true (conservative: assume would add)
        let Some(base) = ctx.integration_target() else {
            return Ok(TaskResult::WouldMergeAdd {
                item_idx: ctx.item_idx,
                would_merge_add: true,
            });
        };
        let repo = &ctx.repo;
        let would_merge_add = repo
            .would_merge_add_to_target(branch, &base)
            .map_err(|e| ctx.error(Self::KIND, &e))?;
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
        // When integration_target is None, return false (conservative: don't mark as ancestor)
        let Some(base) = ctx.integration_target() else {
            return Ok(TaskResult::IsAncestor {
                item_idx: ctx.item_idx,
                is_ancestor: false,
            });
        };
        let repo = &ctx.repo;
        let is_ancestor = repo
            .is_ancestor(&ctx.branch_ref.commit_sha, &base)
            .map_err(|e| ctx.error(Self::KIND, &e))?;

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
        // When default_branch is None, return empty diff (cells show empty)
        let Some(base) = ctx.default_branch() else {
            return Ok(TaskResult::BranchDiff {
                item_idx: ctx.item_idx,
                branch_diff: BranchDiffTotals::default(),
            });
        };
        let repo = &ctx.repo;
        let diff = repo
            .branch_diff_stats(&base, &ctx.branch_ref.commit_sha)
            .map_err(|e| ctx.error(Self::KIND, &e))?;

        Ok(TaskResult::BranchDiff {
            item_idx: ctx.item_idx,
            branch_diff: BranchDiffTotals { diff },
        })
    }
}

/// Task 5 (worktree only): Working tree diff + status flags
///
/// Runs `git status --porcelain` to get working tree status and computes diff stats.
pub struct WorkingTreeDiffTask;

impl Task for WorkingTreeDiffTask {
    const KIND: TaskKind = TaskKind::WorkingTreeDiff;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        // This task is only spawned for worktree items, so worktree path is always present.
        let wt = ctx
            .branch_ref
            .working_tree(&ctx.repo)
            .expect("WorkingTreeDiffTask requires a worktree");

        // Use --no-optional-locks to avoid index lock contention with WorkingTreeConflictsTask's
        // `git stash create` which needs the index lock.
        let status_output = wt
            .run_command(&["--no-optional-locks", "status", "--porcelain"])
            .map_err(|e| ctx.error(Self::KIND, &e))?;

        let (working_tree_status, is_dirty, has_conflicts) =
            parse_working_tree_status(&status_output);

        let working_tree_diff = if is_dirty {
            wt.working_tree_diff_stats()
                .map_err(|e| ctx.error(Self::KIND, &e))?
        } else {
            LineDiff::default()
        };

        Ok(TaskResult::WorkingTreeDiff {
            item_idx: ctx.item_idx,
            working_tree_diff,
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
        // When default_branch is None, return false (no conflicts can be detected)
        let Some(base) = ctx.default_branch() else {
            return Ok(TaskResult::MergeTreeConflicts {
                item_idx: ctx.item_idx,
                has_merge_tree_conflicts: false,
            });
        };
        let repo = &ctx.repo;
        let has_merge_tree_conflicts = repo
            .has_merge_conflicts(&base, &ctx.branch_ref.commit_sha)
            .map_err(|e| ctx.error(Self::KIND, &e))?;
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
        // When default_branch is None, return None (skip conflict check)
        let Some(base) = ctx.default_branch() else {
            return Ok(TaskResult::WorkingTreeConflicts {
                item_idx: ctx.item_idx,
                has_working_tree_conflicts: None,
            });
        };
        // This task is only spawned for worktree items, so worktree path is always present.
        let wt = ctx
            .branch_ref
            .working_tree(&ctx.repo)
            .expect("WorkingTreeConflictsTask requires a worktree");

        // Use --no-optional-locks to avoid index lock contention with WorkingTreeDiffTask.
        // Both tasks run in parallel, and `git stash create` below needs the index lock.
        let status_output = wt
            .run_command(&["--no-optional-locks", "status", "--porcelain"])
            .map_err(|e| ctx.error(Self::KIND, &e))?;

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
        let stash_result = wt.run_command(&["stash", "create"]);

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

        // Run merge-tree with the stash commit (repo-wide operation, doesn't need worktree)
        let has_conflicts = ctx
            .repo
            .has_merge_conflicts(&base, stash_sha)
            .map_err(|e| ctx.error(Self::KIND, &e))?;

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
        // This task is only spawned for worktree items, so worktree path is always present.
        let wt = ctx
            .branch_ref
            .working_tree(&ctx.repo)
            .expect("GitOperationTask requires a worktree");
        let git_operation = detect_git_operation(&wt);
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
        let repo = &ctx.repo;
        let user_marker = repo.user_marker(ctx.branch_ref.branch.as_deref());
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
        let repo = &ctx.repo;

        // No branch means no upstream
        let Some(branch) = ctx.branch_ref.branch.as_deref() else {
            return Ok(TaskResult::Upstream {
                item_idx: ctx.item_idx,
                upstream: UpstreamStatus::default(),
            });
        };

        // Get upstream branch (None is valid - just means no upstream configured)
        let upstream_branch = repo
            .upstream_branch(branch)
            .map_err(|e| ctx.error(Self::KIND, &e))?;
        let Some(upstream_branch) = upstream_branch else {
            return Ok(TaskResult::Upstream {
                item_idx: ctx.item_idx,
                upstream: UpstreamStatus::default(),
            });
        };

        let remote = upstream_branch.split_once('/').map(|(r, _)| r.to_string());
        let (ahead, behind) = repo
            .ahead_behind(&upstream_branch, &ctx.branch_ref.commit_sha)
            .map_err(|e| ctx.error(Self::KIND, &e))?;

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
        let repo = &ctx.repo;
        let pr_status = ctx.branch_ref.branch.as_deref().and_then(|branch| {
            let has_upstream = repo.upstream_branch(branch).ok().flatten().is_some();
            PrStatus::detect(repo, branch, &ctx.branch_ref.commit_sha, has_upstream)
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

// ============================================================================
// Helper Functions
// ============================================================================

/// Detect if a worktree is in the middle of a git operation (rebase/merge).
pub(crate) fn detect_git_operation(wt: &worktrunk::git::WorkingTree<'_>) -> GitOperationState {
    if wt.is_rebasing().unwrap_or(false) {
        GitOperationState::Rebase
    } else if wt.is_merging().unwrap_or(false) {
        GitOperationState::Merge
    } else {
        GitOperationState::None
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

/// Parse git status output to extract working tree status and conflict state.
/// Returns (WorkingTreeStatus, is_dirty, has_conflicts).
pub(super) fn parse_working_tree_status(status_output: &str) -> (WorkingTreeStatus, bool, bool) {
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
