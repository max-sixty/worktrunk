//! Task trait and implementations.
//!
//! Contains the `Task` trait interface and all 16 task implementations that
//! compute various git operations for worktrees and branches.

use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use anyhow::Context;
use worktrunk::git::{LineDiff, Repository};

use super::super::ci_status::{CiBranchName, PrStatus};
use super::super::model::{
    ActiveGitOperation, AheadBehind, BranchDiffTotals, UpstreamStatus, WorkingTreeStatus,
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
    /// LLM command for summary generation (from commit.generation config).
    pub llm_command: Option<String>,
    /// Default branch resolved for this list invocation. Populated from
    /// the collect-phase check that verifies the persisted value still
    /// resolves locally; `None` when unset or stale. Tasks read this
    /// instead of `repo.default_branch()` so a stale persisted value
    /// degrades silently (empty cells) here rather than emitting a cascade
    /// of "ambiguous argument" errors.
    pub default_branch: Option<String>,
    /// Integration target (`default_branch`, or its upstream when ahead).
    /// `None` when the default branch is unset or stale — keeps the same
    /// silent-skip contract as `default_branch` for tasks that compare
    /// against the integration target.
    pub integration_target: Option<String>,
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
            let branch = self.branch_ref.short_name().unwrap_or(short_sha);
            log::debug!("Task {} timed out for {}", kind_str, branch);
            ErrorCause::Timeout
        } else {
            ErrorCause::Other
        };
        TaskError::new(self.item_idx, kind, err.to_string(), cause)
    }

    /// Get the default branch resolved for this list invocation.
    ///
    /// Used for informational stats (ahead/behind, branch diff). Returns
    /// `None` if default branch cannot be determined, or if the persisted
    /// value is stale (see `TaskContext::default_branch` docs).
    pub(super) fn default_branch(&self) -> Option<String> {
        self.default_branch.clone()
    }

    /// Get the integration target resolved for this list invocation.
    ///
    /// Used for integration checks (status symbols, safe deletion).
    /// Returns `None` if default branch cannot be determined or is stale.
    pub(super) fn integration_target(&self) -> Option<String> {
        self.integration_target.clone()
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

/// Task: Ahead/behind counts vs local default branch (informational stats)
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

        // When the ref has a branch name, compute counts against the branch — not
        // the worktree's current HEAD sha. During rebase/merge conflicts, HEAD is
        // transiently at a different commit than the branch tip, so using the sha
        // would report misleading counts (e.g., `0/0 same_commit` when the branch
        // is actually diverged). The batch path already uses branch names, so this
        // keeps both paths consistent.
        let head = ctx
            .branch_ref
            .full_ref()
            .unwrap_or(&ctx.branch_ref.commit_sha);
        let (ahead, behind) = repo
            .ahead_behind(&base, head)
            .map_err(|e| ctx.error(Self::KIND, &e))?;

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
        // Prefer the branch name when present: during a rebase-in-progress, the
        // worktree's HEAD is at a transient replayed commit, so using commit_sha
        // would compare the wrong tree. For branch-only items the two are
        // equivalent; for detached HEAD, commit_sha is the only option.
        let ref_to_check = ctx
            .branch_ref
            .full_ref()
            .unwrap_or(&ctx.branch_ref.commit_sha);
        let committed_trees_match = repo
            .trees_match(ref_to_check, &base)
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
        let Some(branch) = ctx.branch_ref.full_ref() else {
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

/// Task 3b: Merge simulation + patch-id fallback
///
/// Delegates to [`Repository::merge_integration_probe()`], which runs:
///
/// 1. `merge-tree --write-tree` — simulates merging branch into target. If the
///    result tree equals target's tree, the branch is integrated (`MergeAddsNothing`).
/// 2. `patch-id` fallback — only when merge-tree conflicts (returns `None`).
///    Computes the branch's entire diff as a single patch-id and checks if any
///    target commit matches (`PatchIdMatch`). Detects squash merges where target
///    later modified the same files.
///
/// These are bundled in one task because patch-id only runs when merge-tree
/// conflicts — it needs the merge-tree result first. Splitting them into separate
/// parallel tasks would either waste work (running patch-id unconditionally) or
/// require two-phase scheduling.
pub struct WouldMergeAddTask;

impl Task for WouldMergeAddTask {
    const KIND: TaskKind = TaskKind::WouldMergeAdd;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        // No branch name (detached HEAD) - return conservative default (assume would add)
        let Some(branch) = ctx.branch_ref.full_ref() else {
            return Ok(TaskResult::WouldMergeAdd {
                item_idx: ctx.item_idx,
                would_merge_add: true,
                is_patch_id_match: false,
            });
        };
        // When integration_target is None, return true (conservative: assume would add)
        let Some(base) = ctx.integration_target() else {
            return Ok(TaskResult::WouldMergeAdd {
                item_idx: ctx.item_idx,
                would_merge_add: true,
                is_patch_id_match: false,
            });
        };
        let probe = ctx
            .repo
            .merge_integration_probe(branch, &base)
            .map_err(|e| ctx.error(Self::KIND, &e))?;
        Ok(TaskResult::WouldMergeAdd {
            item_idx: ctx.item_idx,
            would_merge_add: probe.would_merge_add,
            is_patch_id_match: probe.is_patch_id_match,
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
        // Prefer the branch name when present — see `CommittedTreesMatchTask`
        // for rationale (rebase-in-progress transient HEAD).
        let ref_to_check = ctx
            .branch_ref
            .full_ref()
            .unwrap_or(&ctx.branch_ref.commit_sha);
        let is_ancestor = repo
            .is_ancestor(ref_to_check, &base)
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
        // Prefer the branch name when present — see `CommittedTreesMatchTask`
        // for rationale (rebase-in-progress transient HEAD).
        let ref_to_check = ctx
            .branch_ref
            .full_ref()
            .unwrap_or(&ctx.branch_ref.commit_sha);
        let diff = repo
            .branch_diff_stats(&base, ref_to_check)
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
            .ok_or_else(|| ctx.error(Self::KIND, &anyhow::anyhow!("requires a worktree")))?;

        // Shared cache: WorkingTreeConflictsTask also needs porcelain. First
        // accessor spawns the subprocess; second hits the cache. Uses
        // --no-optional-locks to avoid index lock contention with `git write-tree`.
        let status_output = wt
            .status_porcelain_cached()
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
        // Prefer the branch name when present — see `CommittedTreesMatchTask`
        // for rationale (rebase-in-progress transient HEAD).
        let ref_to_check = ctx
            .branch_ref
            .full_ref()
            .unwrap_or(&ctx.branch_ref.commit_sha);
        let has_merge_tree_conflicts = repo
            .has_merge_conflicts(&base, ref_to_check)
            .map_err(|e| ctx.error(Self::KIND, &e))?;
        Ok(TaskResult::MergeTreeConflicts {
            item_idx: ctx.item_idx,
            has_merge_tree_conflicts,
        })
    }
}

/// Task 6b (worktree only): Working tree conflict check
///
/// For dirty worktrees, builds a tree SHA from the index (plus untracked
/// files if present) via `git write-tree`, then checks for merge conflicts
/// against the default branch. Much cheaper than `git stash create` (~15ms
/// vs ~50-265ms) because it reads the index directly instead of creating a
/// full stash commit with working-tree diffing.
///
/// Returns None if working tree is clean (caller should fall back to
/// MergeTreeConflicts).
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
            .ok_or_else(|| ctx.error(Self::KIND, &anyhow::anyhow!("requires a worktree")))?;

        // Shared cache with WorkingTreeDiffTask — single subprocess per worktree.
        let status_output = wt
            .status_porcelain_cached()
            .map_err(|e| ctx.error(Self::KIND, &e))?;

        let is_dirty = !status_output.trim().is_empty();

        if !is_dirty {
            // Clean working tree - return None to signal "use commit-based check"
            return Ok(TaskResult::WorkingTreeConflicts {
                item_idx: ctx.item_idx,
                has_working_tree_conflicts: None,
            });
        }

        // Unmerged entries (UU, AU, UA, DU, UD, DD, AA) mean a merge/rebase
        // conflict is in progress. Fall back to the commit-based check to
        // preserve prior behavior — write-tree on unmerged entries would
        // produce a tree with conflict markers as content.
        let has_unmerged = has_unmerged_entries(&status_output);
        if has_unmerged {
            return Ok(TaskResult::WorkingTreeConflicts {
                item_idx: ctx.item_idx,
                has_working_tree_conflicts: None,
            });
        }

        // Porcelain format: XY where X=index, Y=working-tree.
        // Fast path when all changes are staged (Y is space for every line):
        // write-tree on the real index is sufficient.
        // Slow path when there are unstaged modifications (Y != ' ') or
        // untracked files ('??'): copy index, `git add -A`, write-tree.
        let needs_working_tree = status_output
            .lines()
            .any(|l| l.starts_with("??") || l.as_bytes().get(1) != Some(&b' '));

        let tree_sha = if needs_working_tree {
            write_tree_with_working_tree(&wt).map_err(|e| ctx.error(Self::KIND, &e))?
        } else {
            wt.run_command(&["write-tree"])
                .map(|s| s.trim().to_string())
                .map_err(|e| ctx.error(Self::KIND, &e))?
        };

        let has_conflicts = ctx
            .repo
            .has_merge_conflicts_by_tree(&base, &ctx.branch_ref.commit_sha, &tree_sha)
            .map_err(|e| ctx.error(Self::KIND, &e))?;

        Ok(TaskResult::WorkingTreeConflicts {
            item_idx: ctx.item_idx,
            has_working_tree_conflicts: Some(has_conflicts),
        })
    }
}

/// Build a tree SHA representing the full working tree state (staged +
/// unstaged + untracked) by staging everything into a temporary index.
///
/// Copies the real index (preserving git's stat cache for unchanged files),
/// then `git add -A` to stage all modifications and untracked files, then
/// `git write-tree` to produce the tree SHA. The real index is untouched.
fn write_tree_with_working_tree(wt: &worktrunk::git::WorkingTree) -> anyhow::Result<String> {
    use worktrunk::shell_exec::Cmd;

    let git_dir = wt.git_dir()?;
    let worktree_root = wt.root()?;
    let real_index = git_dir.join("index");
    let log_ctx = wt
        .path()
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(".")
        .to_string();

    let temp_index = tempfile::NamedTempFile::new().context("Failed to create temporary index")?;
    std::fs::copy(&real_index, temp_index.path()).context("Failed to copy index file")?;
    let temp_index_path = temp_index
        .path()
        .to_str()
        .context("Temporary index path is not valid UTF-8")?;

    // Stage all changes (unstaged modifications + untracked files)
    Cmd::new("git")
        .args(["add", "-A"])
        .current_dir(&worktree_root)
        .context(&log_ctx)
        .env("GIT_INDEX_FILE", temp_index_path)
        .run()
        .context("Failed to stage working tree changes")?;

    let output = Cmd::new("git")
        .args(["write-tree"])
        .current_dir(&worktree_root)
        .context(&log_ctx)
        .env("GIT_INDEX_FILE", temp_index_path)
        .run()
        .context("Failed to write tree from temporary index")?;

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
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
            .ok_or_else(|| ctx.error(Self::KIND, &anyhow::anyhow!("requires a worktree")))?;
        let git_operation = detect_active_git_operation(&wt);
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
        let user_marker = repo.user_marker(ctx.branch_ref.short_name());
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
        let Some(branch) = ctx.branch_ref.short_name() else {
            return Ok(TaskResult::Upstream {
                item_idx: ctx.item_idx,
                upstream: UpstreamStatus::default(),
            });
        };

        // Get upstream branch (None is valid - just means no upstream configured)
        let upstream_branch = repo
            .branch(branch)
            .upstream()
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
///
/// Remote branches (e.g., "origin/feature") are treated as having upstream
/// by definition - they ARE the upstream. This enables workflow/pipeline
/// fallback for remote-only branches shown via `wt list --remotes`.
pub struct CiStatusTask;

impl Task for CiStatusTask {
    const KIND: TaskKind = TaskKind::CiStatus;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        let repo = &ctx.repo;
        let pr_status = ctx.branch_ref.short_name().and_then(|branch| {
            // Use from_branch_ref with the authoritative is_remote flag
            // rather than guessing from the branch name
            let ci_branch = CiBranchName::from_branch_ref(branch, ctx.branch_ref.is_remote());
            PrStatus::detect(repo, &ci_branch, &ctx.branch_ref.commit_sha)
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

/// Task 14: LLM-generated branch summary (`--full` + `[list] summary = true` + LLM command)
pub struct SummaryGenerateTask;

impl Task for SummaryGenerateTask {
    const KIND: TaskKind = TaskKind::SummaryGenerate;

    fn compute(ctx: TaskContext) -> Result<TaskResult, TaskError> {
        let Some(ref llm_command) = ctx.llm_command else {
            return Err(ctx.error(
                Self::KIND,
                &anyhow::anyhow!("SummaryGenerateTask requires llm_command"),
            ));
        };

        let branch = ctx.branch_ref.short_name().unwrap_or("(detached)");
        let worktree_path = ctx.branch_ref.worktree_path.as_deref();

        let summary = crate::summary::generate_summary_core(
            branch,
            &ctx.branch_ref.commit_sha,
            worktree_path,
            llm_command,
            &ctx.repo,
        )
        .map_err(|e| ctx.error(Self::KIND, &e))?;

        // Extract subject line (first line) for the table column
        let subject = summary.as_deref().map(first_line);

        Ok(TaskResult::SummaryGenerate {
            item_idx: ctx.item_idx,
            summary: subject,
        })
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Extract the first non-empty line from a string (the subject line of a summary).
fn first_line(s: &str) -> String {
    s.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or(s)
        .to_string()
}

/// Detect if a worktree is in the middle of a git operation (rebase/merge).
pub(crate) fn detect_active_git_operation(
    wt: &worktrunk::git::WorkingTree<'_>,
) -> ActiveGitOperation {
    if wt.is_rebasing().unwrap_or(false) {
        ActiveGitOperation::Rebase
    } else if wt.is_merging().unwrap_or(false) {
        ActiveGitOperation::Merge
    } else {
        ActiveGitOperation::None
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

/// Check if `git status --porcelain` output contains unmerged entries.
///
/// All seven unmerged status codes: UU, AU, UA, DU, UD, DD, AA.
/// Five contain `U`; `DD` and `AA` do not and must be matched explicitly.
fn has_unmerged_entries(status_output: &str) -> bool {
    status_output.lines().any(|l| {
        l.len() >= 2 && {
            let xy = &l.as_bytes()[0..2];
            xy.contains(&b'U') || xy == b"AA" || xy == b"DD"
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_first_line_simple() {
        assert_eq!(first_line("Add feature\n\nDetails here"), "Add feature");
    }

    #[test]
    fn test_first_line_skips_empty() {
        assert_eq!(first_line("\n\nAdd feature\nMore"), "Add feature");
    }

    #[test]
    fn test_first_line_single_line() {
        assert_eq!(first_line("Single line"), "Single line");
    }

    #[test]
    fn test_first_line_empty_string() {
        assert_eq!(first_line(""), "");
    }

    #[test]
    fn unmerged_entries_detected_with_u() {
        assert!(has_unmerged_entries("UU src/main.rs"));
        assert!(has_unmerged_entries("AU src/main.rs"));
        assert!(has_unmerged_entries("UA src/main.rs"));
        assert!(has_unmerged_entries("DU src/main.rs"));
        assert!(has_unmerged_entries("UD src/main.rs"));
    }

    #[test]
    fn unmerged_entries_detected_aa_dd() {
        assert!(has_unmerged_entries("AA src/main.rs"));
        assert!(has_unmerged_entries("DD src/main.rs"));
    }

    #[test]
    fn unmerged_entries_mixed_status() {
        assert!(has_unmerged_entries("M  src/lib.rs\nAA src/main.rs"));
        assert!(has_unmerged_entries("?? untracked.txt\nDD deleted.rs"));
    }

    #[test]
    fn unmerged_entries_not_detected_for_normal_status() {
        assert!(!has_unmerged_entries("M  src/main.rs"));
        assert!(!has_unmerged_entries("A  src/new.rs"));
        assert!(!has_unmerged_entries("D  src/old.rs"));
        assert!(!has_unmerged_entries("?? untracked.txt"));
        assert!(!has_unmerged_entries(""));
    }
}
