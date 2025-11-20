//! Progressive worktree collection with parallel git operations.
//!
//! This module contains the implementation of cell-by-cell progressive rendering.
//! Git operations run in parallel and send updates as they complete.
//!
//! TODO(error-handling): Current implementation silently swallows git errors
//! and logs warnings to stderr. Consider whether failures should:
//! - Propagate to user (fail-fast)
//! - Show error placeholder in UI
//! - Continue silently (current behavior)

use crossbeam_channel::Sender;
use std::path::PathBuf;
use worktrunk::git::{LineDiff, Repository, Worktree};
use worktrunk::path::format_path_for_display;

use super::ci_status::PrStatus;
use super::collect::{CellUpdate, detect_worktree_state};
use super::model::{AheadBehind, BranchDiffTotals, CommitDetails, UpstreamStatus};

/// Context for spawning parallel tasks.
struct TaskContext {
    repo_path: PathBuf,
    commit_sha: String,
    branch: Option<String>,
    base_branch: Option<String>,
    item_idx: usize,
}

/// Spawn task 1: Commit details (timestamp, message)
fn spawn_commit_details<'scope>(
    s: &'scope std::thread::Scope<'scope, '_>,
    ctx: &TaskContext,
    tx: Sender<CellUpdate>,
) {
    let item_idx = ctx.item_idx;
    let sha = ctx.commit_sha.clone();
    let path = ctx.repo_path.clone();
    s.spawn(move || {
        let repo = Repository::at(&path);
        // TODO: Handle errors - for now, simplest thing is to skip on error
        if let (Ok(timestamp), Ok(commit_message)) =
            (repo.commit_timestamp(&sha), repo.commit_message(&sha))
        {
            let _ = tx.send(CellUpdate::CommitDetails {
                item_idx,
                commit: CommitDetails {
                    timestamp,
                    commit_message,
                },
            });
        }
    });
}

/// Spawn task 2: Ahead/behind counts
fn spawn_ahead_behind<'scope>(
    s: &'scope std::thread::Scope<'scope, '_>,
    ctx: &TaskContext,
    tx: Sender<CellUpdate>,
) {
    if let Some(base) = ctx.base_branch.as_deref() {
        let item_idx = ctx.item_idx;
        let sha = ctx.commit_sha.clone();
        let path = ctx.repo_path.clone();
        let base = base.to_string();
        s.spawn(move || {
            let repo = Repository::at(&path);
            // TODO: Handle errors
            if let Ok((ahead, behind)) = repo.ahead_behind(&base, &sha) {
                let _ = tx.send(CellUpdate::AheadBehind {
                    item_idx,
                    counts: AheadBehind { ahead, behind },
                });
            }
        });
    }
}

/// Spawn task 3: Branch diff
fn spawn_branch_diff<'scope>(
    s: &'scope std::thread::Scope<'scope, '_>,
    ctx: &TaskContext,
    tx: Sender<CellUpdate>,
) {
    if let Some(base) = ctx.base_branch.as_deref() {
        let item_idx = ctx.item_idx;
        let sha = ctx.commit_sha.clone();
        let path = ctx.repo_path.clone();
        let base = base.to_string();
        s.spawn(move || {
            let repo = Repository::at(&path);
            // TODO: Handle errors
            if let Ok(diff) = repo.branch_diff_stats(&base, &sha) {
                let _ = tx.send(CellUpdate::BranchDiff {
                    item_idx,
                    branch_diff: BranchDiffTotals { diff },
                });
            }
        });
    }
}

/// Spawn task 4 (worktree only): Working tree diff + status symbols
fn spawn_working_tree_diff<'scope>(
    s: &'scope std::thread::Scope<'scope, '_>,
    ctx: &TaskContext,
    tx: Sender<CellUpdate>,
) {
    let item_idx = ctx.item_idx;
    let path = ctx.repo_path.clone();
    let base = ctx.base_branch.clone();
    s.spawn(move || {
        let repo = Repository::at(&path);
        // TODO: Handle errors
        if let Ok(status_output) = repo.run_command(&["status", "--porcelain"]) {
            // Parse status to get symbols and is_dirty
            let (working_tree_symbols, is_dirty) = parse_status_for_symbols(&status_output);

            // Get working tree diff
            let working_tree_diff = if is_dirty {
                repo.working_tree_diff_stats().unwrap_or_default()
            } else {
                LineDiff::default()
            };

            // Get diff with main
            let working_tree_diff_with_main = repo
                .working_tree_diff_with_base(base.as_deref(), is_dirty)
                .ok()
                .flatten();

            let _ = tx.send(CellUpdate::WorkingTreeDiff {
                item_idx,
                working_tree_diff,
                working_tree_diff_with_main,
                working_tree_symbols,
                is_dirty,
            });
        }
    });
}

/// Spawn task 5: Conflicts check
fn spawn_conflicts<'scope>(
    s: &'scope std::thread::Scope<'scope, '_>,
    ctx: &TaskContext,
    check_conflicts: bool,
    send_default_on_skip: bool,
    tx: Sender<CellUpdate>,
) {
    let item_idx = ctx.item_idx;
    if check_conflicts && let Some(base) = ctx.base_branch.as_deref() {
        let sha = ctx.commit_sha.clone();
        let path = ctx.repo_path.clone();
        let base = base.to_string();
        s.spawn(move || {
            let repo = Repository::at(&path);
            // TODO: Handle errors
            let has_conflicts = repo.has_merge_conflicts(&base, &sha).unwrap_or(false);
            let _ = tx.send(CellUpdate::Conflicts {
                item_idx,
                has_conflicts,
            });
        });
    } else if send_default_on_skip {
        // Send default value when not checking conflicts (worktree behavior)
        let _ = tx.send(CellUpdate::Conflicts {
            item_idx,
            has_conflicts: false,
        });
    }
    // Branch behavior: don't send anything when not checking
}

/// Spawn task 6 (worktree only): Worktree state detection
fn spawn_worktree_state<'scope>(
    s: &'scope std::thread::Scope<'scope, '_>,
    ctx: &TaskContext,
    tx: Sender<CellUpdate>,
) {
    let item_idx = ctx.item_idx;
    let path = ctx.repo_path.clone();
    s.spawn(move || {
        let repo = Repository::at(&path);
        let worktree_state = detect_worktree_state(&repo);
        let _ = tx.send(CellUpdate::WorktreeState {
            item_idx,
            worktree_state,
        });
    });
}

/// Spawn task 7 (worktree only): User status
fn spawn_user_status<'scope>(
    s: &'scope std::thread::Scope<'scope, '_>,
    ctx: &TaskContext,
    tx: Sender<CellUpdate>,
) {
    let item_idx = ctx.item_idx;
    let path = ctx.repo_path.clone();
    let branch = ctx.branch.clone();
    s.spawn(move || {
        let repo = Repository::at(&path);
        let user_status = repo.user_status(branch.as_deref());
        let _ = tx.send(CellUpdate::UserStatus {
            item_idx,
            user_status,
        });
    });
}

/// Spawn task 8: Upstream status
fn spawn_upstream<'scope>(
    s: &'scope std::thread::Scope<'scope, '_>,
    ctx: &TaskContext,
    verbose_errors: bool,
    tx: Sender<CellUpdate>,
) {
    let item_idx = ctx.item_idx;
    let branch = ctx.branch.clone();
    let sha = ctx.commit_sha.clone();
    let path = ctx.repo_path.clone();
    s.spawn(move || {
        let repo = Repository::at(&path);
        let upstream = branch
            .as_deref()
            .and_then(|branch| match repo.upstream_branch(branch) {
                Ok(Some(upstream_branch)) => {
                    let remote = upstream_branch.split_once('/').map(|(r, _)| r.to_string());
                    match repo.ahead_behind(&upstream_branch, &sha) {
                        Ok((ahead, behind)) => Some(UpstreamStatus {
                            remote,
                            ahead,
                            behind,
                        }),
                        Err(e) => {
                            if verbose_errors {
                                eprintln!(
                                    "Warning: ahead_behind failed for {}: {}",
                                    format_path_for_display(&path),
                                    e
                                );
                            }
                            None
                        }
                    }
                }
                Ok(None) => None, // No upstream configured
                Err(e) => {
                    if verbose_errors {
                        eprintln!(
                            "Warning: upstream_branch failed for {}: {}",
                            format_path_for_display(&path),
                            e
                        );
                    }
                    None
                }
            })
            .unwrap_or_default();
        let _ = tx.send(CellUpdate::Upstream { item_idx, upstream });
    });
}

/// Spawn task 9: CI/PR status
fn spawn_ci_status<'scope>(
    s: &'scope std::thread::Scope<'scope, '_>,
    ctx: &TaskContext,
    fetch_ci: bool,
    tx: Sender<CellUpdate>,
) {
    if !fetch_ci {
        return;
    }

    let item_idx = ctx.item_idx;
    let branch = ctx.branch.clone();
    let sha = ctx.commit_sha.clone();
    let path = ctx.repo_path.clone();
    s.spawn(move || {
        // Use the repository root if available; fall back to the provided path.
        // This works for both worktrees and branches and keeps fork detection
        // behavior consistent with GH/GL CLI expectations.
        let repo_path = Repository::at(&path).worktree_root().ok().unwrap_or(path);

        let pr_status = branch
            .as_deref()
            .and_then(|branch| PrStatus::detect(branch, &sha, &repo_path));

        let _ = tx.send(CellUpdate::CiStatus {
            item_idx,
            pr_status,
        });
    });
}

/// Collect worktree data progressively, sending cell updates as each task completes.
///
/// Spawns 9 parallel git operations:
/// 1. Commit details (timestamp, message)
/// 2. Ahead/behind counts
/// 3. Branch diff stats
/// 4. Working tree diff + status symbols
/// 5. Conflicts check
/// 6. Worktree state detection
/// 7. User status from git config
/// 8. Upstream tracking status
/// 9. CI/PR status
///
/// Each task sends a CellUpdate when it completes, enabling progressive UI updates.
/// Errors are handled with TODO for simplicity (simplest thing for now).
pub fn collect_worktree_progressive(
    wt: &Worktree,
    primary: &Worktree,
    item_idx: usize,
    fetch_ci: bool,
    check_conflicts: bool,
    tx: Sender<CellUpdate>,
) {
    let base_branch = primary
        .branch
        .as_deref()
        .filter(|_| wt.path != primary.path);

    let ctx = TaskContext {
        repo_path: wt.path.clone(),
        commit_sha: wt.head.clone(),
        branch: wt.branch.clone(),
        base_branch: base_branch.map(String::from),
        item_idx,
    };

    std::thread::scope(|s| {
        spawn_commit_details(s, &ctx, tx.clone());
        spawn_ahead_behind(s, &ctx, tx.clone());
        spawn_branch_diff(s, &ctx, tx.clone());
        spawn_working_tree_diff(s, &ctx, tx.clone());
        spawn_conflicts(s, &ctx, check_conflicts, true, tx.clone());
        spawn_worktree_state(s, &ctx, tx.clone());
        spawn_user_status(s, &ctx, tx.clone());
        spawn_upstream(s, &ctx, true, tx.clone());
        spawn_ci_status(s, &ctx, fetch_ci, tx);
    });
}

/// Parse git status output to extract working tree symbols.
/// Returns (symbols, is_dirty).
fn parse_status_for_symbols(status_output: &str) -> (String, bool) {
    let mut has_untracked = false;
    let mut has_modified = false;
    let mut has_staged = false;
    let mut has_renamed = false;
    let mut has_deleted = false;

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

        if worktree_status == 'M' {
            has_modified = true;
        }

        if index_status == 'A' || index_status == 'M' || index_status == 'C' {
            has_staged = true;
        }

        if index_status == 'R' {
            has_renamed = true;
        }

        if index_status == 'D' || worktree_status == 'D' {
            has_deleted = true;
        }
    }

    // Build working tree string
    let mut working_tree = String::new();
    if has_untracked {
        working_tree.push('?');
    }
    if has_modified {
        working_tree.push('!');
    }
    if has_staged {
        working_tree.push('+');
    }
    if has_renamed {
        working_tree.push('»');
    }
    if has_deleted {
        working_tree.push('✘');
    }

    let is_dirty = has_untracked || has_modified || has_staged || has_renamed || has_deleted;

    (working_tree, is_dirty)
}

/// Collect branch data progressively, sending cell updates as each task completes.
///
/// Spawns 6 parallel git operations (similar to worktrees but without working tree operations):
/// 1. Commit details (timestamp, message)
/// 2. Ahead/behind counts
/// 3. Branch diff stats
/// 4. Upstream tracking status
/// 5. Conflicts check
/// 6. CI/PR status
pub fn collect_branch_progressive(
    branch_name: &str,
    commit_sha: &str,
    primary: &Worktree,
    item_idx: usize,
    fetch_ci: bool,
    check_conflicts: bool,
    tx: Sender<CellUpdate>,
) {
    let ctx = TaskContext {
        repo_path: primary.path.clone(),
        commit_sha: commit_sha.to_string(),
        branch: Some(branch_name.to_string()),
        base_branch: primary.branch.as_deref().map(String::from),
        item_idx,
    };

    std::thread::scope(|s| {
        spawn_commit_details(s, &ctx, tx.clone());
        spawn_ahead_behind(s, &ctx, tx.clone());
        spawn_branch_diff(s, &ctx, tx.clone());
        spawn_upstream(s, &ctx, false, tx.clone());
        spawn_conflicts(s, &ctx, check_conflicts, false, tx.clone());
        spawn_ci_status(s, &ctx, fetch_ci, tx);
    });
}
