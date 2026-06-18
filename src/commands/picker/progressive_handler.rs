//! Progressive-rendering glue between `collect::collect` and the skim picker.
//!
//! Each event funnels into three places: skim's item stream (`tx`, alive
//! while updates may arrive so the 100ms heartbeat keeps firing), each
//! item's shared `rendered` mutex (in-place redraws picked up by the
//! heartbeat), and `shared_items` used by `PickerCollector` for alt-r.
//!
//! Preview pre-compute is staged in two tiers:
//! - `on_skeleton` fires the first item's 4 modes + first-item summary,
//!   plus the default-tab mode for items 1..N (so quick j/k navigation
//!   lands on warm content). It also fills the static Summary hint for
//!   every row when summaries are disabled.
//! - `on_collect_complete` fires the secondary modes (Log / BranchDiff /
//!   UpstreamDiff) and summaries for items 1..N once the row pipeline
//!   has torn down. Preview tasks share the global rayon pool with the
//!   row pipeline; staging keeps low-priority preview submissions out
//!   of the global injector while row tasks are still landing on
//!   workers' local deques during drain.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};

use skim::prelude::*;
use worktrunk::styling::{StyledLine, strip_osc8_hyperlinks};

use super::items::{HeaderSkimItem, PreviewCache, WorktreeSkimItem};
use super::preview_orchestrator::PreviewOrchestrator;
use crate::commands::list::collect::PickerProgressHandler;
use crate::commands::list::model::ListItem;

/// Handler owned by the background collect thread. Implements the
/// `PickerProgressHandler` trait that `collect` drives.
///
/// The `tx` clone lives as long as this handler is referenced — dropping the
/// handler (when the collect thread exits) drops the last sender, which
/// stops skim's heartbeat. That's the explicit contract: once background
/// work is done, the picker can go idle.
pub(super) struct PickerHandler {
    pub(super) tx: SkimItemSender,
    /// Mirror of the skim item vec visible to `PickerCollector`. Populated
    /// atomically in `on_skeleton`.
    pub(super) shared_items: Arc<Mutex<Vec<Arc<dyn SkimItem>>>>,
    /// One `Arc<Mutex<String>>` per data row — same Arcs `WorktreeSkimItem`
    /// holds. Set once in `on_skeleton`, read lock-free thereafter.
    pub(super) rendered_slots: OnceLock<Box<[Arc<Mutex<String>>]>>,
    pub(super) preview_cache: PreviewCache,
    pub(super) orchestrator: Arc<PreviewOrchestrator>,
    pub(super) preview_dims: (usize, usize),
    pub(super) llm_command: Option<String>,
    /// Filled into the Summary preview cache for every item when summaries
    /// are disabled — gives the Summary tab something useful instead of a
    /// perpetual "Generating…" placeholder.
    pub(super) summary_hint: Option<String>,
    /// Pre-formatted warning lines stashed by `collect::collect` while skim
    /// owns the terminal. The picker drains and emits these to stderr after
    /// `Skim::run_with` returns. Lines are kept in arrival order.
    pub(super) stashed_warnings: Arc<Mutex<Vec<String>>>,
    /// Items captured at `on_skeleton` and consumed at `on_collect_complete`
    /// to fan out the bulk preview pre-compute for items 1..N. Set once
    /// (`OnceLock`) because skeletons fire exactly once per collect.
    pub(super) deferred_items: OnceLock<Vec<Arc<ListItem>>>,
    /// Handoff of the layout's column geometry to the `--prs` thread, which
    /// renders PR rows on the same grid as the worktree rows.
    pub(super) grid_slot: Arc<super::prs::GridSlot>,
}

impl PickerProgressHandler for PickerHandler {
    fn on_skeleton(
        &self,
        items: Vec<ListItem>,
        rendered: Vec<String>,
        header: StyledLine,
        grid: crate::commands::list::layout::ColumnGrid,
    ) {
        debug_assert_eq!(items.len(), rendered.len());
        self.grid_slot.set(grid);

        let mut slots: Vec<Arc<Mutex<String>>> = Vec::with_capacity(items.len());
        let mut skim_items: Vec<Arc<dyn SkimItem>> = Vec::with_capacity(items.len() + 1);
        let mut list_items: Vec<Arc<ListItem>> = Vec::with_capacity(items.len());

        // Synchronous skeleton-time tab-availability facts (see `TabAvailability`).
        // Branches with an upstream tracking ref drive the tab-4 (remote⇅) empty
        // state, read from the pre-skeleton `for-each-ref` inventory — never the
        // async `item.upstream`. A `local_branches()` failure yields the empty set
        // (every branch reads as no-upstream); preview rendering must not error.
        let upstream_branches: HashSet<String> = self
            .orchestrator
            .repo()
            .local_branches()
            .map(|branches| {
                branches
                    .iter()
                    .filter(|b| b.upstream_short.is_some())
                    .map(|b| b.name.clone())
                    .collect()
            })
            .unwrap_or_default();
        let summaries_enabled = self.llm_command.is_some();

        // Header row — non-selectable via `header_lines(1)` on the options.
        skim_items.push(Arc::new(HeaderSkimItem {
            display_text: header.plain_text(),
            display_text_with_ansi: header.render(),
        }) as Arc<dyn SkimItem>);

        for (item, rendered_line) in items.into_iter().zip(rendered) {
            let branch_name = item.branch_name().to_string();
            let has_upstream = upstream_branches.contains(&branch_name);
            let path_str = item
                .worktree_path()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            // `search_text` is what the matcher sees — fuzzy ranks stay
            // stable across progressive updates because this field only
            // depends on fast data (branch + path).
            let search_text = if path_str.is_empty() {
                branch_name.clone()
            } else {
                format!("{branch_name} {path_str}")
            };

            // Strip OSC 8 hyperlinks — skim's pipeline mangles them into
            // garbage like `^[8;;…`. Colors (SGR codes) are preserved.
            let rendered_arc = Arc::new(Mutex::new(strip_osc8_hyperlinks(&rendered_line)));
            slots.push(Arc::clone(&rendered_arc));

            let item_arc = Arc::new(item);
            list_items.push(Arc::clone(&item_arc));

            skim_items.push(Arc::new(WorktreeSkimItem {
                search_text,
                rendered: rendered_arc,
                branch_name,
                item: item_arc,
                preview_cache: Arc::clone(&self.preview_cache),
                has_upstream,
                summaries_enabled,
            }) as Arc<dyn SkimItem>);
        }

        // Publish slots + skim items before sending to skim so alt-r reload
        // (which reads `shared_items`) sees a populated list the moment
        // skim calls `CommandCollector::invoke`.
        let _ = self.rendered_slots.set(slots.into_boxed_slice());
        *self.shared_items.lock().unwrap() = skim_items.clone();

        for skim_item in &skim_items {
            let _ = self.tx.send(Arc::clone(skim_item));
        }

        // Tier 1: warm the user's landing row (all modes) and every
        // other row's default tab. Tier 2 (secondary modes + summaries
        // for items 1..N) fires from `on_collect_complete` after the row
        // pipeline tears down — spawning that bulk now would queue ahead
        // of row tasks in the global injector while workers are still
        // grinding through the row work.
        self.orchestrator.spawn_initial_precompute(
            &list_items,
            self.preview_dims,
            self.llm_command.as_deref(),
        );
        // Static Summary hint is a synchronous in-memory insert, no
        // contention concern. Pre-fill every row at skeleton time so the
        // Summary tab is usable for any selection immediately.
        if self.llm_command.is_none()
            && let Some(hint) = self.summary_hint.as_deref()
        {
            self.orchestrator.seed_summary_hints(&list_items, hint);
        }
        let _ = self.deferred_items.set(list_items);
    }

    fn on_update(&self, idx: usize, rendered: String) {
        if let Some(slots) = self.rendered_slots.get()
            && let Some(slot) = slots.get(idx)
        {
            *slot.lock().unwrap() = strip_osc8_hyperlinks(&rendered);
        }
    }

    fn on_reveal(&self, rendered: Vec<String>) {
        let Some(slots) = self.rendered_slots.get() else {
            return;
        };
        for (slot, line) in slots.iter().zip(rendered) {
            *slot.lock().unwrap() = strip_osc8_hyperlinks(&line);
        }
    }

    fn stash_warning(&self, line: String) {
        self.stashed_warnings.lock().unwrap().push(line);
    }

    fn on_collect_complete(&self) {
        let Some(items) = self.deferred_items.get() else {
            return;
        };
        if items.len() <= 1 {
            return;
        }
        self.orchestrator.spawn_deferred_precompute(
            &items[1..],
            self.preview_dims,
            self.llm_command.as_deref(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::list::model::ListItem;
    use worktrunk::testing::TestRepo;

    fn make_handler() -> (
        PickerHandler,
        TestRepo,
        crossbeam_channel::Receiver<Arc<dyn SkimItem>>,
    ) {
        let test = TestRepo::with_initial_commit();
        let (tx, rx) = crossbeam_channel::unbounded::<Arc<dyn SkimItem>>();
        let shared_items = Arc::new(Mutex::new(Vec::new()));
        let orchestrator = Arc::new(PreviewOrchestrator::new(test.repo.clone()));
        let preview_cache: PreviewCache = Arc::clone(&orchestrator.cache);
        let handler = PickerHandler {
            tx,
            shared_items,
            rendered_slots: OnceLock::new(),
            preview_cache,
            orchestrator,
            preview_dims: (80, 24),
            llm_command: None,
            summary_hint: Some("disabled".to_string()),
            stashed_warnings: Arc::new(Mutex::new(Vec::new())),
            deferred_items: OnceLock::new(),
            grid_slot: Arc::new(super::super::prs::GridSlot::new()),
        };
        (handler, test, rx)
    }

    fn header(text: &str) -> StyledLine {
        let mut line = StyledLine::new();
        line.push_raw(text);
        line
    }

    fn grid() -> crate::commands::list::layout::ColumnGrid {
        crate::commands::list::layout::ColumnGrid::default()
    }

    /// Skeleton → update → reveal: verifies that each event writes through
    /// to the shared `rendered` string the `WorktreeSkimItem` holds. Skim
    /// reads these strings on its heartbeat; the matcher-stable search
    /// text (branch + path) never changes.
    #[test]
    fn handler_updates_render_strings_in_place() {
        let (handler, _test, rx) = make_handler();
        let items = vec![
            ListItem::new_branch("aaa".into(), "one".into()),
            ListItem::new_branch("bbb".into(), "two".into()),
        ];
        let rendered = vec!["skel-one".to_string(), "skel-two".to_string()];

        handler.on_skeleton(items, rendered, header("hdr"), grid());

        // Header + 2 items sent to skim.
        let received: Vec<Arc<dyn SkimItem>> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert_eq!(received.len(), 3, "expected header + 2 items");

        let slots = handler.rendered_slots.get().unwrap();
        assert_eq!(slots.len(), 2);
        assert_eq!(*slots[0].lock().unwrap(), "skel-one");
        assert_eq!(*slots[1].lock().unwrap(), "skel-two");

        // on_update rewrites a single slot (the second item here).
        handler.on_update(1, "updated-two".into());
        assert_eq!(*slots[0].lock().unwrap(), "skel-one", "row 0 untouched");
        assert_eq!(*slots[1].lock().unwrap(), "updated-two");

        // on_reveal rewrites every slot — slot writes are idempotent
        // through `Mutex<String>`, so unconditional updates are safe.
        handler.on_reveal(vec!["rev-one".into(), "rev-two".into()]);
        assert_eq!(*slots[0].lock().unwrap(), "rev-one");
        assert_eq!(*slots[1].lock().unwrap(), "rev-two");
    }

    /// Header + items get published in order. `output()` of the
    /// WorktreeSkimItem is the branch name so skim returns the correct
    /// identifier when the user hits Enter.
    #[test]
    fn skeleton_publishes_header_then_items() {
        let (handler, _test, rx) = make_handler();
        let items = vec![
            ListItem::new_branch("aaa".into(), "feat-a".into()),
            ListItem::new_branch("bbb".into(), "feat-b".into()),
        ];

        handler.on_skeleton(
            items,
            vec!["skel-a".into(), "skel-b".into()],
            header("Branch Status"),
            grid(),
        );

        let received: Vec<Arc<dyn SkimItem>> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert_eq!(received.len(), 3);
        // Header emits empty output (not selectable).
        assert_eq!(received[0].output().as_ref(), "");
        // Branch items emit the branch name.
        assert_eq!(received[1].output().as_ref(), "feat-a");
        assert_eq!(received[2].output().as_ref(), "feat-b");

        // Shared state matches what was sent.
        let shared = handler.shared_items.lock().unwrap();
        assert_eq!(shared.len(), 3);
        assert_eq!(shared[1].output().as_ref(), "feat-a");
        assert_eq!(shared[2].output().as_ref(), "feat-b");
    }

    /// `stash_warning` accumulates lines in arrival order so the picker can
    /// drain them in one shot after skim releases the terminal.
    #[test]
    fn stash_warning_preserves_order() {
        let (handler, _test, _rx) = make_handler();
        handler.stash_warning("first".into());
        handler.stash_warning("second".into());
        handler.stash_warning("third".into());
        let stash = handler.stashed_warnings.lock().unwrap();
        assert_eq!(stash.as_slice(), &["first", "second", "third"]);
    }

    /// Preview pre-compute is tiered. After `on_skeleton`:
    /// - First item gets all 4 modes (the user's landing row).
    /// - Items 1..N get only `WorkingTree` (the picker's initial tab) so
    ///   quick j/k navigation hits warm content.
    /// - Secondary modes for items 1..N are deferred until
    ///   `on_collect_complete` fires.
    ///
    /// Summary hint is filled for every item synchronously at skeleton
    /// time so the Summary tab is usable for any selection immediately.
    #[test]
    fn precompute_staging_tiers_match_design() {
        use super::super::preview::PreviewMode;

        let (handler, _test, _rx) = make_handler();
        let items = vec![
            ListItem::new_branch("aaa".into(), "alpha".into()),
            ListItem::new_branch("bbb".into(), "beta".into()),
            ListItem::new_branch("ccc".into(), "gamma".into()),
        ];
        let rendered = vec!["s1".into(), "s2".into(), "s3".into()];

        handler.on_skeleton(items, rendered, header("hdr"), grid());
        handler.orchestrator.wait_for_idle();

        // Static Summary hint primed for every item at skeleton time.
        for branch in ["alpha", "beta", "gamma"] {
            assert!(
                handler
                    .preview_cache
                    .contains_key(&(branch.into(), PreviewMode::Summary)),
                "Summary hint should be filled for {branch} at skeleton time"
            );
        }

        // First item: all 4 modes spawned at skeleton time.
        for mode in [
            PreviewMode::WorkingTree,
            PreviewMode::Log,
            PreviewMode::BranchDiff,
            PreviewMode::UpstreamDiff,
        ] {
            assert!(
                handler.preview_cache.contains_key(&("alpha".into(), mode)),
                "first item should have {mode:?} cached after on_skeleton"
            );
        }

        // Items 1..N: WorkingTree (default tab) cached at skeleton time.
        for branch in ["beta", "gamma"] {
            assert!(
                handler
                    .preview_cache
                    .contains_key(&(branch.into(), PreviewMode::WorkingTree)),
                "{branch}.WorkingTree should be cached after on_skeleton (initial tier)"
            );
        }

        // Items 1..N: secondary modes NOT yet spawned (deferred tier).
        for branch in ["beta", "gamma"] {
            for mode in [
                PreviewMode::Log,
                PreviewMode::BranchDiff,
                PreviewMode::UpstreamDiff,
            ] {
                assert!(
                    !handler.preview_cache.contains_key(&(branch.into(), mode)),
                    "{branch}.{mode:?} should NOT be cached before on_collect_complete"
                );
            }
        }

        handler.on_collect_complete();
        handler.orchestrator.wait_for_idle();

        // After on_collect_complete, every item × every preview mode is cached.
        for branch in ["alpha", "beta", "gamma"] {
            for mode in [
                PreviewMode::WorkingTree,
                PreviewMode::Log,
                PreviewMode::BranchDiff,
                PreviewMode::UpstreamDiff,
            ] {
                assert!(
                    handler.preview_cache.contains_key(&(branch.into(), mode)),
                    "{branch}.{mode:?} should be cached after on_collect_complete"
                );
            }
        }
    }

    /// `on_collect_complete` is safe to call when no skeleton ever fired
    /// (e.g. zero-worktree early return in `collect`) and when only one
    /// item exists (nothing to defer). The post-conditions assert that no
    /// extra spawns leak into the cache — a regression that introduces
    /// work for items 1..N from this hook would surface here.
    #[test]
    fn on_collect_complete_is_no_op_when_no_rest_items() {
        use super::super::preview::PreviewMode;

        // Case 1: never called on_skeleton — cache must remain empty.
        let (handler, _test, _rx) = make_handler();
        handler.on_collect_complete();
        handler.orchestrator.wait_for_idle();
        assert_eq!(
            handler.preview_cache.iter().count(),
            0,
            "no work should be spawned when on_skeleton never fired"
        );

        // Case 2: single-item skeleton — first-item phase covered the 4
        // modes plus the static Summary hint (5 entries total). Nothing
        // left to defer; on_collect_complete must not add any entries.
        let (handler, _test, _rx) = make_handler();
        let items = vec![ListItem::new_branch("aaa".into(), "solo".into())];
        handler.on_skeleton(items, vec!["s1".into()], header("hdr"), grid());
        handler.orchestrator.wait_for_idle();
        let before = handler.preview_cache.iter().count();
        for mode in [
            PreviewMode::WorkingTree,
            PreviewMode::Log,
            PreviewMode::BranchDiff,
            PreviewMode::UpstreamDiff,
            PreviewMode::Summary,
        ] {
            assert!(
                handler.preview_cache.contains_key(&("solo".into(), mode)),
                "first-item phase should have cached {mode:?}"
            );
        }
        assert_eq!(before, 5, "first-item phase populates exactly 5 entries");

        handler.on_collect_complete();
        handler.orchestrator.wait_for_idle();
        assert_eq!(
            handler.preview_cache.iter().count(),
            before,
            "on_collect_complete must not spawn additional work for a single-item skeleton"
        );
    }
}
