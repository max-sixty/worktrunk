//! Picker preview caching — the whole system.
//!
//! This module orchestrates the picker's preview content and sits on top of two
//! cache tiers whose pieces live across several modules; this docstring is the
//! map that ties them together. The disk tiers are in [`super::preview_cache`]
//! and [`crate::summary`], the repaint loop in [`super::preview_notify`], the
//! synchronous read path in [`super::items`], and the cross-spawn lifetime in
//! the picker's `PipelineFactory` (`src/commands/picker/mod.rs`).
//!
//! # Two tiers
//!
//! **In-memory** — [`PreviewCache`], an `Arc<DashMap<(row-key, mode), String>>`.
//! The only *cache tier* `SkimItem::preview` reads (a lock-free `get`; it never
//! touches disk) — a miss renders a loading placeholder. (`preview()` also reads
//! the row's live `pr_status` / `local_content` slots for tab availability and
//! the Pr/Comments panes, but those aren't cache reads.) Session-scoped, and
//! **shared across every
//! `alt-r` spawn** (the one `Arc` is reused). The key is `(row-key, mode)`,
//! where row-key is the branch for a worktree row or the `pr:N` / `mr:N` token
//! for a `--prs` row — with **no SHA or content hash**. Two consequences: every
//! mode shares one key shape (a `--prs` row's forge fetches and a worktree
//! row's git diffs coexist), and a `git fetch` or new commit that moves a
//! branch does *not* invalidate the entry — the key is unchanged, so a warm
//! entry can outlive the content it was computed from. That staleness is
//! reconciled two ways, both below: per-event invalidation for the PR tabs, and
//! a wholesale clear on refresh.
//!
//! **On-disk** — content-addressed, cross-session, consulted only on an
//! in-memory miss. [`super::preview_cache`] holds Log / BranchDiff /
//! UpstreamDiff keyed by git SHA(s) + dimensions; [`crate::summary`] holds
//! summaries keyed by a hash of the diff. WorkingTree, Pr, and Comments have no
//! disk tier. Because these keys *are* content-addressed, moved content yields a
//! fresh key and a natural miss — the disk tier is never stale, which is what
//! makes clearing the in-memory tier above it cheap (an unchanged branch
//! re-reads disk; only changed content recomputes).
//!
//! # What backs an in-memory miss, per mode
//!
//! | Mode | Disk tier | Recompute on miss |
//! |------|-----------|-------------------|
//! | WorkingTree | none (a dirty tree has no stable hash) | live `git diff HEAD` |
//! | Log / BranchDiff / UpstreamDiff | [`super::preview_cache`], SHA-keyed | `git`, then write disk |
//! | Summary | [`crate::summary`], diff-hash-keyed | LLM, then write disk |
//! | Pr | none | render from the already-fetched CI/PR data |
//! | Comments | [`super::preview_cache`], `updatedAt`-keyed (GitHub PRs only) | forge fetch, then write disk |
//!
//! # Invalidation
//!
//! - **Pr / Comments** self-invalidate on the CI path: `on_update` drops the
//!   `(branch, Pr)` entry when a row's live status changes, `--prs` rows drop
//!   theirs on rebuild, and a corrected PR number drops the stale `Comments`
//!   thread (see [`super::progressive_handler`]).
//! - **WorkingTree / Log / BranchDiff / UpstreamDiff / Summary** have *no*
//!   per-event in-memory invalidation. Within a session they are reconciled with
//!   moved content only by a refresh.
//! - **Refresh (`alt-r`)** clears the entire in-memory cache (in
//!   `PipelineFactory::spawn`, gated on `rebuild_repo`). What then refreshes is
//!   bounded by what the recompute sees: the rebuilt inventory gives each row a
//!   current `item.head()`, so the live working-tree diff, the log, and a branch
//!   whose own commits moved all recompute correctly. But precompute runs against
//!   the orchestrator's *startup* repo — it's built once and shared (`Arc`),
//!   never rebuilt — so values read from its `RepoCache`, notably the
//!   default-branch base SHA for BranchDiff, stay at session start; a default
//!   branch that moved externally isn't picked up until the picker reopens. The
//!   disk tiers keep an unchanged branch cheap; only genuinely changed content
//!   pays.
//!
//!   Two known limitations follow from that once-built orchestrator: (1) the
//!   stale base SHA above; (2) a narrow race — a prior spawn's still-draining
//!   precompute task computes against its captured (now stale) `item.head()` and,
//!   because the clear emptied the cache, *fills* it instead of short-circuiting,
//!   after which the new spawn's task short-circuits on that stale entry, which
//!   then persists until the next refresh. The window opens only when a refresh
//!   fires while a prior spawn's precompute is still draining (large repo / slow
//!   summaries) and the row's content moved in that window — the common "I edited
//!   the branch I'm viewing" case doesn't hit it, since that branch's precompute
//!   finished when the picker opened. The structural fix for both is to give the
//!   orchestrator the current spawn's repo plus a spawn generation (mirroring
//!   `prs_epoch`) so a superseded fill drops.
//!
//! # Filling and surfacing
//!
//! Every background producer routes through the one [`PreviewOrchestrator::fill`]
//! choke point: it inserts into the cache and pokes [`super::preview_notify`] so a
//! compute that lands after skim already drew the pane repaints without a
//! keystroke. Precompute is tiered — [`PreviewOrchestrator::spawn_initial_precompute`]
//! at skeleton time (item 0 × the four local modes + summary, plus every row's
//! default tab) and [`PreviewOrchestrator::spawn_deferred_precompute`] after the
//! row drain (the rest); Pr and Comments are never precomputed. Both tiers re-run
//! on every spawn, including a refresh (after its clear). `spawn_preview` and
//! `spawn_compute` short-circuit on an in-memory hit, so a refresh must clear
//! first or their recompute is a no-op; `spawn_summary` has no such guard and
//! always recomputes — cheap, since `crate::summary` is gated by its own
//! diff-hash disk cache.
//!
//! # Orchestration
//!
//! Routes preview tasks to the dedicated [`COLLECT_POOL`] (shared with the
//! row pipeline) and tracks the in-memory cache. A single pool lets workers
//! prefer whichever workload has dominant pressure: row tasks land on
//! workers' local deques and take priority during drain; preview tasks
//! sit in the pool's injector and pick up workers as they free.
//!
//! `COLLECT_POOL` is deliberately *not* the global pool: skim runs its
//! per-keystroke fuzzy matcher on the global pool, so keeping these blocking
//! git tasks off it is what prevents the picker from freezing on the first
//! keystroke. See [`COLLECT_POOL`] for the full reasoning.
//!
//! Provides a pending-task counter for the dry-run path
//! (`WORKTRUNK_PICKER_DRY_RUN`) and tests, both of which want to wait for
//! all spawned tasks to complete before reading the cache. The picker
//! entry point (`handle_picker`) uses this for its real spawns.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use dashmap::DashMap;
use skim::prelude::Event;
use tokio::sync::mpsc::Sender;
use worktrunk::git::Repository;

use super::items::{PickerRow, PreviewCache, PreviewCacheKey};
use super::preview::PreviewMode;
use super::preview_notify::PreviewNotifier;
use super::summary;
use crate::commands::list::collect::COLLECT_POOL;
use crate::commands::list::model::ListItem;

/// The picker's initial preview tab — `WorkingTree`, shown when the
/// picker opens. Pre-computed for every row at skeleton time so j/k
/// navigation lands on warm content without paying the 4-mode bulk cost
/// per row during the row-fill window.
const INITIAL_MODE: PreviewMode = PreviewMode::WorkingTree;

/// Modes other than [`INITIAL_MODE`]. For the user's landing row (item 0)
/// these pre-compute at skeleton time alongside `INITIAL_MODE` so
/// tab-cycling is responsive immediately. For items 1..N they're
/// deferred until `spawn_deferred_precompute` fires (after row drain).
const SECONDARY_MODES: [PreviewMode; 3] = [
    PreviewMode::Log,
    PreviewMode::BranchDiff,
    PreviewMode::UpstreamDiff,
];

struct PendingGuard(Arc<AtomicUsize>);

impl Drop for PendingGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

pub(super) struct PreviewOrchestrator {
    pub(super) cache: PreviewCache,
    pending: Arc<AtomicUsize>,
    /// Bridges each fill to skim's event loop so a finished compute surfaces
    /// without a keystroke (see [`PreviewNotifier`]). Shared with the skim
    /// items, which record their awaited preview; every fill site here notifies.
    notifier: Arc<PreviewNotifier>,
    /// Repository used by preview compute. Captured once at construction
    /// so background tasks see a stable repo binding, and so unit tests
    /// can inject a `TestRepo`-rooted `Repository` instead of relying on
    /// process CWD.
    ///
    /// Cloned into each spawned task so they share the underlying
    /// `Arc<RepoCache>` — including the memoized comparison base that
    /// [`Repository::branch_diff_spec`] resolves from a single `for-each-ref`
    /// scan. That shared cache is how the BranchDiff preview avoids
    /// re-scanning refs per item.
    repo: Repository,
}

impl PreviewOrchestrator {
    pub(super) fn new(repo: Repository, render_tx: Arc<OnceLock<Sender<Event>>>) -> Self {
        Self {
            cache: Arc::new(DashMap::new()),
            pending: Arc::new(AtomicUsize::new(0)),
            notifier: Arc::new(PreviewNotifier::new(render_tx)),
            repo,
        }
    }

    /// The repository this orchestrator computes previews against. Exposed so
    /// `on_skeleton` can read the cached local-branch inventory
    /// (`local_branches()`) for synchronous tab-availability facts.
    pub(super) fn repo(&self) -> &Repository {
        &self.repo
    }

    /// The shared preview notifier, handed to each skim item so its `preview()`
    /// can record what it's awaiting (see [`PreviewNotifier`]).
    pub(super) fn notifier(&self) -> &Arc<PreviewNotifier> {
        &self.notifier
    }

    /// Insert a computed preview into the cache and surface it if the selected
    /// row is awaiting exactly this key. The single fill path: every background
    /// producer routes through this (or the `&self` [`Self::fill_external`]) so a
    /// finished compute can never reach the cache without giving skim the chance
    /// to repaint it.
    fn fill(cache: &PreviewCache, notifier: &PreviewNotifier, key: PreviewCacheKey, value: String) {
        cache.insert(key.clone(), value);
        notifier.notify_filled(&key);
    }

    /// [`Self::fill`] for callers that hold the orchestrator rather than the
    /// captured `cache` / `notifier` clones — the `--prs` comments path's
    /// synchronous "unsupported forge" pane.
    pub(super) fn fill_external(&self, key: PreviewCacheKey, value: String) {
        Self::fill(&self.cache, &self.notifier, key, value);
    }

    /// Spawn a preview compute task. Returns immediately.
    ///
    /// Idempotent on the cache key: if another task already populated it,
    /// this one short-circuits after the `contains_key` check. Compute
    /// happens outside any DashMap lock so skim's UI thread (which calls
    /// `preview()` synchronously and reads via `DashMap::get`) is never
    /// blocked on a shard write held across git/pager subprocesses.
    ///
    /// Log mode that hits the disk cache also enqueues a refresh task
    /// (via `rayon::spawn_fifo`, so it lands behind in-flight foreground
    /// precompute) to recompute the embedded ref decorations before the
    /// next visit. The `spawn_fifo` runs from inside a `COLLECT_POOL`
    /// worker, so it inherits that pool rather than the global one. See the
    /// `LogCacheEntry` docstring for why the disk cache itself is
    /// SHA-keyed but decoration text drifts.
    pub(super) fn spawn_preview(
        &self,
        item: Arc<ListItem>,
        mode: PreviewMode,
        dims: (usize, usize),
    ) {
        let cache = Arc::clone(&self.cache);
        let notifier = Arc::clone(&self.notifier);
        let (w, h) = dims;
        let repo = self.repo.clone();
        let pending = Arc::clone(&self.pending);
        self.spawn_task(move || {
            let cache_key = (item.branch_name().to_string(), mode);
            if cache.contains_key(&cache_key) {
                return;
            }
            let (value, log_disk_hit) =
                PickerRow::compute_and_page_preview(&repo, &item, mode, w, h);
            Self::fill(&cache, &notifier, cache_key, value);
            if log_disk_hit {
                pending.fetch_add(1, Ordering::SeqCst);
                let guard = PendingGuard(Arc::clone(&pending));
                let item = Arc::clone(&item);
                let cache = Arc::clone(&cache);
                let notifier = Arc::clone(&notifier);
                let repo = repo.clone();
                rayon::spawn_fifo(move || {
                    let _g = guard;
                    let rendered = PickerRow::refresh_log_preview(&repo, &item, w, h);
                    // Skip empty results so a transient `git log` failure
                    // doesn't poison the in-memory cache with "" and wipe
                    // out the value the producer just inserted.
                    if !rendered.is_empty() {
                        Self::fill(
                            &cache,
                            &notifier,
                            (item.branch_name().to_string(), PreviewMode::Log),
                            rendered,
                        );
                    }
                });
            }
        });
    }

    /// Spawn an LLM summary task. Returns immediately.
    pub(super) fn spawn_summary(&self, item: Arc<ListItem>, llm_command: String, repo: Repository) {
        let cache = Arc::clone(&self.cache);
        let notifier = Arc::clone(&self.notifier);
        self.spawn_task(move || {
            let summary = summary::generate_summary_for_item(&item, &llm_command, &repo);
            Self::fill(
                &cache,
                &notifier,
                (item.branch_name().to_string(), PreviewMode::Summary),
                summary,
            );
        });
    }

    /// Spawn a preview-compute task whose value comes from a caller-supplied
    /// closure. Returns immediately.
    ///
    /// The general-purpose companion to [`Self::spawn_preview`]: that method
    /// computes a worktree `ListItem`'s preview via the local-git
    /// `compute_and_page_preview`, whereas `--prs` rows (no local checkout) have
    /// no local worktree, so they fetch their `log` / `comments` panes through a
    /// forge CLI and pass that work in as `compute`. Both share the same
    /// [`PreviewCache`], the same `COLLECT_POOL` routing, and the same
    /// pending-counter accounting (so the dry-run path's `wait_for_idle` and
    /// the cache dump cover PR-row fetches too).
    ///
    /// Idempotent on `key` (short-circuits on a cache hit) and runs `compute`
    /// outside any DashMap lock, like `spawn_preview`. A `None` or empty result
    /// is deliberately NOT cached: the slot stays empty (read as "still
    /// loading"), so a later `spawn_compute` with the same key recomputes. The
    /// `--prs` callers spawn once per row and never re-invoke, so they convert a
    /// failed fetch into a terminal "couldn't load" pane and hand that back as
    /// `Some(..)` rather than `None` — an uncached `None` would strand the tab on
    /// its loading placeholder until the picker reopens.
    pub(super) fn spawn_compute<F>(&self, key: PreviewCacheKey, compute: F)
    where
        F: FnOnce(&Repository) -> Option<String> + Send + 'static,
    {
        let cache = Arc::clone(&self.cache);
        let notifier = Arc::clone(&self.notifier);
        let repo = self.repo.clone();
        self.spawn_task(move || {
            if cache.contains_key(&key) {
                return;
            }
            if let Some(value) = compute(&repo)
                && !value.is_empty()
            {
                Self::fill(&cache, &notifier, key, value);
            }
        });
    }

    /// Spawn the skeleton-time pre-compute tier.
    ///
    /// Fires at `on_skeleton`. Two layers of priority:
    /// - First item × all 4 modes + first item summary — the user lands on
    ///   row 0 and frequently tab-cycles modes there.
    /// - Items 1..N × [`INITIAL_MODE`] only — pre-warms the default tab
    ///   for every row so quick j/k navigation hits cached content,
    ///   bounded contention with the row pipeline (~N tasks).
    ///
    /// The remaining [`SECONDARY_MODES`] for items 1..N and their summaries
    /// are deferred to [`Self::spawn_deferred_precompute`], which fires
    /// after the row pipeline tears down.
    pub(super) fn spawn_initial_precompute(
        &self,
        items: &[Arc<ListItem>],
        preview_dims: (usize, usize),
        llm_command: Option<&str>,
    ) {
        let Some(first) = items.first() else { return };

        // First item: all modes + summary.
        self.spawn_preview(Arc::clone(first), INITIAL_MODE, preview_dims);
        for mode in SECONDARY_MODES {
            self.spawn_preview(Arc::clone(first), mode, preview_dims);
        }
        if let Some(llm) = llm_command {
            self.spawn_summary(Arc::clone(first), llm.to_string(), self.repo.clone());
        }

        // Items 1..N: default tab only. Other modes wait for drain.
        for item in items.iter().skip(1) {
            self.spawn_preview(Arc::clone(item), INITIAL_MODE, preview_dims);
        }
    }

    /// Spawn the deferred pre-compute tier for items 1..N.
    ///
    /// Fires from the picker handler's `on_collect_complete` hook — i.e.
    /// after `collect::collect`'s drain ends. `COLLECT_POOL` serves
    /// both the row pipeline and the preview pipeline. Deferring this
    /// tier keeps these submissions out of that pool's injector while
    /// row tasks are still landing on workers' local deques. The
    /// default tab for these rows already fired at skeleton time via
    /// [`Self::spawn_initial_precompute`]; what's left is
    /// [`SECONDARY_MODES`] plus summaries.
    ///
    /// Spawn order: mode-major across previews, then summaries last —
    /// each LLM call can take seconds. Called from outside any rayon
    /// worker (the picker-collect bg thread), so submissions land on
    /// rayon's FIFO injector and workers pick previews before summaries.
    pub(super) fn spawn_deferred_precompute(
        &self,
        rest: &[Arc<ListItem>],
        preview_dims: (usize, usize),
        llm_command: Option<&str>,
    ) {
        for mode in SECONDARY_MODES {
            for item in rest {
                self.spawn_preview(Arc::clone(item), mode, preview_dims);
            }
        }
        if let Some(llm) = llm_command {
            for item in rest {
                self.spawn_summary(Arc::clone(item), llm.to_string(), self.repo.clone());
            }
        }
    }

    /// Seed the Summary cache with a static hint for every item.
    ///
    /// Used when summaries are disabled — gives the Summary tab something
    /// useful instead of a perpetual "Generating…" placeholder. Pure
    /// synchronous `DashMap::insert` calls (zero CPU, no subprocess), so
    /// this runs at skeleton time for every row regardless of position
    /// — no contention concern.
    pub(super) fn seed_summary_hints(&self, items: &[Arc<ListItem>], hint: &str) {
        for item in items {
            self.cache.insert(
                (item.branch_name().to_string(), PreviewMode::Summary),
                hint.to_string(),
            );
        }
    }

    fn spawn_task<F: FnOnce() + Send + 'static>(&self, task: F) {
        self.pending.fetch_add(1, Ordering::SeqCst);
        let guard = PendingGuard(Arc::clone(&self.pending));
        let wrapped = move || {
            // Guard decrements on drop, so a panic inside `task` still
            // releases the counter — otherwise `wait_for_idle` hangs
            // forever on any panicking preview task.
            let _g = guard;
            task();
        };
        // The `pending` counter is independent of which pool the task
        // lands on, so routing through `COLLECT_POOL` (shared with the row
        // pipeline, off the global pool skim's matcher uses) doesn't change
        // `wait_for_idle` semantics in tests or the dry-run path.
        COLLECT_POOL.spawn(wrapped);
    }

    /// Block until all spawned tasks complete.
    ///
    /// Used by the dry-run path and tests; production never waits — tasks
    /// are fire-and-forget while skim runs. Polls at 10ms resolution; tasks
    /// typically take tens to hundreds of ms, so a condvar isn't worth the
    /// complexity.
    pub(super) fn wait_for_idle(&self) {
        while self.pending.load(Ordering::SeqCst) > 0 {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Preview-cache inventory for the dry-run dump: one sorted
    /// `{branch, mode, bytes}` object per cached preview. Byte-length only
    /// (not content) keeps output small and deterministic across terminals.
    pub(super) fn cache_entries_json(&self) -> serde_json::Value {
        let mut entries: Vec<_> = self
            .cache
            .iter()
            .map(|e| {
                let (branch, mode) = e.key();
                (branch.clone(), *mode as u8, e.value().len())
            })
            .collect();
        entries.sort();

        entries
            .into_iter()
            .map(|(branch, mode, bytes)| {
                serde_json::json!({ "branch": branch, "mode": mode, "bytes": bytes })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::list::model::{ItemKind, WorktreeData};
    use std::fs;
    use worktrunk::testing::TestRepo;

    fn orch_for(t: &TestRepo) -> PreviewOrchestrator {
        // No render_tx published, so fills don't notify — these tests assert on
        // the cache, not on skim repaints (see `fill_notifies_only_awaited_key`).
        PreviewOrchestrator::new(Repository::at(t.path()).unwrap(), Arc::new(OnceLock::new()))
    }

    fn dirty_worktree_item() -> (TestRepo, Arc<ListItem>) {
        let t = TestRepo::new();
        fs::write(t.path().join("README.md"), "# Project\n").unwrap();
        t.repo.run_command(&["add", "README.md"]).unwrap();
        t.repo.run_command(&["commit", "-m", "initial"]).unwrap();
        // Dirty the working tree so WorkingTree diff has content.
        fs::write(t.path().join("README.md"), "# Project\nmore\n").unwrap();

        let head = t
            .repo
            .run_command(&["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();
        let mut item = ListItem::new_branch(head, "main".to_string());
        item.kind = ItemKind::Worktree(Box::new(WorktreeData {
            path: t.path().to_path_buf(),
            ..Default::default()
        }));
        (t, Arc::new(item))
    }

    /// End-to-end: orchestrator spawns real previews, populates the cache.
    /// Regression test for the "previews never load" class of bugs — if the
    /// spawn pipeline silently fails, this catches it without needing skim.
    #[test]
    fn orchestrator_populates_cache_for_real_worktree() {
        let (t, item) = dirty_worktree_item();

        let orch = orch_for(&t);
        orch.spawn_preview(Arc::clone(&item), PreviewMode::WorkingTree, (80, 24));
        orch.spawn_preview(Arc::clone(&item), PreviewMode::Log, (80, 24));
        orch.wait_for_idle();

        let wt_key = ("main".to_string(), PreviewMode::WorkingTree);
        let log_key = ("main".to_string(), PreviewMode::Log);
        assert!(
            orch.cache.contains_key(&wt_key),
            "WorkingTree preview not cached"
        );
        assert!(orch.cache.contains_key(&log_key), "Log preview not cached");
        assert!(
            !orch.cache.get(&wt_key).unwrap().is_empty(),
            "WorkingTree preview was empty"
        );
    }

    #[test]
    fn duplicate_spawn_short_circuits() {
        let (t, item) = dirty_worktree_item();

        let orch = orch_for(&t);
        orch.spawn_preview(Arc::clone(&item), PreviewMode::WorkingTree, (80, 24));
        orch.wait_for_idle();
        let first = orch
            .cache
            .get(&("main".to_string(), PreviewMode::WorkingTree))
            .unwrap()
            .value()
            .clone();

        // Second spawn should hit `contains_key` and skip.
        orch.spawn_preview(Arc::clone(&item), PreviewMode::WorkingTree, (80, 24));
        orch.wait_for_idle();
        let second = orch
            .cache
            .get(&("main".to_string(), PreviewMode::WorkingTree))
            .unwrap()
            .value()
            .clone();
        assert_eq!(first, second);
    }

    /// `spawn_summary` delegates to the same spawn-task machinery as
    /// `spawn_preview`, but via the LLM summary path. The test uses `/bin/cat`
    /// as a fake LLM command (it echoes the prompt back), so the test stays
    /// hermetic — no real LLM is invoked, but the cache receives a Summary
    /// entry proving the task ran to completion.
    #[test]
    fn spawn_summary_populates_cache() {
        let (t, item) = dirty_worktree_item();
        let repo = Repository::at(t.path()).unwrap();

        let orch = orch_for(&t);
        orch.spawn_summary(Arc::clone(&item), "/bin/cat".to_string(), repo);
        orch.wait_for_idle();

        assert!(
            orch.cache
                .contains_key(&("main".to_string(), PreviewMode::Summary)),
            "Summary entry not cached"
        );
    }

    /// Disk-cache hit on a Log preview enqueues a background refresh that
    /// overwrites both the disk file and the in-memory DashMap. Seed the
    /// disk cache with a stale `LogCacheEntry` containing a marker —
    /// after `spawn_preview` + `wait_for_idle`, neither cache should
    /// hold the marker, because the refresh thread re-ran
    /// `compute_log_raw_and_stats` and wrote real git-log output.
    ///
    /// `wait_for_idle` covers the refresh thread's task because the
    /// producer increments `pending` before sending and the refresh
    /// thread decrements via `PendingGuard` after running.
    #[test]
    fn log_disk_hit_triggers_background_refresh() {
        let (t, item) = dirty_worktree_item();
        let repo = Repository::at(t.path()).unwrap();

        let stale = super::super::preview_cache::LogCacheEntry {
            raw_log: "STALE_MARKER\n".to_string(),
            stats: std::collections::HashMap::new(),
        };
        super::super::preview_cache::write_log(&repo, item.head(), 80, 24, &stale);

        let orch = orch_for(&t);
        orch.spawn_preview(Arc::clone(&item), PreviewMode::Log, (80, 24));
        orch.wait_for_idle();

        let disk = super::super::preview_cache::read_log(&repo, item.head(), 80, 24)
            .expect("disk cache present after refresh");
        assert!(
            !disk.raw_log.contains("STALE_MARKER"),
            "refresh should overwrite stale disk entry, got raw_log: {:?}",
            disk.raw_log
        );

        let in_memory = orch
            .cache
            .get(&("main".to_string(), PreviewMode::Log))
            .expect("in-memory entry present")
            .clone();
        assert!(
            !in_memory.contains("STALE_MARKER"),
            "refresh should overwrite stale in-memory entry, got: {in_memory:?}"
        );
    }

    /// Non-Log modes have content-addressed cache keys (BranchDiff is
    /// `(base_sha, branch_sha, w)`, UpstreamDiff similar) and no
    /// decoration drift, so a disk-cache hit on those modes must NOT
    /// enqueue a Log refresh. Seed the disk Log cache with stale content
    /// and spawn a BranchDiff preview — the disk Log cache must remain
    /// stale because the refresh thread never received a task.
    #[test]
    fn non_log_modes_do_not_trigger_log_refresh() {
        let (t, item) = dirty_worktree_item();
        let repo = Repository::at(t.path()).unwrap();

        let stale = super::super::preview_cache::LogCacheEntry {
            raw_log: "STALE_MARKER\n".to_string(),
            stats: std::collections::HashMap::new(),
        };
        super::super::preview_cache::write_log(&repo, item.head(), 80, 24, &stale);

        let orch = orch_for(&t);
        orch.spawn_preview(Arc::clone(&item), PreviewMode::BranchDiff, (80, 24));
        orch.wait_for_idle();

        let disk = super::super::preview_cache::read_log(&repo, item.head(), 80, 24)
            .expect("disk Log cache untouched");
        assert_eq!(
            disk.raw_log, "STALE_MARKER\n",
            "non-Log spawn must not trigger Log refresh"
        );
    }

    /// `spawn_compute` fills the shared cache from a closure, short-circuits a
    /// duplicate key, and refuses to cache a `None` or empty result (so a
    /// transient forge failure doesn't pin a blank pane). One test covers the
    /// belief "the generic spawn path behaves like spawn_preview's caching".
    #[test]
    fn spawn_compute_fills_caches_once_and_skips_empty() {
        let t = TestRepo::new();
        let orch = orch_for(&t);

        // A populated value lands in the cache under its key.
        orch.spawn_compute(("pr:7".to_string(), PreviewMode::Log), |_| {
            Some("commit list".to_string())
        });
        orch.wait_for_idle();
        assert_eq!(
            orch.cache
                .get(&("pr:7".to_string(), PreviewMode::Log))
                .map(|v| v.clone()),
            Some("commit list".to_string())
        );

        // A second spawn for the same key short-circuits on `contains_key`, so
        // the original value survives even though this closure would overwrite.
        orch.spawn_compute(("pr:7".to_string(), PreviewMode::Log), |_| {
            Some("REPLACED".to_string())
        });
        orch.wait_for_idle();
        assert_eq!(
            orch.cache
                .get(&("pr:7".to_string(), PreviewMode::Log))
                .map(|v| v.clone()),
            Some("commit list".to_string()),
            "duplicate key short-circuits"
        );

        // `None` (forge failure) and `Some("")` both leave the slot empty.
        orch.spawn_compute(("pr:9".to_string(), PreviewMode::Log), |_| None);
        orch.spawn_compute(("pr:8".to_string(), PreviewMode::Log), |_| {
            Some(String::new())
        });
        orch.wait_for_idle();
        assert!(
            !orch
                .cache
                .contains_key(&("pr:9".to_string(), PreviewMode::Log)),
            "None is not cached"
        );
        assert!(
            !orch
                .cache
                .contains_key(&("pr:8".to_string(), PreviewMode::Log)),
            "empty string is not cached"
        );
    }

    /// A fill injects an `Event::RunPreview` exactly when the selected row is
    /// awaiting that key, and nothing otherwise — the "surface a finished
    /// compute, but don't thrash off-screen rows" contract. Drives the notifier
    /// through a real `tokio` channel as skim's event sender so the assertion is
    /// on the injected event, not the cache.
    #[test]
    fn fill_notifies_only_awaited_key() {
        let t = TestRepo::new();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Event>(8);
        let render_tx = Arc::new(OnceLock::new());
        render_tx.set(tx).unwrap();
        let orch = PreviewOrchestrator::new(Repository::at(t.path()).unwrap(), render_tx);

        // The selected row is showing main's working-tree diff (a cache miss, so
        // it's awaiting that key).
        orch.notifier()
            .note_awaiting("main", PreviewMode::WorkingTree);

        // The awaited compute lands → skim is poked to repaint.
        orch.fill_external(
            ("main".to_string(), PreviewMode::WorkingTree),
            "diff".to_string(),
        );
        assert!(
            matches!(rx.try_recv(), Ok(Event::RunPreview)),
            "the awaited fill injects a RunPreview"
        );

        // Fills for other rows / other tabs must not poke — no preview thrash.
        orch.fill_external(
            ("feature".to_string(), PreviewMode::WorkingTree),
            "x".to_string(),
        );
        orch.fill_external(("main".to_string(), PreviewMode::Log), "y".to_string());
        assert!(
            rx.try_recv().is_err(),
            "an off-screen / other-tab fill injects nothing"
        );
    }

    #[test]
    fn cache_entries_json_format() {
        let t = TestRepo::new();
        let orch = orch_for(&t);
        orch.cache.insert(
            ("branch-a".to_string(), PreviewMode::WorkingTree),
            "x".to_string(),
        );
        orch.cache
            .insert(("branch-b".to_string(), PreviewMode::Log), "xy".to_string());
        // Structural assertion — future field additions shouldn't flake the test.
        let entries = orch.cache_entries_json();
        let entries = entries.as_array().expect("entries array");
        assert_eq!(entries.len(), 2);
        for e in entries {
            assert!(e["branch"].is_string());
            assert!(e["mode"].is_number());
            assert!(e["bytes"].is_number());
        }
    }
}
