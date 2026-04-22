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

use super::super::model::{ItemKind, ListItem};
use super::execution::ExpectedResults;
use super::types::{DrainOutcome, MissingResult, TaskError, TaskKind, TaskResult};

/// Deadline for the entire drain operation. Generous to avoid flaky timeouts
/// under CI load where process spawning for ~70 work items can be slow.
pub(super) const DRAIN_TIMEOUT: Duration = Duration::from_secs(120);

/// Stall detection timings used by production callers of `drain_results`.
pub(super) const STALL_TIMINGS: StallTimings = StallTimings {
    threshold: Duration::from_secs(5),
    tick: Duration::from_millis(500),
};

/// Timings controlling `DrainEvent::Stall` emission.
///
/// `threshold` is how long the drain waits with no incoming result before
/// emitting a stall event. `tick` is how often the drain wakes up while idle
/// to re-check — lower values surface the hint faster at the cost of a few
/// extra syscalls per second.
#[derive(Debug, Clone, Copy)]
pub(super) struct StallTimings {
    pub threshold: Duration,
    pub tick: Duration,
}

/// Events emitted by `drain_results`. A single callback handles every
/// event kind so the caller can share state (e.g. the progressive table)
/// across them without fighting the borrow checker.
pub(super) enum DrainEvent<'a> {
    /// A task result has been applied to `item` — progressive mode re-renders.
    Result {
        item_idx: usize,
        item: &'a mut ListItem,
    },
    /// The `reveal_at` deadline passed — fires exactly once. `wt list` uses
    /// this to promote blank placeholders to the `·` loading indicator.
    Reveal { items: &'a [ListItem] },
    /// No results for at least `STALL_TIMINGS.threshold`. `pending_count`
    /// is the total number of expected-but-not-yet-received results
    /// (includes both queued and running tasks); `first_kind` / `first_name`
    /// identify one of them deterministically. Fires repeatedly (each
    /// `STALL_TIMINGS.tick`) while stalled.
    Stall {
        pending_count: usize,
        first_kind: TaskKind,
        first_name: &'a str,
    },
}

/// Tally pending results (expected minus received) and pick the first as an
/// exemplar. Iteration order mirrors how work items were registered, giving
/// a deterministic pick when multiple are pending.
fn pick_pending_hint<'a>(
    expected: &ExpectedResults,
    received_by_item: &[Vec<TaskKind>],
    items: &'a [ListItem],
) -> (usize, Option<(TaskKind, &'a str)>) {
    let received_count: usize = received_by_item.iter().map(|v| v.len()).sum();
    let pending_count = expected.count().saturating_sub(received_count);
    let first = received_by_item
        .iter()
        .enumerate()
        .zip(items.iter())
        .find_map(|((item_idx, received), item)| {
            expected
                .results_for(item_idx)
                .into_iter()
                .find(|kind| !received.contains(kind))
                .map(|kind| (kind, item.display_name()))
        });
    (pending_count, first)
}

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
    on_event: impl FnMut(DrainEvent<'_>),
    reveal_at: Option<Instant>,
) -> DrainOutcome {
    drain_results_with_timings(
        rx,
        items,
        errors,
        expected_results,
        deadline,
        integration_target,
        STALL_TIMINGS,
        on_event,
        reveal_at,
    )
}

/// Core drain loop. Stall timings are parameterized so tests can trigger the
/// stall path quickly; production callers use [`drain_results`].
#[allow(clippy::too_many_arguments)]
pub(super) fn drain_results_with_timings(
    rx: chan::Receiver<Result<TaskResult, TaskError>>,
    items: &mut [ListItem],
    errors: &mut Vec<TaskError>,
    expected_results: &ExpectedResults,
    deadline: Instant,
    integration_target: Option<&str>,
    stall_timings: StallTimings,
    mut on_event: impl FnMut(DrainEvent<'_>),
    mut reveal_at: Option<Instant>,
) -> DrainOutcome {
    // Track which result kinds we've received per item (for timeout diagnostics)
    let mut received_by_item: Vec<Vec<TaskKind>> = vec![Vec::new(); items.len()];

    // Last time a result arrived (start with now so stall is measured from
    // the beginning of collection, not from the Unix epoch).
    let mut last_result_time = Instant::now();

    // Process task results as they arrive (with deadline)
    loop {
        // Fire the one-shot reveal when its deadline has passed. Reveal fires
        // between channel recvs, so it never races with `DrainEvent::Result`.
        if let Some(at) = reveal_at
            && Instant::now() >= at
        {
            on_event(DrainEvent::Reveal { items });
            reveal_at = None;
        }

        let now = Instant::now();
        let remaining = deadline.saturating_duration_since(now);
        // Clamp recv timeout to the nearest of: real deadline, stall tick
        // (so we can fire stall events promptly), and the reveal deadline
        // (so we wake to emit it).
        let mut recv_timeout_dur = remaining.min(stall_timings.tick);
        if let Some(at) = reveal_at {
            recv_timeout_dur = recv_timeout_dur.min(at.saturating_duration_since(now));
        }
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
                    items_with_missing.push(MissingResult {
                        item_idx,
                        name: item.display_name().to_string(),
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
            Err(chan::RecvTimeoutError::Timeout) => {
                // Nothing arrived within the tick. If we've been silent for
                // at least `stall_timings.threshold`, emit a stall event
                // with the pending count plus one exemplar. Fires
                // repeatedly while stalled — `update_footer` is idempotent
                // for same content. The loop top handles firing any
                // due one-shot tick and re-checks the real deadline.
                if last_result_time.elapsed() >= stall_timings.threshold {
                    let (pending_count, first) =
                        pick_pending_hint(expected_results, &received_by_item, items);
                    if let Some((first_kind, first_name)) = first {
                        on_event(DrainEvent::Stall {
                            pending_count,
                            first_kind,
                            first_name,
                        });
                    }
                }
                continue;
            }
            Err(chan::RecvTimeoutError::Disconnected) => break, // All senders dropped - done
        };

        // Handle success or error
        let (item_idx, kind) = match outcome {
            Ok(ref result) => (result.item_idx(), TaskKind::from(result)),
            Err(ref error) => (error.item_idx, error.kind),
        };

        // Track this result for diagnostics (both success and error count as "received")
        received_by_item[item_idx].push(kind);
        last_result_time = Instant::now();

        // Errors leave the errored task's fields at `None`. The
        // corresponding gate stays unresolved (renders `·`). Callers
        // must call `refresh_status_symbols` post-drain to cover items
        // with zero successful results. Still run the callback so the
        // footer progress counter advances.
        if let Err(error) = outcome {
            errors.push(error);
            on_event(DrainEvent::Result {
                item_idx,
                item: &mut items[item_idx],
            });
            continue;
        }

        // Handle success case
        let result = outcome.unwrap();
        let item = &mut items[item_idx];

        match result {
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
        on_event(DrainEvent::Result { item_idx, item });
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
            |_| {},
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
            |_| {},
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
            |_| {},
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
            |_| {},
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
        expected.expect(0, TaskKind::Upstream);
        expected.expect(0, TaskKind::AheadBehind);

        // Use an already-expired deadline — remaining.is_zero() triggers immediately
        let outcome = drain_results(
            rx,
            &mut items,
            &mut errors,
            &expected,
            Instant::now(),
            None,
            |_| {},
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
                .contains(&TaskKind::Upstream)
        );
        assert!(
            items_with_missing[0]
                .missing_kinds
                .contains(&TaskKind::AheadBehind)
        );
    }

    #[test]
    fn test_drain_results_fires_stall_when_silent_past_threshold() {
        // No results arrive; the drain should emit a Stall event reporting
        // the pending count plus a deterministic exemplar once
        // `stall_threshold` elapses, and keep firing on each tick until
        // the deadline.
        let (_tx, rx) = crossbeam_channel::unbounded();
        let mut items = vec![
            ListItem::new_branch("abc123".into(), "feat".into()),
            ListItem::new_branch("def456".into(), "other".into()),
        ];
        let mut errors = Vec::new();

        let expected = ExpectedResults::default();
        expected.expect(0, TaskKind::AheadBehind);
        expected.expect(1, TaskKind::Upstream);

        let mut stall_events: Vec<(usize, TaskKind, String)> = Vec::new();
        let outcome = drain_results_with_timings(
            rx,
            &mut items,
            &mut errors,
            &expected,
            Instant::now() + Duration::from_millis(200),
            None,
            StallTimings {
                threshold: Duration::from_millis(20),
                tick: Duration::from_millis(20),
            },
            |event| {
                if let DrainEvent::Stall {
                    pending_count,
                    first_kind,
                    first_name,
                } = event
                {
                    stall_events.push((pending_count, first_kind, first_name.to_string()));
                }
            },
            None,
        );

        assert!(matches!(outcome, DrainOutcome::TimedOut { .. }));
        assert!(
            !stall_events.is_empty(),
            "expected at least one stall event before the deadline"
        );
        let (count, kind, name) = &stall_events[0];
        assert_eq!(*count, 2);
        assert_eq!(*kind, TaskKind::AheadBehind);
        assert_eq!(name, "feat");
    }

    #[test]
    fn test_drain_results_does_not_fire_stall_when_results_flow() {
        // Results arrive faster than the stall threshold, so no Stall event
        // should fire. last_result_time is reset on each received result.
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut items = vec![ListItem::new_branch("abc123".into(), "feat".into())];
        let mut errors = Vec::new();

        let expected = ExpectedResults::default();
        expected.expect(0, TaskKind::Upstream);

        tx.send(Ok(TaskResult::SummaryGenerate {
            item_idx: 0,
            summary: None,
        }))
        .unwrap();
        drop(tx);

        let mut stall_count = 0;
        let outcome = drain_results_with_timings(
            rx,
            &mut items,
            &mut errors,
            &expected,
            Instant::now() + Duration::from_millis(50),
            None,
            StallTimings {
                threshold: Duration::from_secs(10), // far exceeds deadline
                tick: Duration::from_millis(20),
            },
            |event| {
                if matches!(event, DrainEvent::Stall { .. }) {
                    stall_count += 1;
                }
            },
            None,
        );

        assert!(matches!(outcome, DrainOutcome::Complete));
        assert_eq!(stall_count, 0, "stall must not fire under the threshold");
    }

    #[test]
    fn test_drain_results_survives_mid_stall_result() {
        // A result arriving mid-stall must not break the loop: the drain
        // processes the result, keeps emitting stalls, and exits cleanly
        // when tx drops.
        //
        // The scenario is driven causally through `on_event` — the first
        // Stall injects a result, and a Stall observed after the result
        // drops tx to end the drain. No wall-clock sleeps, so the test
        // runs at the speed of the threshold on any hardware.
        //
        // This does NOT verify `last_result_time` resets on receipt —
        // that would need a mock clock.
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut items = vec![ListItem::new_branch("abc123".into(), "feat".into())];
        let mut errors = Vec::new();

        let expected = ExpectedResults::default();
        expected.expect(0, TaskKind::Upstream);
        expected.expect(0, TaskKind::AheadBehind);

        let mut sender = Some(tx);
        let mut saw_result = false;
        let mut stalls_before_result = 0;
        let mut stalls_after_result = 0;

        let outcome = drain_results_with_timings(
            rx,
            &mut items,
            &mut errors,
            &expected,
            Instant::now() + Duration::from_secs(5),
            None,
            StallTimings {
                threshold: Duration::from_millis(20),
                tick: Duration::from_millis(10),
            },
            |event| match event {
                DrainEvent::Stall { .. } => {
                    if saw_result {
                        stalls_after_result += 1;
                        sender.take();
                    } else {
                        stalls_before_result += 1;
                        if let Some(tx) = sender.as_ref() {
                            tx.send(Ok(TaskResult::SummaryGenerate {
                                item_idx: 0,
                                summary: None,
                            }))
                            .unwrap();
                        }
                    }
                }
                DrainEvent::Result { .. } => {
                    saw_result = true;
                }
                _ => {}
            },
            None,
        );

        assert!(matches!(outcome, DrainOutcome::Complete));
        assert!(stalls_before_result >= 1);
        assert!(saw_result, "drain should deliver the injected result");
        assert!(
            stalls_after_result >= 1,
            "drain should keep emitting stalls after a mid-stall result"
        );
    }
}
