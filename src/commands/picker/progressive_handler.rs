//! Progressive-rendering glue between `collect::collect` and the skim picker.
//!
//! Each event funnels into three places: skim's item stream (`tx`, alive
//! while updates may arrive so the picker stays non-idle), each item's
//! shared `rendered` mutex (in-place redraws), and `shared_items` used by
//! `PickerCollector` for alt-r.
//!
//! # Why updates poke a render
//!
//! skim 4.x's ratatui backend renders **on demand** — it repaints on a key
//! press, when new items land on the channel, or when its matcher flags
//! `needs_render`. It does *not* unconditionally repaint on a timer (the
//! periodic `Heartbeat` only renders when `needs_render` is already set).
//! The old skim 0.20 tuikit backend *did* repaint every 100ms, which is what
//! made in-place `rendered`-mutex mutations surface for free.
//!
//! So after the one-shot skeleton batch, every later mutation is invisible to
//! skim unless we wake it. `request_render` pushes an `Event::Render` through
//! the sender skim hands us at startup (`render_tx`, filled once the TUI is
//! initialized). Intermediate updates coalesce to [`RENDER_THROTTLE`]; the
//! terminal `on_collect_complete` forces one final unthrottled frame.
//!
//! Preview pre-compute is staged in two tiers:
//! - `on_skeleton` fires the first item's 4 modes + first-item summary,
//!   plus the default-tab mode for items 1..N (so quick j/k navigation
//!   lands on warm content). It also fills the static Summary hint for
//!   every row when summaries are disabled.
//! - `on_collect_complete` fires the secondary modes (Log / BranchDiff /
//!   UpstreamDiff) and summaries for items 1..N once the row pipeline
//!   has torn down. Preview tasks share `COLLECT_POOL` with the row
//!   pipeline. Staging keeps low-priority preview submissions out of
//!   that pool's injector while row tasks are still landing on
//!   workers' local deques during drain.

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use color_print::cformat;
use skim::prelude::*;
use worktrunk::styling::{StyledLine, strip_osc8_hyperlinks};

/// Minimum gap between repaint pokes during collection. Holds the redraw rate
/// at ~60fps so a burst of task results can't flood skim's (effectively
/// unbounded) event channel ahead of the user's key presses. A directly-sent
/// `Event::Render` bypasses skim's own frame-rate cap, so we cap it here.
const RENDER_THROTTLE: Duration = Duration::from_millis(16);

use super::items::{HeaderLoading, HeaderSkimItem, PrStatusSlot, PreviewCache, WorktreeSkimItem};
use super::preview::PreviewMode;
use super::preview_orchestrator::PreviewOrchestrator;
use crate::commands::list::collect::PickerProgressHandler;
use crate::commands::list::model::ListItem;

/// Handler owned by the background collect thread. Implements the
/// `PickerProgressHandler` trait that `collect` drives.
///
/// The `tx` clone lives as long as this handler is referenced — dropping the
/// handler (when the collect thread exits) drops the last sender, which signals
/// EOF to skim's reader. That's the explicit contract: once background work is
/// done, the picker can go idle.
pub(super) struct PickerHandler {
    pub(super) tx: SkimItemSender,
    /// skim's event sender, published by the picker once `Skim::init_tui` has
    /// run. `None` until then — early skeleton sends drive the first paint via
    /// the item channel, so a missed poke before the TUI exists is harmless.
    /// Used to push `Event::Render` when a row mutates in place; see the module
    /// docstring.
    pub(super) render_tx: Arc<OnceLock<tokio::sync::mpsc::Sender<Event>>>,
    /// Last time `request_render` actually poked, for the [`RENDER_THROTTLE`]
    /// coalescing gate.
    pub(super) last_render_poke: Mutex<Instant>,
    /// Mirror of the skim item vec visible to `PickerCollector`. Populated
    /// atomically in `on_skeleton`.
    pub(super) shared_items: Arc<Mutex<Vec<Arc<dyn SkimItem>>>>,
    /// One `Arc<Mutex<String>>` per data row — same Arcs `WorktreeSkimItem`
    /// holds. Set once in `on_skeleton`, read lock-free thereafter.
    pub(super) rendered_slots: OnceLock<Box<[Arc<Mutex<String>>]>>,
    /// One live `pr_status` slot per data row — same `PrStatusSlot` Arcs the
    /// `WorktreeSkimItem`s hold. Set once in `on_skeleton` (primed from the
    /// cache-filled snapshot), then written by `on_update` as the `CiStatus`
    /// task reports, so the `pr` tab reflects the live fetch.
    pub(super) pr_status_slots: OnceLock<Box<[PrStatusSlot]>>,
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
    /// Shared with the header: `Some(true)` while the `--prs` forge call is in
    /// flight, so the header shows a "loading…" marker. The `--prs` thread
    /// clears it when the fetch resolves. `None` on non-`--prs` pickers.
    pub(super) prs_loading: Option<Arc<AtomicBool>>,
}

impl PickerHandler {
    /// Wake skim to repaint after an in-place row mutation. No-op until the
    /// picker has published `render_tx` (TUI initialized). `force` skips the
    /// throttle for the final post-collection frame; otherwise pokes are
    /// coalesced to [`RENDER_THROTTLE`]. Best-effort: a full or closed channel
    /// just drops the poke (the next update, or `on_collect_complete`, catches
    /// the row up).
    fn request_render(&self, force: bool) {
        let Some(tx) = self.render_tx.get() else {
            return;
        };
        if !force {
            let mut last = self.last_render_poke.lock().unwrap();
            if last.elapsed() < RENDER_THROTTLE {
                return;
            }
            *last = Instant::now();
        }
        let _ = tx.try_send(Event::Render);
    }
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
        let mut pr_slots: Vec<PrStatusSlot> = Vec::with_capacity(items.len());
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

        // Parent of the main worktree — stripped from each row's matcher path
        // (below) so the fuzzy matcher indexes only the distinguishing tail
        // (`worktrunk.skim-features`), not the `~/workspace/` prefix every sibling
        // worktree shares. The base comes from `list_worktrees()` — the same
        // source the row paths come from — so it shares their exact
        // canonicalization (the strip can't miss on a `/private/var`-vs-`/var`
        // mismatch) and adds no network or uncached work (the skeleton was just
        // built from this list). `[0]` is the main worktree for normal repos and
        // the first linked worktree for bare ones; either way its parent is the
        // shared sibling parent. Computed once; a worktree outside that parent
        // keeps its full path via the `strip_prefix` fallback.
        let path_base = self
            .orchestrator
            .repo()
            .list_worktrees()
            .ok()
            .and_then(|worktrees| worktrees.first())
            .and_then(|wt| wt.path.parent().map(Path::to_path_buf));

        // Header row — non-selectable via `header_lines(1)` on the options.
        // In `--prs` mode it shows a dim "loading open PRs…" line (in place of
        // the column labels) until the forge call's rows land — wording mirrors
        // the empty-list "No open PRs found".
        let loading = self.prs_loading.as_ref().map(|pending| {
            let noun = super::prs::forge_noun(self.orchestrator.repo());
            HeaderLoading {
                pending: Arc::clone(pending),
                marker_ansi: cformat!("  <dim>loading open {noun}…</>"),
            }
        });
        skim_items.push(Arc::new(HeaderSkimItem {
            display_text: header.plain_text(),
            display_text_with_ansi: header.render(),
            loading,
        }) as Arc<dyn SkimItem>);

        for (item, rendered_line) in items.into_iter().zip(rendered) {
            let branch_name = item.branch_name().to_string();
            let has_upstream = upstream_branches.contains(&branch_name);
            // The *distinct* path (leaf relative to the shared worktree parent),
            // not the absolute path — see `path_base`. This feeds only the
            // matcher's `search_text`; the rendered Path column is a separate
            // field and is untouched.
            let path_str = item
                .worktree_path()
                .map(|p| {
                    let p = p.as_path();
                    path_base
                        .as_deref()
                        .and_then(|base| p.strip_prefix(base).ok())
                        .unwrap_or(p)
                        .to_string_lossy()
                        .into_owned()
                })
                .unwrap_or_default();
            // `search_text` is what the matcher sees — fuzzy ranks stay
            // stable across progressive updates because this field only
            // depends on fast data (branch + path + gutter glyph). The
            // trailing gutter glyph lets typing a sigil filter by row kind:
            // `+` to linked worktrees, `@` to the current one, `/`/`|` to
            // local/remote branches (`gutter_glyph` is the same skeleton-time
            // fact the rendered Gutter column uses).
            let gutter = item.kind.gutter_glyph();
            let search_text = if path_str.is_empty() {
                format!("{branch_name} {gutter}")
            } else {
                format!("{branch_name} {path_str} {gutter}")
            };

            // Strip OSC 8 hyperlinks — skim's pipeline mangles them into
            // garbage like `^[8;;…`. Colors (SGR codes) are preserved.
            let rendered_arc = Arc::new(Mutex::new(strip_osc8_hyperlinks(&rendered_line)));
            slots.push(Arc::clone(&rendered_arc));

            let item_arc = Arc::new(item);
            list_items.push(Arc::clone(&item_arc));

            // Prime the live slot from the (cache-filled) snapshot so the `pr`
            // tab paints instantly; `on_update` overwrites it as the CiStatus
            // task reports.
            let pr_status_arc: PrStatusSlot = Arc::new(Mutex::new(item_arc.pr_status.clone()));
            pr_slots.push(Arc::clone(&pr_status_arc));

            skim_items.push(Arc::new(WorktreeSkimItem {
                search_text,
                rendered: rendered_arc,
                branch_name,
                item: item_arc,
                preview_cache: Arc::clone(&self.preview_cache),
                has_upstream,
                summaries_enabled,
                pr_status: pr_status_arc,
            }) as Arc<dyn SkimItem>);
        }

        // Publish slots + skim items before sending to skim so alt-r reload
        // (which reads `shared_items`) sees a populated list the moment
        // skim calls `CommandCollector::invoke`.
        let _ = self.rendered_slots.set(slots.into_boxed_slice());
        let _ = self.pr_status_slots.set(pr_slots.into_boxed_slice());
        *self.shared_items.lock().unwrap() = skim_items.clone();

        // skim 4.x's item channel carries Vec batches; the skeleton is a single
        // batch. This append wakes skim's reader (`items_available`) and drives
        // the first paint. Later field updates happen in place through each
        // item's shared `rendered` mutex and are *not* resent — they surface via
        // `request_render` (see module docstring), since skim won't repaint a
        // silent in-place mutation on its own.
        let _ = self.tx.send(skim_items);

        // Tier 1: warm the user's landing row (all modes) and every
        // other row's default tab. Tier 2 (secondary modes + summaries
        // for items 1..N) fires from `on_collect_complete` after the row
        // pipeline tears down — spawning that bulk now would queue ahead
        // of row tasks in `COLLECT_POOL`'s injector while workers are still
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

    fn on_update(&self, idx: usize, rendered: String, item: &ListItem) {
        if let Some(slots) = self.rendered_slots.get()
            && let Some(slot) = slots.get(idx)
        {
            *slot.lock().unwrap() = strip_osc8_hyperlinks(&rendered);
        }
        // Mirror the row's current CI status into its live slot so the `pr`
        // tab reflects the fetch as it lands. Cheap clone; `pr_status` is
        // `None` until the CiStatus task reports, then `Some(..)`.
        if let Some(slots) = self.pr_status_slots.get()
            && let Some(slot) = slots.get(idx)
        {
            *slot.lock().unwrap() = item.pr_status.clone();
            // Drop the memoized `pr` pane for this row so the next `preview()`
            // re-renders from the status just mirrored — see
            // `WorktreeSkimItem::render_pr_pane_cached`.
            self.preview_cache
                .remove(&(item.branch_name().to_string(), PreviewMode::Pr));
        }
        self.request_render(false);
    }

    fn on_reveal(&self, rendered: Vec<String>) {
        let Some(slots) = self.rendered_slots.get() else {
            return;
        };
        for (slot, line) in slots.iter().zip(rendered) {
            *slot.lock().unwrap() = strip_osc8_hyperlinks(&line);
        }
        self.request_render(false);
    }

    fn stash_warning(&self, line: String) {
        self.stashed_warnings.lock().unwrap().push(line);
    }

    fn on_collect_complete(&self) {
        // Collection is done — `on_update` may have throttled away the last
        // row's repaint, so force one final frame that reflects every slot.
        self.request_render(true);

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

    fn make_handler() -> (PickerHandler, TestRepo, SkimItemReceiver) {
        let test = TestRepo::with_initial_commit();
        let (tx, rx): (SkimItemSender, SkimItemReceiver) = unbounded();
        let shared_items = Arc::new(Mutex::new(Vec::new()));
        let orchestrator = Arc::new(PreviewOrchestrator::new(test.repo.clone()));
        let preview_cache: PreviewCache = Arc::clone(&orchestrator.cache);
        let handler = PickerHandler {
            tx,
            render_tx: Arc::new(OnceLock::new()),
            last_render_poke: Mutex::new(Instant::now()),
            shared_items,
            rendered_slots: OnceLock::new(),
            pr_status_slots: OnceLock::new(),
            preview_cache,
            orchestrator,
            preview_dims: (80, 24),
            llm_command: None,
            summary_hint: Some("disabled".to_string()),
            stashed_warnings: Arc::new(Mutex::new(Vec::new())),
            deferred_items: OnceLock::new(),
            grid_slot: Arc::new(super::super::prs::GridSlot::new()),
            prs_loading: None,
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
    /// reads these strings each time it repaints (which `request_render`
    /// triggers); the matcher-stable search text (branch + path) never changes.
    #[test]
    fn handler_updates_render_strings_in_place() {
        let (handler, _test, rx) = make_handler();
        let items = vec![
            ListItem::new_branch("aaa".into(), "one".into()),
            ListItem::new_branch("bbb".into(), "two".into()),
        ];
        let rendered = vec!["skel-one".to_string(), "skel-two".to_string()];

        handler.on_skeleton(items, rendered, header("hdr"), grid());

        // Header + 2 items sent to skim as one batch.
        let received = rx.recv().expect("skeleton batch");
        assert_eq!(received.len(), 3, "expected header + 2 items");

        let slots = handler.rendered_slots.get().unwrap();
        assert_eq!(slots.len(), 2);
        assert_eq!(*slots[0].lock().unwrap(), "skel-one");
        assert_eq!(*slots[1].lock().unwrap(), "skel-two");

        // pr_status slots start primed from the (empty) snapshots.
        let pr_slots = handler.pr_status_slots.get().unwrap();
        assert!(pr_slots[0].lock().unwrap().is_none());
        assert!(pr_slots[1].lock().unwrap().is_none());

        // on_update rewrites a single render slot (the second item here) and
        // mirrors that item's CI status into its pr_status slot.
        let mut updated = ListItem::new_branch("bbb".into(), "two".into());
        updated.pr_status = Some(None);
        handler.on_update(1, "updated-two".into(), &updated);
        assert_eq!(*slots[0].lock().unwrap(), "skel-one", "row 0 untouched");
        assert_eq!(*slots[1].lock().unwrap(), "updated-two");
        assert!(
            matches!(&*pr_slots[1].lock().unwrap(), Some(None)),
            "row 1 pr status mirrored from the updated item"
        );
        assert!(
            pr_slots[0].lock().unwrap().is_none(),
            "row 0 pr status untouched"
        );

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

        let received = rx.recv().expect("skeleton batch");
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

    /// `on_skeleton` folds each row's gutter glyph onto the end of the
    /// matcher's `search_text`, so typing a sigil filters by row kind. The
    /// glyph comes from `ItemKind::gutter_glyph` (which handles every kind and
    /// is unit-tested in `item.rs`), so branch rows here prove the wiring.
    #[test]
    fn search_text_folds_in_gutter_glyph() {
        let (handler, _test, rx) = make_handler();
        let items = vec![
            ListItem::new_branch("abc".into(), "localbr".into()),
            ListItem::new_remote_branch("abc".into(), "origin/remotebr".into()),
        ];
        handler.on_skeleton(
            items,
            vec!["skel-local".into(), "skel-remote".into()],
            header("hdr"),
            grid(),
        );

        let received = rx.recv().expect("skeleton batch");
        // received[0] is the non-selectable header; data rows follow in order.
        let text = |i: usize| received[i].text().into_owned();
        assert!(text(1).ends_with('/'), "local branch: {:?}", text(1));
        assert!(text(2).ends_with('|'), "remote branch: {:?}", text(2));
        // Branch name stays searchable alongside the folded-in glyph.
        assert!(text(1).contains("localbr"));
        assert!(text(2).contains("origin/remotebr"));
    }

    /// `on_skeleton` strips the shared main-worktree parent from each row's
    /// matcher `search_text`, so the fuzzy matcher indexes the distinguishing
    /// leaf (`repo.feat`) rather than the absolute prefix every sibling worktree
    /// carries. The inside row's path is read back from `worktree_for_branch`, so
    /// the strip runs against the real `list_worktrees()` path the rows carry, not
    /// `add_worktree`'s separately canonicalized return. A worktree outside the
    /// shared parent keeps its full path via the `strip_prefix` fallback.
    #[test]
    fn search_text_strips_shared_worktree_parent() {
        use crate::commands::list::model::{ItemKind, WorktreeData};

        let (handler, mut test, rx) = make_handler();
        // A genuine sibling worktree at `{temp}/repo.feat`; read its path back from
        // the worktree list (the production path source — both the row path and
        // the stripped base come from `list_worktrees()`, so they share one
        // canonicalization).
        test.add_worktree("feat");
        let inside_path = test
            .repo
            .worktree_for_branch("feat")
            .unwrap()
            .expect("feat worktree is registered");
        let outside_path = std::path::PathBuf::from("/nonexistent-root/external-wt");

        let worktree_item = |branch: &str, path: &Path| {
            let mut item = ListItem::new_branch("abc".into(), branch.into());
            item.kind = ItemKind::Worktree(Box::new(WorktreeData {
                path: path.to_path_buf(),
                ..Default::default()
            }));
            item
        };

        handler.on_skeleton(
            vec![
                worktree_item("inside", &inside_path),
                worktree_item("outside", &outside_path),
            ],
            vec!["skel-inside".into(), "skel-outside".into()],
            header("hdr"),
            grid(),
        );

        let received = rx.recv().expect("skeleton batch");
        let text = |i: usize| received[i].text().into_owned();
        let (inside, outside) = (text(1), text(2));

        // Inside the shared parent: only the leaf is indexed (gutter `+` for a
        // non-current linked worktree). The absolute prefix is gone.
        assert_eq!(inside, "inside repo.feat +", "leaf only: {inside:?}");

        // Outside the shared parent: the full path is retained (fallback).
        assert_eq!(
            outside, "outside /nonexistent-root/external-wt +",
            "out-of-tree path kept whole: {outside:?}"
        );
    }

    /// Locks the gutter-sigil filtering contract against skim's default engine —
    /// an `AndOrEngineFactory` wrapping an `ExactOrFuzzyEngineFactory`, the
    /// factory `Matcher::create_engine_factory` builds when the picker sets
    /// neither `--exact` nor `--regex` (skim 4.8 `src/matcher.rs`). `+`/`@` are
    /// literals that filter to their row kind; `^` and `|` collide with skim's
    /// prefix-anchor and OR operators and don't.
    #[test]
    fn gutter_glyphs_filter_under_skims_default_engine() {
        struct TextItem(String);
        impl SkimItem for TextItem {
            fn text(&self) -> std::borrow::Cow<'_, str> {
                std::borrow::Cow::Borrowed(&self.0)
            }
        }

        // One representative folded `search_text` per gutter kind.
        let current = "cur /tmp/cur @";
        let primary = "primary /tmp/primary ^";
        let linked = "linked /tmp/linked +";
        let local = "localbr /";
        let remote = "origin/remotebr |";

        let matches = |query: &str, haystack: &str| -> bool {
            let factory = AndOrEngineFactory::new(ExactOrFuzzyEngineFactory::builder().build());
            factory
                .create_engine(query)
                .match_item(&TextItem(haystack.to_string()))
                .is_some()
        };

        // `+`/`@` are literals — each filters to exactly its row kind.
        assert!(matches("+", linked), "+ selects the linked worktree");
        for other in [current, primary, local, remote] {
            assert!(!matches("+", other), "+ must not match {other:?}");
        }
        assert!(matches("@", current), "@ selects the current worktree");
        for other in [primary, linked, local, remote] {
            assert!(!matches("@", other), "@ must not match {other:?}");
        }

        // `^` is skim's prefix anchor: a bare `^` is an empty prefix pattern
        // that matches every row, so it can't isolate the primary worktree.
        // Proven by matching rows that contain no literal `^`.
        for row in [current, linked, local, remote] {
            assert!(
                matches("^", row),
                "^ (prefix anchor) matches all rows — not a filter: {row:?}"
            );
        }
        // A bare `|` is skim's OR separator with empty operands, which matches
        // nothing — so it can't isolate remote-branch rows (which carry `|`).
        for row in [remote, current, linked, local] {
            assert!(
                !matches("|", row),
                "| (OR operator) matches nothing — not a filter: {row:?}"
            );
        }
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
