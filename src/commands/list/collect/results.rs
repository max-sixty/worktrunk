//! Result processing and draining.
//!
//! Contains the logic for processing task results:
//! - `drain_results()` - drain channel and apply results to items
//! - `apply_default()` - apply defaults for failed tasks

use crossbeam_channel as chan;
use worktrunk::git::LineDiff;

use super::super::model::{CommitDetails, ItemKind, ListItem, UpstreamStatus, WorkingTreeStatus};
use super::execution::ExpectedResults;
use super::types::{DrainOutcome, MissingResult, StatusContext, TaskError, TaskKind, TaskResult};

/// Apply default values for a failed task.
///
/// When a task fails, we still need to populate the item fields with sensible
/// defaults so the UI can render. This centralizes all default logic in one place.
pub(super) fn apply_default(
    items: &mut [ListItem],
    status_contexts: &mut [StatusContext],
    error: &TaskError,
) {
    let idx = error.item_idx;
    match error.kind {
        TaskKind::CommitDetails => {
            items[idx].commit = Some(CommitDetails::default());
        }
        TaskKind::AheadBehind => {
            // Leave as None — UI shows `⋯` for not-loaded tasks
            // Conservative: don't claim orphan if we couldn't check
            items[idx].is_orphan = Some(false);
        }
        TaskKind::CommittedTreesMatch => {
            // Conservative: don't claim integrated if we couldn't check
            items[idx].committed_trees_match = Some(false);
        }
        TaskKind::HasFileChanges => {
            // Conservative: assume has changes if we couldn't check
            items[idx].has_file_changes = Some(true);
        }
        TaskKind::WouldMergeAdd => {
            // Conservative: assume would add changes if we couldn't check
            items[idx].would_merge_add = Some(true);
        }
        TaskKind::IsAncestor => {
            // Conservative: don't claim merged if we couldn't check
            items[idx].is_ancestor = Some(false);
        }
        TaskKind::BranchDiff => {
            // Leave as None — UI shows `…` for skipped/failed tasks
        }
        TaskKind::WorkingTreeDiff => {
            if let ItemKind::Worktree(data) = &mut items[idx].kind {
                data.working_tree_diff = Some(LineDiff::default());
            } else {
                debug_assert!(false, "WorkingTreeDiff task spawned for non-worktree item");
            }
            status_contexts[idx].working_tree_status = Some(WorkingTreeStatus::default());
            status_contexts[idx].has_conflicts = false;
        }
        TaskKind::MergeTreeConflicts => {
            // Don't show conflict symbol if we couldn't check
            status_contexts[idx].has_merge_tree_conflicts = false;
        }
        TaskKind::WorkingTreeConflicts => {
            // Fall back to commit-based check on failure
            status_contexts[idx].has_working_tree_conflicts = None;
        }
        TaskKind::GitOperation => {
            // Already defaults to GitOperationState::None in WorktreeData
        }
        TaskKind::UserMarker => {
            // Already defaults to None
            status_contexts[idx].user_marker = None;
        }
        TaskKind::Upstream => {
            items[idx].upstream = Some(UpstreamStatus::default());
        }
        TaskKind::CiStatus => {
            // Leave as None (not fetched) on error. This allows the hint path
            // in mod.rs to run and show "install gh/glab" when CI tools fail.
            // Some(None) means "CI tool ran successfully but found no PR".
        }
        TaskKind::UrlStatus => {
            // URL is set at item creation, only default url_active
            items[idx].url_active = None;
        }
    }
}

/// Drain task results from the channel and apply them to items.
///
/// This is the shared logic between progressive and buffered collection modes.
/// The `on_result` callback is called after each result is processed with the
/// item index and a reference to the updated item, allowing progressive mode
/// to update the live table while buffered mode does nothing.
///
/// Uses a 30-second deadline to prevent infinite hangs if git commands stall.
/// When timeout occurs, returns `DrainOutcome::TimedOut` with diagnostic info.
///
/// Errors are collected in the `errors` vec for display after rendering.
/// Default values are applied for failed tasks so the UI can still render.
///
/// Callers decide how to handle timeout:
/// - `collect()`: Shows user-facing diagnostic (interactive command)
/// - `populate_item()`: Logs silently (used by statusline)
pub(super) fn drain_results(
    rx: chan::Receiver<Result<TaskResult, TaskError>>,
    items: &mut [ListItem],
    errors: &mut Vec<TaskError>,
    expected_results: &ExpectedResults,
    mut on_result: impl FnMut(usize, &mut ListItem, &StatusContext),
) -> DrainOutcome {
    use std::time::{Duration, Instant};

    // Deadline for the entire drain operation (30 seconds should be more than enough)
    let deadline = Instant::now() + Duration::from_secs(30);

    // Track which result kinds we've received per item (for timeout diagnostics)
    let mut received_by_item: Vec<Vec<TaskKind>> = vec![Vec::new(); items.len()];

    // Temporary storage for data needed by status_symbols computation
    let mut status_contexts = vec![StatusContext::default(); items.len()];

    // Process task results as they arrive (with deadline)
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            // Deadline exceeded - build diagnostic info showing MISSING results
            let received_count: usize = received_by_item.iter().map(|v| v.len()).sum();

            // Find items with missing results by comparing received vs expected
            let mut items_with_missing: Vec<MissingResult> = Vec::new();

            for (item_idx, item) in items.iter().enumerate() {
                // Get expected results for this item (populated at spawn time)
                let expected = expected_results.results_for(item_idx);

                // Get received results for this item (empty vec if none received)
                let received = received_by_item[item_idx].as_slice();

                // Find missing results
                let missing_kinds: Vec<TaskKind> = expected
                    .iter()
                    .filter(|kind| !received.contains(kind))
                    .copied()
                    .collect();

                if !missing_kinds.is_empty() {
                    let name = item
                        .branch
                        .clone()
                        .unwrap_or_else(|| item.head[..8.min(item.head.len())].to_string());
                    items_with_missing.push(MissingResult {
                        item_idx,
                        name,
                        missing_kinds,
                    });
                }
            }

            // Sort by item index and limit to first 5
            items_with_missing.sort_by_key(|result| result.item_idx);
            items_with_missing.truncate(5);

            return DrainOutcome::TimedOut {
                received_count,
                items_with_missing,
            };
        }

        let outcome = match rx.recv_timeout(remaining) {
            Ok(outcome) => outcome,
            Err(chan::RecvTimeoutError::Timeout) => continue, // Check deadline in next iteration
            Err(chan::RecvTimeoutError::Disconnected) => break, // All senders dropped - done
        };

        // Handle success or error
        let (item_idx, kind) = match outcome {
            Ok(ref result) => (result.item_idx(), TaskKind::from(result)),
            Err(ref error) => (error.item_idx, error.kind),
        };

        // Track this result for diagnostics (both success and error count as "received")
        received_by_item[item_idx].push(kind);

        // Handle error case: apply defaults and collect error
        if let Err(error) = outcome {
            apply_default(items, &mut status_contexts, &error);
            errors.push(error);
            let item = &mut items[item_idx];
            let status_ctx = &status_contexts[item_idx];
            on_result(item_idx, item, status_ctx);
            continue;
        }

        // Handle success case
        let result = outcome.unwrap();
        let item = &mut items[item_idx];
        let status_ctx = &mut status_contexts[item_idx];

        match result {
            TaskResult::CommitDetails { commit, .. } => {
                item.commit = Some(commit);
            }
            TaskResult::AheadBehind {
                counts, is_orphan, ..
            } => {
                item.counts = Some(counts);
                item.is_orphan = Some(is_orphan);
            }
            TaskResult::CommittedTreesMatch {
                committed_trees_match,
                ..
            } => {
                item.committed_trees_match = Some(committed_trees_match);
            }
            TaskResult::HasFileChanges {
                has_file_changes, ..
            } => {
                item.has_file_changes = Some(has_file_changes);
            }
            TaskResult::WouldMergeAdd {
                would_merge_add, ..
            } => {
                item.would_merge_add = Some(would_merge_add);
            }
            TaskResult::IsAncestor { is_ancestor, .. } => {
                item.is_ancestor = Some(is_ancestor);
            }
            TaskResult::BranchDiff { branch_diff, .. } => {
                item.branch_diff = Some(branch_diff);
            }
            TaskResult::WorkingTreeDiff {
                working_tree_diff,
                working_tree_status,
                has_conflicts,
                ..
            } => {
                if let ItemKind::Worktree(data) = &mut item.kind {
                    data.working_tree_diff = Some(working_tree_diff);
                } else {
                    debug_assert!(false, "WorkingTreeDiff result for non-worktree item");
                }
                // Store for status_symbols computation
                status_ctx.working_tree_status = Some(working_tree_status);
                status_ctx.has_conflicts = has_conflicts;
            }
            TaskResult::MergeTreeConflicts {
                has_merge_tree_conflicts,
                ..
            } => {
                // Store for status_symbols computation
                status_ctx.has_merge_tree_conflicts = has_merge_tree_conflicts;
            }
            TaskResult::WorkingTreeConflicts {
                has_working_tree_conflicts,
                ..
            } => {
                // Store for status_symbols computation (takes precedence over commit check)
                status_ctx.has_working_tree_conflicts = has_working_tree_conflicts;
            }
            TaskResult::GitOperation { git_operation, .. } => {
                if let ItemKind::Worktree(data) = &mut item.kind {
                    data.git_operation = git_operation;
                } else {
                    debug_assert!(false, "GitOperation result for non-worktree item");
                }
            }
            TaskResult::UserMarker { user_marker, .. } => {
                // Store for status_symbols computation
                status_ctx.user_marker = user_marker;
            }
            TaskResult::Upstream { upstream, .. } => {
                item.upstream = Some(upstream);
            }
            TaskResult::CiStatus { pr_status, .. } => {
                // Wrap in Some() to indicate "loaded" (Some(None) = no CI, Some(Some(status)) = has CI)
                item.pr_status = Some(pr_status);
            }
            TaskResult::UrlStatus { url, active, .. } => {
                // Two-phase URL rendering:
                // 1. First result (from spawning code): url=Some, active=None → URL appears in normal styling
                // 2. Second result (from health check): url=None, active=Some → dims if inactive
                // Only update non-None fields to preserve values from earlier results.
                if url.is_some() {
                    item.url = url;
                }
                if active.is_some() {
                    item.url_active = active;
                }
            }
        }

        // Invoke callback (progressive mode re-renders rows, buffered mode does nothing)
        on_result(item_idx, item, status_ctx);
    }

    DrainOutcome::Complete
}
