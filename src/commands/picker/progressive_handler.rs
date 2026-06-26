//! Progressive-rendering glue between `collect::collect` and the skim picker.
//!
//! Each event funnels into three places: skim's item stream (`tx`, alive
//! while updates may arrive so the picker stays non-idle), each item's
//! shared `rendered` mutex (in-place redraws), and `shared_items` used by
//! `PickerCollector` for alt-x.
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

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use color_print::cformat;
use skim::prelude::*;
use worktrunk::git::Repository;
use worktrunk::styling::{StyledLine, strip_osc8_hyperlinks};

use super::super::list::ci_status::PrStatus;

/// Minimum gap between repaint pokes during collection. Holds the redraw rate
/// at ~60fps so a burst of task results can't flood skim's (effectively
/// unbounded) event channel ahead of the user's key presses. A directly-sent
/// `Event::Render` bypasses skim's own frame-rate cap, so we cap it here.
const RENDER_THROTTLE: Duration = Duration::from_millis(16);

use super::items::{
    HeaderLoading, HeaderSkimItem, LocalCheckout, LocalContent, LocalContentSlot, PickerRow,
    PrStatusSlot, PreviewCache, RowShortcutData, RowUrl, ShortcutTable, worktree_output_token,
};
use super::preview::PreviewMode;
use super::preview_orchestrator::PreviewOrchestrator;
use crate::commands::list::collect::PickerProgressHandler;
use crate::commands::list::model::{BranchScope, ItemKind, ListItem};

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
    /// The `alt-y` / `alt-o` lookup table (token → branch + URL). Replaced
    /// atomically in `on_skeleton` with this skeleton's worktree/branch rows;
    /// the `--prs` thread extends it with PR/MR rows. See [`ShortcutTable`].
    pub(super) shortcut_table: ShortcutTable,
    /// One `Arc<Mutex<String>>` per data row — same Arcs `PickerRow`
    /// holds. Set once in `on_skeleton`, read lock-free thereafter.
    pub(super) rendered_slots: OnceLock<Box<[Arc<Mutex<String>>]>>,
    /// One live `pr_status` slot per data row — same `PrStatusSlot` Arcs the
    /// `PickerRow`s hold. Set once in `on_skeleton` (primed from the
    /// cache-filled snapshot), then written by `on_update` as the `CiStatus`
    /// task reports, so the `pr` tab reflects the live fetch.
    pub(super) pr_status_slots: OnceLock<Box<[PrStatusSlot]>>,
    /// Per data row, the PR/MR number whose `comments` thread was last fetched
    /// (`0` = none yet). Set once in `on_skeleton`. The `comments` tab's
    /// background forge fetch must fire at most once per PR, but the PR surfaces
    /// asynchronously (a cache prime at skeleton, or the live `CiStatus` task via
    /// `on_update`) and `on_update` fires repeatedly — so a fetch fires only when
    /// the observed number differs from this slot, and a corrected number (a
    /// reused branch, a stale prime) re-fetches and keeps the `comments` tab
    /// consistent with the `pr` tab. See [`Self::maybe_spawn_comments`].
    pub(super) comments_fetched: OnceLock<Box<[Arc<AtomicU64>]>>,
    /// One live `LocalContent` slot per data row — same Arcs the
    /// `PickerRow`s hold. Set once in `on_skeleton` (all-loading), then
    /// overwritten by `on_update` from the row's `ListItem` as the list pipeline
    /// lands, so the `working_tree` / `branch_diff` / `upstream` tabs dim once
    /// their diff is known empty.
    pub(super) local_content_slots: OnceLock<Box<[LocalContentSlot]>>,
    pub(super) preview_cache: PreviewCache,
    /// Fresh `Repository` for this spawn, used for the mutation-sensitive
    /// `on_skeleton` reads (`list_worktrees`, `local_branches`). The
    /// `orchestrator` carries its own startup-cloned repo shared across every
    /// spawn — reading worktrees/branches through that re-probes a
    /// `RepoCache.worktrees`/`local_branches` `OnceCell` primed at startup and
    /// never invalidated, so after an in-picker removal it would yield the stale
    /// pre-removal set. `spawn` rebuilds this repo per pass (same `spawn_repo`
    /// the collect/prs threads use); read inventories through it, not through
    /// `orchestrator.repo()`.
    pub(super) repo: Repository,
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
    /// Handoff to the `--prs` thread: the layout's column geometry (PR rows
    /// align to the worktree grid) plus the branches already shown (so `--prs`
    /// skips PRs already represented). Filled in `on_skeleton`. See
    /// [`super::prs::Skeleton`].
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

    /// Spawn row `idx`'s `comments` background fetch if its branch has an open
    /// PR and that PR's thread hasn't been fetched yet. The `comments` tab on a
    /// worktree row shows the same forge-fetched thread a `--prs` row's tab does
    /// (see `items::PickerRow::render_comments_pane`); the PR number comes
    /// from the row's live status, which arrives asynchronously, so this fires
    /// from both the skeleton prime and `on_update`. The per-row
    /// [`Self::comments_fetched`] slot records which PR number was fetched, so
    /// repeated `on_update`s short-circuit — and a *changed* number (a reused
    /// branch, a stale prime corrected by the live fetch) drops the now-wrong
    /// cached thread and re-fetches, keeping the `comments` tab consistent with
    /// the `pr` tab. Keyed by branch name — `--prs` rows key by their `pr:{N}`
    /// token, and git forbids `:` in branch names, so the two keyspaces never
    /// collide.
    fn maybe_spawn_comments(
        &self,
        idx: usize,
        branch_name: &str,
        pr_status: &Option<Option<PrStatus>>,
    ) {
        let Some(Some(status)) = pr_status else {
            return;
        };
        let Some(pr_ref) = status.number else { return };
        let Some(slots) = self.comments_fetched.get() else {
            return;
        };
        let Some(slot) = slots.get(idx) else { return };
        let number = pr_ref.number;
        let previous = slot.swap(number, Ordering::Relaxed);
        if previous == number {
            return; // this PR's comments are already fetched (or in flight)
        }
        if previous != 0 {
            // The live fetch corrected the PR number — drop the stale thread so
            // the re-fetch (below) replaces it rather than serving the old PR.
            self.preview_cache
                .remove(&(branch_name.to_string(), PreviewMode::Comments));
        }
        super::prs::spawn_worktree_comments_fetch(
            &self.orchestrator,
            branch_name.to_string(),
            number as u32,
            self.preview_dims.0,
        );
    }
}

/// Branch names the skeleton shows, for the `--prs` thread's dedup: a PR whose
/// head branch is in this set is already on screen, so its row is dropped. A
/// remote row ("origin/foo") also contributes its bare "foo" — remote names
/// carry no '/', so the first segment is the remote — so a PR for "foo" dedups
/// against a shown "origin/foo". Detached rows (no branch) contribute nothing.
fn collect_shown_branches(items: &[ListItem]) -> HashSet<String> {
    let mut shown = HashSet::new();
    for item in items {
        let Some(name) = item.branch.as_deref() else {
            continue;
        };
        shown.insert(name.to_string());
        if matches!(item.kind, ItemKind::Branch(BranchScope::Remote))
            && let Some((_, bare)) = name.split_once('/')
        {
            shown.insert(bare.to_string());
        }
    }
    shown
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

        // Hand the `--prs` thread the column geometry plus the branches already
        // shown, so it aligns PR rows *and* skips PRs already represented by a
        // worktree/branch row (see `prs::Skeleton`). Built before the row loop
        // consumes `items`, and set before any other handoff so the `--prs`
        // thread's wait sees both.
        self.grid_slot.set(super::prs::Skeleton {
            grid,
            shown_branches: collect_shown_branches(&items),
        });

        let mut slots: Vec<Arc<Mutex<String>>> = Vec::with_capacity(items.len());
        let mut pr_slots: Vec<PrStatusSlot> = Vec::with_capacity(items.len());
        let mut comments_slots: Vec<Arc<AtomicU64>> = Vec::with_capacity(items.len());
        let mut local_content_slots: Vec<LocalContentSlot> = Vec::with_capacity(items.len());
        let mut skim_items: Vec<Arc<dyn SkimItem>> = Vec::with_capacity(items.len() + 1);
        let mut list_items: Vec<Arc<ListItem>> = Vec::with_capacity(items.len());
        // `alt-y` / `alt-o` lookup entries for this skeleton's rows, keyed by the
        // same `output()` token the rows carry. Replaces the table wholesale below
        // (a refresh re-collect rebuilds it); the `--prs` thread extends it later.
        let mut shortcut_map: HashMap<String, RowShortcutData> =
            HashMap::with_capacity(items.len());

        // Synchronous skeleton-time tab-availability facts (see `TabAvailability`).
        // Branches with an upstream tracking ref drive the tab-4 (remote⇅) empty
        // state, read from the pre-skeleton `for-each-ref` inventory — never the
        // async `item.upstream`. A `local_branches()` failure yields the empty set
        // (every branch reads as no-upstream); preview rendering must not error.
        let upstream_branches: HashSet<String> = self
            .repo
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
            .repo
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
            // `search_base` is the stable head of the matcher text: branch +
            // distinct path. The PR/MR tokens (reference, title, author) and the
            // trailing gutter glyph are appended live in
            // `PickerRow::text()`, which reads the row's `pr_status` slot
            // each time skim matches — so a PR filters by number/title/author as
            // soon as the CI fetch lands them, the same fields a `--prs` row
            // carries. The gutter glyph stays last so a typed sigil filters by
            // row kind: `+` linked worktrees, `@` the current one, `/`/`|`
            // local/remote branches (`gutter_glyph` is the same skeleton-time
            // fact the rendered Gutter column uses). The folded reference also
            // lets `#` isolate PR-bearing rows (a plain fuzzy char); the GitLab
            // `!` is skim's inverse-match operator, so only the bare number
            // filters those (see
            // `folded_pr_reference_filters_under_skims_default_engine`).
            let gutter = item.kind.gutter_glyph();
            let mut search_base = branch_name.clone();
            if !path_str.is_empty() {
                search_base.push(' ');
                search_base.push_str(&path_str);
            }

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
            comments_slots.push(Arc::new(AtomicU64::new(0)));

            // Prime the diff-content slot from the skeleton snapshot. Its async
            // fields are still `None` here (every diff tab reads as loading →
            // available), except a branch-only row, which has no working tree and
            // so resolves `working_tree` to empty immediately. `on_update`
            // overwrites it as the pipeline lands.
            let local_content_arc: LocalContentSlot =
                Arc::new(Mutex::new(LocalContent::from_item(&item_arc)));
            local_content_slots.push(Arc::clone(&local_content_arc));

            // Shortcut lookup for this row: `alt-y` copies `branch`, `alt-o`
            // opens the URL once the live `pr_status` slot reports one. Carry the
            // raw `Option` (not `branch_name()`'s `"(detached)"` fallback) so
            // `alt-y` no-ops on a detached worktree instead of copying the label.
            let output_token = worktree_output_token(&item_arc, &branch_name);
            shortcut_map.insert(
                output_token.clone(),
                RowShortcutData {
                    branch: item_arc.branch.clone(),
                    url: RowUrl::Live(Arc::clone(&pr_status_arc)),
                },
            );

            skim_items.push(Arc::new(PickerRow {
                search_base,
                gutter,
                rendered: rendered_arc,
                branch_name,
                output_token,
                preview_cache: Arc::clone(&self.preview_cache),
                pr_status: pr_status_arc,
                notifier: Arc::clone(self.orchestrator.notifier()),
                local: Some(LocalCheckout {
                    has_upstream,
                    summaries_enabled,
                    local_content: local_content_arc,
                }),
            }) as Arc<dyn SkimItem>);
        }

        // Publish slots + skim items before sending to skim so alt-x reload
        // (which reads `shared_items`) sees a populated list the moment
        // skim calls `CommandCollector::invoke`.
        let _ = self.rendered_slots.set(slots.into_boxed_slice());
        let _ = self.pr_status_slots.set(pr_slots.into_boxed_slice());
        let _ = self.comments_fetched.set(comments_slots.into_boxed_slice());
        let _ = self
            .local_content_slots
            .set(local_content_slots.into_boxed_slice());
        *self.shared_items.lock().unwrap() = skim_items.clone();
        *self.shortcut_table.lock().unwrap() = shortcut_map;

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
        // A row whose CI status was primed from cache already knows its PR at
        // skeleton time — kick off its `comments` fetch now so the tab is warm.
        // Rows whose PR only surfaces later get theirs from `on_update`.
        for (idx, item) in list_items.iter().enumerate() {
            self.maybe_spawn_comments(idx, item.branch_name(), &item.pr_status);
        }
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
            // `PickerRow::render_pr_pane_cached`.
            self.preview_cache
                .remove(&(item.branch_name().to_string(), PreviewMode::Pr));
        }
        // Mirror the row's live diff-content signals so the `working_tree` /
        // `branch_diff` / `upstream` tabs dim once their diff is known empty.
        // Cheap copy; the snapshot starts all-loading and resolves field-by-field
        // as the pipeline lands.
        if let Some(slots) = self.local_content_slots.get()
            && let Some(slot) = slots.get(idx)
        {
            *slot.lock().unwrap() = LocalContent::from_item(item);
        }
        // If this update is where the row's PR first surfaced, kick off its
        // `comments` background fetch (once per row) so the `comments` tab loads
        // the thread — the same fetch a `--prs` row makes.
        self.maybe_spawn_comments(idx, item.branch_name(), &item.pr_status);
        // `request_render` sends `Event::Render`, which repaints the *list* row
        // (its CI/status cells just changed) but does NOT re-run the preview.
        // The slots just mirrored — `pr_status` (the `pr` / `comments` panes) and
        // `local_content` (the diff tabs' dim state) — feed the preview, so if
        // this is the selected row also poke a `RunPreview` to re-render it:
        // that's what flips its `pr` tab from "Fetching PR status…" to the
        // resolved PR without a keystroke. Scoped to the selected row, so
        // off-screen updates don't thrash the preview (see `PreviewNotifier`).
        self.orchestrator
            .notifier()
            .notify_row_changed(item.branch_name());
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

    /// Build a handler with explicit `repo` (the per-spawn inventory source)
    /// and `orchestrator` (preview compute). Diverging the two lets a test
    /// prove which one `on_skeleton`'s inventory reads consult. `render_tx` is
    /// shared with the orchestrator's notifier (as `handle_picker` shares it),
    /// so a test can publish a channel into it and observe both the list redraw
    /// (`Event::Render`, via `request_render`) and the notifier's
    /// `Event::RunPreview`.
    fn handler_with(
        repo: Repository,
        orchestrator: Arc<PreviewOrchestrator>,
        tx: SkimItemSender,
        render_tx: Arc<OnceLock<tokio::sync::mpsc::Sender<Event>>>,
    ) -> PickerHandler {
        let preview_cache: PreviewCache = Arc::clone(&orchestrator.cache);
        PickerHandler {
            tx,
            render_tx,
            last_render_poke: Mutex::new(Instant::now()),
            shared_items: Arc::new(Mutex::new(Vec::new())),
            shortcut_table: Arc::new(Mutex::new(std::collections::HashMap::new())),
            rendered_slots: OnceLock::new(),
            pr_status_slots: OnceLock::new(),
            comments_fetched: OnceLock::new(),
            local_content_slots: OnceLock::new(),
            preview_cache,
            repo,
            orchestrator,
            preview_dims: (80, 24),
            llm_command: None,
            summary_hint: Some("disabled".to_string()),
            stashed_warnings: Arc::new(Mutex::new(Vec::new())),
            deferred_items: OnceLock::new(),
            grid_slot: Arc::new(super::super::prs::GridSlot::new()),
            prs_loading: None,
        }
    }

    fn make_handler() -> (PickerHandler, TestRepo, SkimItemReceiver) {
        let test = TestRepo::with_initial_commit();
        let (tx, rx): (SkimItemSender, SkimItemReceiver) = unbounded();
        // Share one `render_tx` between the orchestrator's notifier and the
        // handler, as `handle_picker` does — so a test can publish a channel into
        // `handler.render_tx` and observe both the list redraw and the notifier's
        // `Event::RunPreview` on it.
        let render_tx: Arc<OnceLock<tokio::sync::mpsc::Sender<Event>>> = Arc::new(OnceLock::new());
        let orchestrator = Arc::new(PreviewOrchestrator::new(
            test.repo.clone(),
            Arc::clone(&render_tx),
        ));
        let handler = handler_with(test.repo.clone(), orchestrator, tx, render_tx);
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

    /// `on_skeleton` reads the worktree/branch inventory through the handler's
    /// own per-spawn `repo`, NOT the orchestrator's startup-cloned repo. `spawn`
    /// rebuilds `repo` fresh each refresh; reading inventory through the
    /// orchestrator's never-invalidated `RepoCache` would serve the stale
    /// pre-removal snapshot after an in-picker removal. Here the two repos live
    /// under different temp parents, so the matcher path-base
    /// (`list_worktrees().first()`) differs — a row under the handler-repo's tree
    /// strips to a relative tail only if `on_skeleton` consulted `self.repo`.
    #[test]
    fn on_skeleton_reads_inventory_from_handler_repo_not_orchestrator() {
        use crate::commands::list::model::{ItemKind, WorktreeData};

        let self_repo = TestRepo::with_initial_commit();
        let orchestrator_repo = TestRepo::with_initial_commit();
        let (tx, rx): (SkimItemSender, SkimItemReceiver) = unbounded();
        let orchestrator = Arc::new(PreviewOrchestrator::new(
            orchestrator_repo.repo.clone(),
            Arc::new(OnceLock::new()),
        ));
        let handler = handler_with(
            self_repo.repo.clone(),
            orchestrator,
            tx,
            Arc::new(OnceLock::new()),
        );

        // A worktree row under the handler-repo's tree. The shared parent strips
        // only if path_base comes from self_repo (parent of `<temp>/repo`), not
        // from orchestrator_repo (a different temp parent).
        let mut item = ListItem::new_branch("abc".into(), "feature".into());
        item.kind = ItemKind::Worktree(Box::new(WorktreeData {
            path: self_repo.path().join("feature"),
            ..Default::default()
        }));
        handler.on_skeleton(vec![item], vec!["skel".into()], header("hdr"), grid());

        let received = rx.recv().expect("skeleton batch");
        // received[0] is the header; received[1] is the worktree row.
        let matcher_text = received[1].text().into_owned();
        // The stripped tail renders with the OS separator (`to_string_lossy`),
        // so build the expected tail the same way rather than hardcoding `/`
        // (the row path is `repo\feature` on Windows).
        let expected_tail = Path::new("repo").join("feature");
        assert!(
            matcher_text.contains(expected_tail.to_string_lossy().as_ref()),
            "row path should appear in the matcher text: {matcher_text:?}"
        );
        // The buggy orchestrator-repo path_base can't strip a self_repo path, so
        // it would leave the absolute `<temp>/repo` prefix in the matcher text.
        assert!(
            !matcher_text.contains(self_repo.path().to_string_lossy().as_ref()),
            "path_base from self.repo must strip the absolute prefix: {matcher_text:?}"
        );
    }

    /// Skeleton → update → reveal: verifies that each event writes through
    /// to the shared `rendered` string the `PickerRow` holds. Skim
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

        // local_content slots start primed from the skeleton snapshots — these are
        // branch-only rows (no worktree), so `working_tree` already resolves empty
        // while the rest stays loading.
        let lc_slots = handler.local_content_slots.get().unwrap();
        let skeleton_lc =
            LocalContent::from_item(&ListItem::new_branch("aaa".into(), "one".into()));
        assert_eq!(*lc_slots[0].lock().unwrap(), skeleton_lc);
        assert_eq!(*lc_slots[1].lock().unwrap(), skeleton_lc);

        // on_update rewrites a single render slot (the second item here) and
        // mirrors that item's CI status and diff-content into its live slots.
        let mut updated = ListItem::new_branch("bbb".into(), "two".into());
        updated.pr_status = Some(None);
        updated.counts = Some(crate::commands::list::model::AheadBehind {
            ahead: 1,
            behind: 0,
        });
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
        assert_eq!(
            *lc_slots[1].lock().unwrap(),
            LocalContent::from_item(&updated),
            "row 1 diff-content mirrored from the updated item"
        );
        assert_eq!(
            *lc_slots[0].lock().unwrap(),
            skeleton_lc,
            "row 0 diff-content untouched"
        );

        // on_reveal rewrites every slot — slot writes are idempotent
        // through `Mutex<String>`, so unconditional updates are safe.
        handler.on_reveal(vec!["rev-one".into(), "rev-two".into()]);
        assert_eq!(*slots[0].lock().unwrap(), "rev-one");
        assert_eq!(*slots[1].lock().unwrap(), "rev-two");
    }

    /// `on_update` mirrors a row's live CI status into the preview-feeding slots,
    /// then pokes `Event::RunPreview` so the selected row's `pr` / `comments` tab
    /// re-renders from the new status without a keystroke. This is the loop the
    /// orchestrator-fill path can't close — `on_update` isn't a cache fill, so it
    /// can't ride `PreviewOrchestrator::fill`'s notify; the poke is wired
    /// separately here. Scoped to the selected row: an update for a row the
    /// cursor isn't on repaints the list (`Event::Render`) but must not re-run
    /// the visible preview. Fast producer-site guard for that wiring; the
    /// end-to-end path is also covered by the PTY test
    /// `test_switch_picker_pr_tab_auto_resolves_from_fetching`.
    #[test]
    fn on_update_pokes_run_preview_for_the_selected_row() {
        let (handler, _test, rx) = make_handler();
        // Publish skim's event sender (shared with the orchestrator's notifier,
        // as in production) so both the list redraw and the preview re-run land
        // on this channel.
        let (tx, mut render_rx) = tokio::sync::mpsc::channel::<Event>(8);
        handler.render_tx.set(tx).unwrap();

        handler.on_skeleton(
            vec![ListItem::new_branch("aaa".into(), "feature".into())],
            vec!["s".into()],
            header("hdr"),
            grid(),
        );
        let _ = rx.recv(); // drain the skeleton batch

        // A live-status change for the row: CI reported "no PR".
        let mut updated = ListItem::new_branch("aaa".into(), "feature".into());
        updated.pr_status = Some(None);

        let run_previews = |rx: &mut tokio::sync::mpsc::Receiver<Event>| {
            let mut n = 0;
            while let Ok(ev) = rx.try_recv() {
                if matches!(ev, Event::RunPreview) {
                    n += 1;
                }
            }
            n
        };

        // Cursor is on `feature`'s `pr` tab (still loading) → its CI update
        // re-runs the preview.
        handler
            .orchestrator
            .notifier()
            .note_awaiting("feature", PreviewMode::Pr);
        handler.on_update(0, "r".into(), &updated);
        assert!(
            run_previews(&mut render_rx) >= 1,
            "the selected row's CI update re-runs its preview"
        );

        // Cursor is on a different row → `feature`'s update must not re-run the
        // preview the cursor is showing (no thrash). `request_render`'s throttle
        // may swallow the `Event::Render` too, but the `RunPreview` poke is
        // unthrottled, so its absence is the meaningful signal.
        handler
            .orchestrator
            .notifier()
            .note_awaiting("other", PreviewMode::Pr);
        handler.on_update(0, "r".into(), &updated);
        assert_eq!(
            run_previews(&mut render_rx),
            0,
            "an update to a row the cursor isn't on doesn't re-run the visible preview"
        );
    }

    /// A row's `comments` fetch is once-per-PR, keyed by branch name — but if the
    /// live CI fetch corrects the PR number (a stale prime, a reused branch), the
    /// now-wrong cached thread is dropped and re-fetched, so the `comments` tab
    /// can't keep serving the old PR's thread under the new number. The
    /// `make_handler` repo has no forge, so the fetch caches a terminal pane
    /// synchronously; the test watches the cache key being invalidated and
    /// repopulated across the number change.
    #[test]
    fn comments_refetch_on_pr_number_change() {
        use crate::commands::list::ci_status::{CiSource, CiStatus, PrRef, PrStatus};

        let (handler, _test, rx) = make_handler();
        handler.on_skeleton(
            vec![ListItem::new_branch("aaa".into(), "feature".into())],
            vec!["s".into()],
            header("hdr"),
            grid(),
        );
        let _ = rx.recv();

        let with_pr = |number: u64| {
            let mut item = ListItem::new_branch("aaa".into(), "feature".into());
            item.pr_status = Some(Some(PrStatus {
                ci_status: CiStatus::Passed,
                source: CiSource::PullRequest,
                is_stale: false,
                is_priming: false,
                url: None,
                number: Some(PrRef::pr(number)),
                review_state: None,
                title: None,
                body: None,
                author: None,
                comment_count: None,
            }));
            item
        };
        let key = ("feature".to_string(), PreviewMode::Comments);

        // First the row resolves to PR #41 → its comments fetch fires and caches.
        handler.on_update(0, "r".into(), &with_pr(41));
        handler.orchestrator.wait_for_idle();
        assert!(
            handler.preview_cache.contains_key(&key),
            "PR #41's comments fetch populated the branch-keyed cache"
        );

        // A repeat update with the SAME number must not re-fetch: overwrite with a
        // sentinel and confirm it survives.
        handler
            .preview_cache
            .insert(key.clone(), "SENTINEL-41".into());
        handler.on_update(0, "r".into(), &with_pr(41));
        handler.orchestrator.wait_for_idle();
        assert_eq!(
            handler
                .preview_cache
                .get(&key)
                .map(|v| v.clone())
                .as_deref(),
            Some("SENTINEL-41"),
            "same PR number short-circuits — no re-fetch"
        );

        // The live fetch corrects the number to #42 → the stale #41 thread is
        // dropped and a fresh fetch repopulates the key.
        handler.on_update(0, "r".into(), &with_pr(42));
        handler.orchestrator.wait_for_idle();
        assert_ne!(
            handler
                .preview_cache
                .get(&key)
                .map(|v| v.clone())
                .as_deref(),
            Some("SENTINEL-41"),
            "a corrected PR number drops the stale thread and re-fetches"
        );
        assert!(
            handler.preview_cache.contains_key(&key),
            "the re-fetch repopulated the comments cache for #42"
        );
    }

    /// Header + items get published in order. `output()` of the
    /// PickerRow is the branch name so skim returns the correct
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

        // The shortcut table is keyed by each row's `output()` token (the branch
        // name for these branch-only rows) and carries the branch for `alt-y`.
        let table = handler.shortcut_table.lock().unwrap();
        assert_eq!(table.len(), 2);
        assert_eq!(
            table.get("feat-a").and_then(|d| d.branch.as_deref()),
            Some("feat-a")
        );
        assert_eq!(
            table.get("feat-b").and_then(|d| d.branch.as_deref()),
            Some("feat-b")
        );
    }

    /// A detached worktree row has no branch, so its shortcut entry carries
    /// `branch: None` — `alt-y` then no-ops rather than copying the
    /// `"(detached)"` label `branch_name()` falls back to.
    #[test]
    fn shortcut_table_branch_is_none_for_detached_worktree() {
        use crate::commands::list::model::{ItemKind, WorktreeData};

        let (handler, _test, _rx) = make_handler();
        let mut item = ListItem::new_branch("abc123".into(), "(detached)".into());
        item.branch = None;
        item.kind = ItemKind::Worktree(Box::new(WorktreeData {
            path: std::path::PathBuf::from("/tmp/wt-detached"),
            detached: true,
            ..Default::default()
        }));

        handler.on_skeleton(vec![item], vec!["skel".into()], header("hdr"), grid());

        let table = handler.shortcut_table.lock().unwrap();
        assert_eq!(table.len(), 1);
        let entry = table.values().next().expect("one row in the table");
        assert_eq!(
            entry.branch, None,
            "a detached worktree carries no branch, so alt-y no-ops"
        );
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

    /// A worktree row's matcher text folds in its PR's reference, title, and
    /// author — the same fields a `--prs` row carries — so a PR filters
    /// identically however it's shown. Here the PR is primed at skeleton (the
    /// cache case); the gutter glyph stays trailing for sigil filtering.
    #[test]
    fn worktree_row_matcher_text_folds_in_pr_number_title_author() {
        use crate::commands::list::ci_status::{CiSource, CiStatus, PrRef, PrStatus};

        let (handler, _test, rx) = make_handler();
        let mut item = ListItem::new_branch("abc".into(), "localbr".into());
        item.pr_status = Some(Some(PrStatus {
            ci_status: CiStatus::Passed,
            source: CiSource::PullRequest,
            is_stale: false,
            is_priming: false,
            url: None,
            number: Some(PrRef::pr(123)),
            review_state: None,
            title: Some("Speed up startup".into()),
            body: None,
            author: Some("alice".into()),
            comment_count: None,
        }));
        handler.on_skeleton(vec![item], vec!["skel".into()], header("hdr"), grid());

        let received = rx.recv().expect("skeleton batch");
        let text = received[1].text().into_owned();
        // The CI column shows `#123`; typing the number (or title/author) filters.
        assert!(text.contains("#123"), "PR reference folded in: {text:?}");
        assert!(
            text.contains("Speed up startup"),
            "title folded in: {text:?}"
        );
        assert!(text.contains("alice"), "author folded in: {text:?}");
        assert!(
            text.contains("localbr"),
            "branch still searchable: {text:?}"
        );
        assert!(text.ends_with('/'), "gutter glyph stays trailing: {text:?}");
    }

    /// The freeze is relaxed: a worktree row's matcher text is read live from its
    /// `pr_status` slot, so a PR the live CI fetch discovers (cold cache, nothing
    /// primed at skeleton) becomes filterable by number/title/author once
    /// `on_update` lands it — not just on a later run.
    #[test]
    fn worktree_row_matcher_text_updates_when_live_fetch_lands_pr() {
        use crate::commands::list::ci_status::{CiSource, CiStatus, PrRef, PrStatus};

        let (handler, _test, rx) = make_handler();
        // Skeleton with no PR (cold cache).
        handler.on_skeleton(
            vec![ListItem::new_branch("abc".into(), "localbr".into())],
            vec!["skel".into()],
            header("hdr"),
            grid(),
        );
        let received = rx.recv().expect("skeleton batch");
        let row = Arc::clone(&received[1]);
        assert!(
            !row.text().contains("#7"),
            "no PR before the fetch: {:?}",
            row.text()
        );

        // The live fetch reports a PR for the row.
        let mut updated = ListItem::new_branch("abc".into(), "localbr".into());
        updated.pr_status = Some(Some(PrStatus {
            ci_status: CiStatus::Passed,
            source: CiSource::PullRequest,
            is_stale: false,
            is_priming: false,
            url: None,
            number: Some(PrRef::pr(7)),
            review_state: None,
            title: Some("Add caching".into()),
            body: None,
            author: Some("bob".into()),
            comment_count: None,
        }));
        handler.on_update(0, "rendered".into(), &updated);

        // Same row item; its matcher text now reflects the landed PR.
        let text = row.text().into_owned();
        assert!(text.contains("#7"), "number now filters: {text:?}");
        assert!(text.contains("Add caching"), "title now filters: {text:?}");
        assert!(text.contains("bob"), "author now filters: {text:?}");
        assert!(text.ends_with('/'), "gutter glyph stays trailing: {text:?}");
    }

    /// `collect_shown_branches` gathers branch names for the `--prs` dedup, and
    /// adds a remote row's bare name so a PR for "foo" dedups against a shown
    /// "origin/foo".
    #[test]
    fn collect_shown_branches_adds_remote_bare_names() {
        let items = vec![
            ListItem::new_branch("a".into(), "local-feat".into()),
            ListItem::new_remote_branch("b".into(), "origin/remote-feat".into()),
        ];
        let shown = collect_shown_branches(&items);
        assert!(shown.contains("local-feat"), "local branch name");
        assert!(shown.contains("origin/remote-feat"), "full remote ref");
        assert!(
            shown.contains("remote-feat"),
            "bare name so a PR head dedups against the remote row"
        );
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

    /// Locks the PR/MR filtering contract: a worktree row folds its PR's
    /// reference, title, and author into the matcher text, so under skim's
    /// default engine all three filter the row regardless of forge sigil. `#`
    /// (GitHub/Gitea/Azure) is a plain fuzzy char that also isolates PR-bearing
    /// rows, while the GitLab `!` is skim's inverse-match operator, so typing the
    /// literal `!7` *excludes* its own MR row — only the number filters there.
    /// Companion to `gutter_glyphs_filter_under_skims_default_engine`.
    #[test]
    fn folded_pr_reference_filters_under_skims_default_engine() {
        struct TextItem(String);
        impl SkimItem for TextItem {
            fn text(&self) -> std::borrow::Cow<'_, str> {
                std::borrow::Cow::Borrowed(&self.0)
            }
        }
        let matches = |query: &str, haystack: &str| -> bool {
            AndOrEngineFactory::new(ExactOrFuzzyEngineFactory::builder().build())
                .create_engine(query)
                .match_item(&TextItem(haystack.to_string()))
                .is_some()
        };

        // Folded matcher text for a worktree row carrying a PR (`#123`) / MR
        // (`!7`) — branch, path, reference, title, author, gutter — and one with
        // no PR. The PR fields are exactly what a `--prs` row also folds in.
        let github = "feature feature.wt #123 Add caching alice +";
        let gitlab = "feature feature.wt !7 Speed up startup bob +";
        let no_pr = "plain plain.wt +";

        // The number, title, and author all filter — the same fields in both
        // pickers, regardless of forge sigil.
        assert!(matches("123", github), "PR number filters the GitHub row");
        assert!(matches("7", gitlab), "MR number filters the GitLab row");
        assert!(matches("caching", github), "title filters");
        assert!(matches("alice", github), "author filters");
        assert!(matches("bob", gitlab), "MR author filters");
        // `#` is a plain fuzzy char: it isolates PR-bearing rows and skips
        // PR-less ones (same as the `--prs` gutter sigil).
        assert!(matches("#", github), "# selects a PR-bearing row");
        assert!(!matches("#", no_pr), "# skips a PR-less row");
        // `!7` is skim's inverse-match operator — it excludes rows containing
        // `7`, hiding the very MR row it names. The bare number is reliable.
        assert!(!matches("!7", gitlab), "!7 inverse-excludes its own MR row");
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
