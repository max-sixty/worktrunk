//! Result processing and draining.
//!
//! The drain consumes task results from the channel and writes them
//! straight onto `ListItem` / `WorktreeData` fields. There is no parallel
//! "status context" structure — status-feeding fields flow through the
//! same `Option<T>` slots as every other column.
//!
//! `ListItem::refresh_status_symbols` is called after every successful
//! result; each gate resolves independently once its inputs arrive.
//! Callers must also call `refresh_status_symbols` post-drain to cover
//! items with zero successful results (all tasks errored or timed out),
//! so that synchronously-derivable gates (e.g. `worktree_state` from
//! metadata) still materialize.

use std::time::{Duration, Instant};

use crossbeam_channel as chan;

/// Deadline for the entire drain operation. Generous to avoid flaky timeouts
/// under CI load where process spawning for ~70 work items can be slow.
pub(super) const DRAIN_TIMEOUT: Duration = Duration::from_secs(120);

use super::super::model::{ItemKind, ListItem};
use super::execution::ExpectedResults;
use super::types::{DrainOutcome, MissingResult, TaskError, TaskKind, TaskResult};

/// Boxed one-shot tick closure. Factored out so the inferred closure can
/// coerce to `dyn FnMut` at the `Box::new` site.
pub(super) type DrainTickFn<'a> = Box<dyn FnMut(&mut [ListItem]) + 'a>;

/// One-shot tick scheduled against [`drain_results`]. When `Instant` elapses,
/// the boxed closure fires once with the full item slice — used by `wt list`
/// to reveal the `·` loading indicator at T+200ms.
pub(super) type DrainTick<'a> = (Instant, DrainTickFn<'a>);

/// Drain task results from the channel and apply them to items.
///
/// This is the shared logic between progressive and buffered collection modes.
/// The `on_result` callback fires after each result so progressive mode can
/// refresh the live table; buffered mode passes a no-op.
///
/// Uses a caller-provided `deadline` to cap wall-clock time. When the deadline
/// is reached, returns `DrainOutcome::TimedOut` with diagnostic info.
///
/// Errors are collected in the `errors` vec for display after rendering. No
/// defaults are applied — an errored field stays `None`, so the renderer
/// shows its standard placeholder and `compute_status_symbols` stays a
/// no-op for that item.
///
/// Callers decide how to handle timeout:
/// - `collect()`: Shows user-facing diagnostic (interactive command)
/// - `populate_item()`: Logs silently (used by statusline)
#[allow(clippy::too_many_arguments)]
pub(super) fn drain_results(
    rx: chan::Receiver<Result<TaskResult, TaskError>>,
    items: &mut [ListItem],
    errors: &mut Vec<TaskError>,
    expected_results: &ExpectedResults,
    deadline: Instant,
    integration_target: Option<&str>,
    mut on_result: impl FnMut(usize, &mut ListItem),
    mut tick: Option<DrainTick<'_>>,
) -> DrainOutcome {
    // Track which result kinds we've received per item (for timeout diagnostics)
    let mut received_by_item: Vec<Vec<TaskKind>> = vec![Vec::new(); items.len()];

    // Process task results as they arrive (with deadline)
    loop {
        // Fire the one-shot tick when its deadline has passed. The tick fires
        // between channel recvs, so it never races with `on_result`.
        if let Some((tick_at, _)) = tick.as_ref()
            && Instant::now() >= *tick_at
        {
            let (_, mut tick_fn) = tick.take().unwrap();
            tick_fn(items);
        }

        let now = Instant::now();
        let remaining = deadline.saturating_duration_since(now);
        // Clamp recv timeout so we wake at the tick deadline (earliest of the two).
        let recv_timeout_dur = match tick.as_ref() {
            Some((tick_at, _)) => remaining.min(tick_at.saturating_duration_since(now)),
            None => remaining,
        };
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

            // Sort by item index. The display site in `collect()` truncates
            // to the first 5 when rendering the warning.
            items_with_missing.sort_by_key(|result| result.item_idx);

            return DrainOutcome::TimedOut {
                received_count,
                items_with_missing,
            };
        }

        let outcome = match rx.recv_timeout(recv_timeout_dur) {
            Ok(outcome) => outcome,
            Err(chan::RecvTimeoutError::Timeout) => continue, // Check tick/deadline next iteration
            Err(chan::RecvTimeoutError::Disconnected) => break, // All senders dropped - done
        };

        // Handle success or error
        let (item_idx, kind) = match outcome {
            Ok(ref result) => (result.item_idx(), TaskKind::from(result)),
            Err(ref error) => (error.item_idx, error.kind),
        };

        // Track this result for diagnostics (both success and error count as "received")
        received_by_item[item_idx].push(kind);

        // Errors leave the errored task's fields at `None`. The
        // corresponding gate stays unresolved (renders `·`). Callers
        // must call `refresh_status_symbols` post-drain to cover items
        // with zero successful results. Still run the callback so the
        // footer progress counter advances.
        if let Err(error) = outcome {
            errors.push(error);
            on_result(item_idx, &mut items[item_idx]);
            continue;
        }

        // Handle success case
        let result = outcome.unwrap();
        let item = &mut items[item_idx];

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
                would_merge_add,
                is_patch_id_match,
                ..
            } => {
                item.would_merge_add = Some(would_merge_add);
                item.is_patch_id_match = Some(is_patch_id_match);
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
                    data.working_tree_status = Some(working_tree_status);
                    data.has_conflicts = Some(has_conflicts);
                } else {
                    debug_assert!(false, "WorkingTreeDiff result for non-worktree item");
                }
            }
            TaskResult::MergeTreeConflicts {
                has_merge_tree_conflicts,
                ..
            } => {
                item.has_merge_tree_conflicts = Some(has_merge_tree_conflicts);
            }
            TaskResult::WorkingTreeConflicts {
                has_working_tree_conflicts,
                ..
            } => {
                if let ItemKind::Worktree(data) = &mut item.kind {
                    data.has_working_tree_conflicts = Some(has_working_tree_conflicts);
                } else {
                    debug_assert!(false, "WorkingTreeConflicts result for non-worktree item");
                }
            }
            TaskResult::GitOperation { git_operation, .. } => {
                if let ItemKind::Worktree(data) = &mut item.kind {
                    data.git_operation = Some(git_operation);
                } else {
                    debug_assert!(false, "GitOperation result for non-worktree item");
                }
            }
            TaskResult::UserMarker { user_marker, .. } => {
                item.user_marker = Some(user_marker);
            }
            TaskResult::Upstream { upstream, .. } => {
                item.upstream = Some(upstream);
            }
            TaskResult::CiStatus { pr_status, .. } => {
                // Wrap in Some() to indicate "loaded" (Some(None) = no CI, Some(Some(status)) = has CI)
                item.pr_status = Some(pr_status);
            }
            TaskResult::UrlStatus { url, active, .. } => {
                // The synchronous URL write in `work_items_for_worktree`
                // already set `item.url`; this result only carries the
                // health-check outcome. Still guard for older call sites
                // that may send `url=Some`.
                if url.is_some() {
                    item.url = url;
                }
                if active.is_some() {
                    item.url_active = active;
                }
            }
            TaskResult::SummaryGenerate { summary, .. } => {
                item.summary = Some(summary);
            }
        }

        // Refresh status symbols. Each gate resolves independently once
        // its inputs arrive; already-resolved gates are skipped.
        item.refresh_status_symbols(integration_target);

        // Invoke callback (progressive mode re-renders rows, buffered mode does nothing)
        on_result(item_idx, item);
    }

    DrainOutcome::Complete
}

#[cfg(test)]
mod tests {
    use super::super::super::model::{
        AheadBehind, MainState, UpstreamStatus, WorkingTreeStatus, WorktreeState,
    };
    use super::super::execution::seed_skipped_task_defaults;
    use super::super::types::ErrorCause;
    use super::*;
    use worktrunk::git::LineDiff;

    /// Seed every status-feeding field on an item so `refresh_status_symbols`
    /// has no remaining `None` inputs. The values are deliberately
    /// non-default — counts of (3, 5) means a fully-computed StatusSymbols
    /// will report `MainState::Diverged`, which is distinguishable from the
    /// metadata-only fallback's `MainState::None`. Tests use that to assert
    /// "the full computation ran" vs "only the metadata fallback ran."
    fn seed_all_fields(item: &mut ListItem) {
        for kind in [
            TaskKind::UserMarker,
            TaskKind::MergeTreeConflicts,
            TaskKind::IsAncestor,
            TaskKind::CommittedTreesMatch,
            TaskKind::HasFileChanges,
            TaskKind::WouldMergeAdd,
            TaskKind::WorkingTreeConflicts,
            TaskKind::GitOperation,
        ] {
            seed_skipped_task_defaults(item, kind);
        }
        // WorkingTreeDiff is intentionally NOT seeded via
        // `seed_skipped_task_defaults` (it would fabricate a clean tree).
        // Simulate a real task result instead.
        if let ItemKind::Worktree(data) = &mut item.kind {
            data.working_tree_diff = Some(LineDiff::default());
            data.working_tree_status = Some(WorkingTreeStatus::default());
            data.has_conflicts = Some(false);
        }
        item.counts = Some(AheadBehind {
            ahead: 3,
            behind: 5,
        });
        item.is_orphan = Some(false);
        item.upstream = Some(UpstreamStatus::default());
    }

    /// True iff `compute_status_symbols` produced its full output (using
    /// the seed values from `seed_all_fields`). False iff status is at
    /// default (nothing computed) or the metadata-only fallback ran.
    fn full_computation_ran(item: &ListItem) -> bool {
        item.status_symbols.main_state == Some(MainState::Diverged)
    }

    fn fully_seeded_branch_item() -> ListItem {
        let mut item = ListItem::new_branch("abc123".into(), "feat".into());
        seed_all_fields(&mut item);
        item
    }

    fn fully_seeded_worktree_item() -> ListItem {
        use crate::commands::list::collect::build_worktree_item;
        use worktrunk::git::WorktreeInfo;
        let wt = WorktreeInfo {
            path: std::path::PathBuf::from("/tmp/wt"),
            head: "abc123".into(),
            branch: Some("feat".into()),
            bare: false,
            detached: false,
            locked: None,
            prunable: None,
        };
        // `is_main: false` so the test exercises the full guard set,
        // not the main-worktree fast path.
        let mut item = build_worktree_item(&wt, false, false, false);
        seed_all_fields(&mut item);
        item
    }

    #[test]
    fn test_drain_results_summary_generate() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut items = vec![ListItem::new_branch("abc123".into(), "feat".into())];
        let mut errors = Vec::new();
        let expected = ExpectedResults::default();

        // Send a SummaryGenerate result
        tx.send(Ok(TaskResult::SummaryGenerate {
            item_idx: 0,
            summary: Some("Add feature".into()),
        }))
        .unwrap();
        drop(tx);

        let outcome = drain_results(
            rx,
            &mut items,
            &mut errors,
            &expected,
            Instant::now() + DRAIN_TIMEOUT,
            None,
            |_, _| {},
            None,
        );
        assert!(matches!(outcome, DrainOutcome::Complete));
        assert_eq!(items[0].summary, Some(Some("Add feature".into())));
    }

    /// Sanity check: a fully-seeded worktree item resolves every gate to
    /// its expected `Some(...)` value after one `refresh_status_symbols`
    /// call. Per-gate loading / short-circuit behavior is exercised by
    /// the dedicated tests in `item.rs`.
    #[test]
    fn test_refresh_fully_seeded_worktree_resolves_all_gates() {
        let mut item = fully_seeded_worktree_item();
        item.refresh_status_symbols(Some("main"));
        let s = &item.status_symbols;
        assert!(s.working_tree.is_some(), "gate 1 (working tree flags)");
        assert!(s.operation_state.is_some(), "gate 2 (operation state)");
        assert!(s.worktree_state.is_some(), "gate 2 (metadata)");
        assert_eq!(
            s.main_state,
            Some(MainState::Diverged),
            "gate 3 — baseline counts (3, 5) produce Diverged"
        );
        assert!(
            s.upstream_divergence.is_some(),
            "gate 4 (upstream divergence)"
        );
        assert!(s.user_marker.is_some(), "gate 5 (user marker)");
    }

    /// Sanity check: a fully-seeded branch item resolves every gate after
    /// one `refresh_status_symbols` call.
    #[test]
    fn test_refresh_fully_seeded_branch_resolves_all_gates() {
        let mut item = fully_seeded_branch_item();
        item.refresh_status_symbols(Some("main"));
        let s = &item.status_symbols;
        assert!(s.working_tree.is_some(), "gate 1 (branches are clean)");
        assert!(s.operation_state.is_some(), "gate 2 (branches have no op)");
        assert_eq!(s.worktree_state, Some(WorktreeState::Branch));
        assert_eq!(
            s.main_state,
            Some(MainState::Diverged),
            "gate 3 — baseline counts (3, 5) produce Diverged"
        );
        assert!(s.upstream_divergence.is_some());
        assert!(s.user_marker.is_some());
    }

    #[test]
    fn test_drain_results_status_stays_none_while_field_pending() {
        // A fully-seeded item whose counts are manually reset to None
        // mirrors the case of an AheadBehind task that has not yet
        // arrived. The drain must not run the full status computation
        // for it (the metadata fallback may still mark the item as a
        // branch — that's fine and expected).
        let mut item = fully_seeded_branch_item();
        item.counts = None;

        let (tx, rx) = crossbeam_channel::unbounded();
        let expected = ExpectedResults::default();
        expected.expect(0, TaskKind::Upstream);

        tx.send(Ok(TaskResult::Upstream {
            item_idx: 0,
            upstream: UpstreamStatus::default(),
        }))
        .unwrap();
        drop(tx);

        let mut errors = Vec::new();
        let outcome = drain_results(
            rx,
            std::slice::from_mut(&mut item),
            &mut errors,
            &expected,
            Instant::now() + DRAIN_TIMEOUT,
            Some("main"),
            |_, _| {},
            None,
        );

        assert!(matches!(outcome, DrainOutcome::Complete));
        assert!(
            !full_computation_ran(&item),
            "full status computation should not run while a required field is pending",
        );
    }

    #[test]
    fn test_drain_results_status_snaps_when_final_field_arrives() {
        // A fully-seeded item whose counts are manually reset to None
        // starts the drain in the "one field pending" state. Delivering
        // the AheadBehind result should let the drain run the full
        // computation.
        let mut item = fully_seeded_branch_item();
        item.counts = None;
        item.is_orphan = None;

        let (tx, rx) = crossbeam_channel::unbounded();
        let expected = ExpectedResults::default();
        expected.expect(0, TaskKind::AheadBehind);

        tx.send(Ok(TaskResult::AheadBehind {
            item_idx: 0,
            counts: AheadBehind {
                ahead: 3,
                behind: 5,
            },
            is_orphan: false,
        }))
        .unwrap();
        drop(tx);

        let mut errors = Vec::new();
        let outcome = drain_results(
            rx,
            std::slice::from_mut(&mut item),
            &mut errors,
            &expected,
            Instant::now() + DRAIN_TIMEOUT,
            Some("main"),
            |_, _| {},
            None,
        );

        assert!(matches!(outcome, DrainOutcome::Complete));
        assert!(
            full_computation_ran(&item),
            "full status computation should run once the final required field arrives",
        );
    }

    #[test]
    fn test_drain_results_status_stays_none_when_feeder_errors() {
        // Errors leave fields untouched. A required field that starts
        // `None` and is errored stays `None`, so the full computation
        // never runs.
        let mut item = fully_seeded_branch_item();
        item.counts = None;
        item.is_orphan = None;

        let (tx, rx) = crossbeam_channel::unbounded();
        let expected = ExpectedResults::default();
        expected.expect(0, TaskKind::AheadBehind);

        tx.send(Err(TaskError::new(
            0,
            TaskKind::AheadBehind,
            "boom",
            ErrorCause::Other,
        )))
        .unwrap();
        drop(tx);

        let mut errors = Vec::new();
        let outcome = drain_results(
            rx,
            std::slice::from_mut(&mut item),
            &mut errors,
            &expected,
            Instant::now() + DRAIN_TIMEOUT,
            Some("main"),
            |_, _| {},
            None,
        );

        assert!(matches!(outcome, DrainOutcome::Complete));
        assert!(
            !full_computation_ran(&item),
            "full status computation should not run when its sole feeder errored",
        );
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn test_drain_results_timeout_returns_missing_diagnostics() {
        let (_tx, rx) = crossbeam_channel::unbounded();
        let mut items = vec![ListItem::new_branch("abc123".into(), "feat".into())];
        let mut errors = Vec::new();

        // Register expected results but don't send any — simulates tasks still running
        let expected = ExpectedResults::default();
        expected.expect(0, TaskKind::CommitDetails);
        expected.expect(0, TaskKind::AheadBehind);

        // Use an already-expired deadline — remaining.is_zero() triggers immediately
        let outcome = drain_results(
            rx,
            &mut items,
            &mut errors,
            &expected,
            Instant::now(),
            None,
            |_, _| {},
            None,
        );

        let DrainOutcome::TimedOut {
            received_count,
            items_with_missing,
        } = outcome
        else {
            panic!("expected TimedOut with immediate deadline");
        };

        assert_eq!(received_count, 0);
        assert_eq!(items_with_missing.len(), 1);
        assert_eq!(items_with_missing[0].name, "feat");
        assert!(
            items_with_missing[0]
                .missing_kinds
                .contains(&TaskKind::CommitDetails)
        );
        assert!(
            items_with_missing[0]
                .missing_kinds
                .contains(&TaskKind::AheadBehind)
        );
    }

    #[test]
    fn test_drain_results_tick_fires_when_deadline_passes() {
        // A tick whose deadline has already elapsed should fire exactly once
        // with a mutable view of the items, then stop (no re-fire on subsequent
        // loop iterations). The drain loop wakes at the tick deadline even
        // without any channel traffic, so this also exercises the
        // recv_timeout-clamping path.
        let (tx, rx) = crossbeam_channel::unbounded::<Result<TaskResult, TaskError>>();
        let mut items = vec![ListItem::new_branch("abc123".into(), "feat".into())];
        let mut errors = Vec::new();
        let expected = ExpectedResults::default();

        // Send one result so the drain exits promptly after the tick.
        tx.send(Ok(TaskResult::SummaryGenerate {
            item_idx: 0,
            summary: None,
        }))
        .unwrap();
        drop(tx);

        let mut tick_fires = 0usize;
        let tick_fn: DrainTickFn<'_> = Box::new(|items: &mut [ListItem]| {
            tick_fires += 1;
            // Mutate an item to prove we got a live mutable reference.
            items[0].branch = Some("touched-by-tick".into());
        });

        let outcome = drain_results(
            rx,
            &mut items,
            &mut errors,
            &expected,
            Instant::now() + DRAIN_TIMEOUT,
            None,
            |_, _| {},
            Some((Instant::now(), tick_fn)),
        );

        assert!(matches!(outcome, DrainOutcome::Complete));
        assert_eq!(tick_fires, 1, "tick should fire exactly once");
        assert_eq!(items[0].branch.as_deref(), Some("touched-by-tick"));
    }
}
