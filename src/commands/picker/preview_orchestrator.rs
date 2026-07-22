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
//! - **Refresh (`alt-r`)** calls [`PreviewOrchestrator::refresh`] (from
//!   `PipelineFactory::spawn`, gated on `rebuild_repo`), which supersedes the
//!   prior spawn's producers, rebinds the compute repo to the rebuilt spawn's,
//!   and clears the entire in-memory cache — see *Spawn generations* below.
//!   The rebuilt inventory gives each row a current `item.head()` and the
//!   rebound repo a fresh `RepoCache` (notably the default-branch base SHA for
//!   BranchDiff), so recompute sees current state, not session-start state.
//!   The disk tiers keep an unchanged branch cheap; only genuinely changed
//!   content pays.
//!
//!   What's left unrefreshed is only the in-between: content that moves
//!   mid-session without an `alt-r` waits for the next refresh (or reopen) by
//!   design — the refresh *is* the reconciliation point.
//!
//! # Spawn generations
//!
//! A refresh doesn't wait for the previous spawn's producers — precompute
//! tasks still draining `COLLECT_POOL`, a `--prs` forge call in flight, a
//! demand request parked or mid-compute — and each of those holds a frozen
//! `Arc<ListItem>` whose `head()` the refresh may have made stale. Left
//! alone, such a producer would land its stale content in the just-cleared
//! cache, and the new spawn's own producer would then short-circuit on the
//! entry instead of recomputing. [`SpawnGeneration`] closes this: each
//! pipeline spawn mints a token (`PipelineFactory::spawn`), everything that
//! spawn starts carries it, and [`PreviewOrchestrator::fill`] — the one
//! insert path — drops a write whose token a later
//! [`PreviewOrchestrator::refresh`] superseded. `refresh` bumps the
//! generation *first*, then rebinds the compute repo to the new spawn's (so
//! recompute resolves bases from a fresh `RepoCache`, not session-start
//! state), then clears the cache. `fill` checks the token while holding the
//! key's shard write lock, which makes its check-and-insert atomic against
//! the bump-then-clear: a check that passes happened before the bump, so
//! the clear always lands after (and wipes) that insert, and a check after
//! the bump refuses the write — a stale fill cannot straddle the refresh.
//!
//! The same token gates every other cross-spawn effect, all enforcement
//! secondary to `fill`'s: a superseded row's demand is refused at the
//! channel ([`PreviewDemand::request`]) instead of re-seeding the cache
//! from its frozen item on every repaint; superseded queued tasks skip
//! their git/LLM compute rather than paying for doomed work; a superseded
//! `--prs` batch is dropped whole (row appends re-check inside the
//! `shared_items` lock — see `prs::PrsShared`); and a superseded skeleton
//! neither overwrites the session-shared row list / shortcut table nor
//! seeds summary hints (the one documented `fill` bypass). A superseded
//! handler's `maybe_spawn_comments` is inert for the same reason its fill
//! would drop: its *eviction* of the shared Comments entry would otherwise
//! delete the live spawn's fetch with no producer left to refill it.
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
//! Precompute is best-effort background work; the tab the user is *looking
//! at* is served by a third producer, the demand worker ([`PreviewDemand`]):
//! a `preview()` cache miss on a local-git tab posts the row's item to a
//! dedicated thread that computes just that key, off `COLLECT_POOL`, so the
//! selected tab costs its own compute rather than a position in the
//! precompute queue.
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
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use dashmap::DashMap;
use skim::prelude::Event;
use tokio::sync::mpsc::Sender;
use worktrunk::git::Repository;

use super::items::{PickerRow, PreviewCache, PreviewCacheKey};
use super::preview::{LOCAL_GIT_MODES, PreviewMode};
use super::preview_notify::PreviewNotifier;
use super::summary;
use crate::commands::list::collect::COLLECT_POOL;
use crate::commands::list::model::ListItem;

/// The picker's initial preview tab — `WorkingTree`, shown when the
/// picker opens. Pre-computed for every row at skeleton time so j/k
/// navigation lands on warm content without paying the 4-mode bulk cost
/// per row during the row-fill window. The remaining [`LOCAL_GIT_MODES`]
/// for items 1..N are deferred until `spawn_deferred_precompute` fires
/// (after row drain); for the landing row they fire at skeleton time so
/// tab-cycling is responsive immediately.
const INITIAL_MODE: PreviewMode = PreviewMode::WorkingTree;

struct PendingGuard(Arc<AtomicUsize>);

impl Drop for PendingGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Identity of one pipeline spawn, held by everything that spawn starts —
/// precompute tasks, `--prs` fetches, demand requests, the handler, and the
/// skim rows themselves. [`PreviewOrchestrator::refresh`] supersedes all
/// outstanding tokens by bumping the shared counter; a producer whose token
/// is no longer current has its cache write dropped at
/// [`PreviewOrchestrator::fill`], its demand refused at
/// [`PreviewDemand::request`], its queued work skipped before compute, and
/// its writes to the session-shared row list / shortcut table declined — so
/// a refresh can never be undone by a stale straggler. Minted per spawn in
/// `PipelineFactory::spawn` (see the *Spawn generations* section of the
/// module docs).
#[derive(Clone)]
pub(super) struct SpawnGeneration {
    current: Arc<AtomicUsize>,
    value: usize,
}

impl SpawnGeneration {
    /// Whether the spawn this token identifies is still the live one.
    pub(super) fn is_current(&self) -> bool {
        self.current.load(Ordering::SeqCst) == self.value
    }
}

/// A generation with no refresh lifecycle behind it — nothing ever
/// supersedes it, so it is always current. For rows built outside a picker
/// session; tests building skim rows directly use this the way they use
/// [`PreviewDemand::new`].
impl Default for SpawnGeneration {
    fn default() -> Self {
        Self {
            current: Arc::new(AtomicUsize::new(0)),
            value: 0,
        }
    }
}

/// On-demand compute channel for the preview the user is looking at *right
/// now* — a one-slot, latest-wins handoff from skim's UI thread to a
/// dedicated worker thread.
///
/// Precompute alone leaves a gap: `SkimItem::preview` never computes (it only
/// reads the in-memory cache), so a tab whose precompute hasn't landed sits on
/// its loading placeholder until the background queue reaches it — behind the
/// row pipeline, its network tail (`COLLECT_POOL` workers park on blocking
/// `gh` subprocesses, and rayon has no task priorities), and the mode-major
/// deferred tier. In a large repo with many worktrees that's several seconds
/// of queue for a tab the user is staring at. The demand worker closes the
/// gap: `preview()` routes a cache miss on a local-git tab here
/// ([`PreviewMode::is_local_git`]), and the worker computes exactly that key,
/// off `COLLECT_POOL` entirely — so the looked-at tab costs one disk-cache
/// read (~ms when previously computed) or one git command, not a queue
/// position.
///
/// One slot is the point: `preview()` only ever misses on the *selected*
/// row's current tab, so the newest request supersedes any unserved one and
/// rows skimmed past are never computed. A request whose key a racing pool
/// task already filled is dropped on the worker's `contains_key` check. A
/// simultaneous double-compute — the pool task for the same key mid-flight
/// when the demand is taken — is accepted rather than tracked: it's common
/// exactly once per spawn (the landing row's default tab at first paint,
/// before its skeleton-time precompute lands) and harmless (both producers
/// route through `fill` with identical content); an in-flight key set shared
/// across producers isn't worth that one duplicate `git diff`.
pub(super) struct PreviewDemand {
    state: Mutex<DemandState>,
    cond: Condvar,
}

/// The slot plus the close flag, under the one mutex the condvar guards.
#[derive(Default)]
struct DemandState {
    request: Option<DemandRequest>,
    closed: bool,
}

/// One demand: compute `mode` for `item` at the preview pane's `dims`, on
/// behalf of the spawn identified by `spawn_gen`. Self-contained (the row hands
/// over its own `Arc<ListItem>` and spawn token), so the worker needs no item
/// registry that could drift from the rows skim holds.
struct DemandRequest {
    item: Arc<ListItem>,
    mode: PreviewMode,
    dims: (usize, usize),
    spawn_gen: SpawnGeneration,
}

impl PreviewDemand {
    /// The channel only — the orchestrator spawns the worker that drains it.
    /// Tests building skim rows without an orchestrator use this directly:
    /// requests are recorded and never served.
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(DemandState::default()),
            cond: Condvar::new(),
        })
    }

    /// Publish the selected row's awaited preview, replacing any unserved
    /// request. Called from skim's UI thread on every `preview()` cache miss,
    /// so it must stay non-blocking (a brief uncontended lock).
    ///
    /// A request whose spawn a refresh superseded is refused: after `alt-r`
    /// clears the cache, the pre-refresh rows keep missing (and posting) on
    /// every repaint until the rebuilt skeleton replaces them, and each of
    /// those computes would be dropped at `fill` anyway — refusing at the
    /// channel keeps the worker free for the live spawn's first demand.
    pub(super) fn request(
        &self,
        item: Arc<ListItem>,
        mode: PreviewMode,
        dims: (usize, usize),
        spawn_gen: SpawnGeneration,
    ) {
        let mut state = self.state.lock().unwrap();
        if state.closed || !spawn_gen.is_current() {
            return;
        }
        state.request = Some(DemandRequest {
            item,
            mode,
            dims,
            spawn_gen,
        });
        self.cond.notify_one();
    }

    /// Close the channel: the worker drains out and releases its captures.
    /// Called when the orchestrator drops (the picker is done), so the parked
    /// thread doesn't pin the preview cache and repo until process exit.
    fn close(&self) {
        self.state.lock().unwrap().closed = true;
        self.cond.notify_one();
    }

    /// Worker side: block until a request is available and take it, or
    /// return `None` once the channel is closed (a parked request is
    /// dropped — the picker is gone, nothing would read the fill).
    fn take_blocking(&self) -> Option<DemandRequest> {
        let mut state = self.state.lock().unwrap();
        loop {
            if state.closed {
                return None;
            }
            if let Some(req) = state.request.take() {
                return Some(req);
            }
            state = self.cond.wait(state).unwrap();
        }
    }
}

pub(super) struct PreviewOrchestrator {
    pub(super) cache: PreviewCache,
    pending: Arc<AtomicUsize>,
    /// Bridges each fill to skim's event loop so a finished compute surfaces
    /// without a keystroke (see [`PreviewNotifier`]). Shared with the skim
    /// items, which record their awaited preview; every fill site here notifies.
    notifier: Arc<PreviewNotifier>,
    /// Repository used by preview compute. Bound at construction (unit tests
    /// inject a `TestRepo`-rooted `Repository` instead of relying on process
    /// CWD) and rebound by [`Self::refresh`] to each rebuilt spawn's repo, so
    /// recompute reads a current `RepoCache` rather than session-start state.
    ///
    /// Cloned out of the cell into each spawned task, so one spawn's tasks
    /// share the underlying `Arc<RepoCache>` — including the memoized
    /// comparison base that [`Repository::branch_diff_spec`] resolves from a
    /// single `for-each-ref` scan. That shared cache is how the BranchDiff
    /// preview avoids re-scanning refs per item. The lock is held only for
    /// the clone, never across a compute.
    repo: Arc<Mutex<Repository>>,
    /// The live spawn's generation; [`Self::generation`] mints tokens at this
    /// value and [`Self::refresh`] bumps it to supersede them. See
    /// [`SpawnGeneration`].
    generation: Arc<AtomicUsize>,
    /// The on-demand channel serving the selected row's awaited tab (see
    /// [`PreviewDemand`]). Handed to each worktree-backed row at
    /// construction; its worker thread is spawned once here and shared
    /// across `alt-r` spawns, like the cache.
    demand: Arc<PreviewDemand>,
}

impl PreviewOrchestrator {
    pub(super) fn new(repo: Repository, render_tx: Arc<OnceLock<Sender<Event>>>) -> Self {
        let cache: PreviewCache = Arc::new(DashMap::new());
        let pending = Arc::new(AtomicUsize::new(0));
        let notifier = Arc::new(PreviewNotifier::new(render_tx));
        let demand = PreviewDemand::new();
        let repo = Arc::new(Mutex::new(repo));
        {
            let demand = Arc::clone(&demand);
            let cache = Arc::clone(&cache);
            let notifier = Arc::clone(&notifier);
            let pending = Arc::clone(&pending);
            let repo = Arc::clone(&repo);
            // Detached worker thread: it parks on the condvar between
            // requests and exits when the orchestrator's `Drop` closes the
            // channel. Spawn failure degrades to the pre-demand behavior —
            // the tab fills when precompute reaches it.
            let _ = std::thread::Builder::new()
                .name("wt-preview-demand".into())
                .spawn(move || {
                    while let Some(req) = demand.take_blocking() {
                        // Read the repo cell per request, not once at spawn:
                        // a refresh rebinds it, and a demand from the rebuilt
                        // spawn must compute against the rebuilt repo.
                        let repo = repo.lock().unwrap().clone();
                        Self::serve_demand(&cache, &notifier, &pending, &repo, req);
                    }
                });
        }
        Self {
            cache,
            pending,
            notifier,
            repo,
            generation: Arc::new(AtomicUsize::new(0)),
            demand,
        }
    }

    /// Supersede the previous spawn's producers and rebind compute state to a
    /// rebuilt spawn's `repo`. Called from `PipelineFactory::spawn` on every
    /// `alt-r` rebuild, before the new spawn starts any producer.
    ///
    /// Order matters: the generation bump comes first, so a stale in-flight
    /// fill racing this call either lands before the clear (wiped by it) or
    /// after (dropped by [`Self::fill`]'s check) — clear-then-bump would
    /// leave a window where a stale fill lands post-clear and survives until
    /// the next refresh.
    pub(super) fn refresh(&self, repo: Repository) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        *self.repo.lock().unwrap() = repo;
        self.cache.clear();
    }

    /// Mint a token for the live spawn. Everything the spawn starts carries a
    /// clone; a later [`Self::refresh`] invalidates them all at once.
    pub(super) fn generation(&self) -> SpawnGeneration {
        SpawnGeneration {
            current: Arc::clone(&self.generation),
            value: self.generation.load(Ordering::SeqCst),
        }
    }

    /// Serve one demand request: compute the awaited preview and fill the
    /// cache. The demand worker's whole loop body, factored out so each arm
    /// is unit-testable without racing the thread.
    ///
    /// Skips a key a racing pool task already filled, and a request parked
    /// across a refresh (posted while its spawn was still current, taken
    /// after it was superseded) — computing the latter would make the live
    /// spawn's first demand queue behind a git call whose fill is doomed to
    /// drop. A panicking compute is contained to its key: on a rayon worker
    /// it would abort the process, but here it would only unwind the worker
    /// thread — and a dead worker silently regresses every later navigation
    /// to queue-wait latency for the rest of the session — so log and keep
    /// serving.
    fn serve_demand(
        cache: &PreviewCache,
        notifier: &Arc<PreviewNotifier>,
        pending: &Arc<AtomicUsize>,
        repo: &Repository,
        req: DemandRequest,
    ) {
        if !req.spawn_gen.is_current() {
            return;
        }
        let key = (req.item.branch_name().to_string(), req.mode);
        if cache.contains_key(&key) {
            return;
        }
        let (w, h) = req.dims;
        let computed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            PickerRow::compute_and_page_preview(repo, &req.item, req.mode, w, h)
        }));
        let Ok((value, log_disk_hit)) = computed else {
            tracing::debug!(
                branch = key.0,
                mode = ?key.1,
                "preview demand compute panicked; leaving the tab to precompute"
            );
            return;
        };
        Self::fill(cache, notifier, &req.spawn_gen, key, value);
        if log_disk_hit {
            Self::spawn_log_refresh(
                cache,
                notifier,
                pending,
                &req.spawn_gen,
                req.item,
                repo,
                (w, h),
            );
        }
    }

    /// The shared demand channel, handed to each worktree-backed skim row so
    /// its `preview()` can request the awaited tab on a cache miss.
    pub(super) fn demand(&self) -> &Arc<PreviewDemand> {
        &self.demand
    }

    /// The repository the live spawn computes previews against (a clone out
    /// of the rebindable cell — see the `repo` field).
    pub(super) fn repo(&self) -> Repository {
        self.repo.lock().unwrap().clone()
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
    /// to repaint it — and, symmetrically, so a producer whose spawn a refresh
    /// superseded can never re-seed the just-cleared cache with stale content
    /// (the write is dropped, and nothing notifies).
    fn fill(
        cache: &PreviewCache,
        notifier: &PreviewNotifier,
        spawn_gen: &SpawnGeneration,
        key: PreviewCacheKey,
        value: String,
    ) {
        {
            // The generation check and the insert hold the key's shard write
            // lock together (`entry` acquires it), making them atomic against
            // `refresh`'s bump-then-clear: a check that passes here happened
            // before the bump, so the clear — which starts after the bump and
            // must wait for this shard lock — always runs after the insert
            // and wipes it. A plain check-then-insert would leave a third
            // interleaving (check passes pre-bump, insert lands post-clear)
            // where the stale entry survives. Compute has already finished by
            // now, so the lock spans only the check and the insert — never a
            // subprocess — keeping skim's UI-thread reads unblocked.
            let entry = cache.entry(key.clone());
            if !spawn_gen.is_current() {
                return;
            }
            entry.insert(value);
        }
        notifier.notify_filled(&key);
    }

    /// [`Self::fill`] for callers that hold the orchestrator rather than the
    /// captured `cache` / `notifier` clones — the `--prs` comments path's
    /// synchronous "unsupported forge" pane.
    pub(super) fn fill_external(
        &self,
        spawn_gen: &SpawnGeneration,
        key: PreviewCacheKey,
        value: String,
    ) {
        Self::fill(&self.cache, &self.notifier, spawn_gen, key, value);
    }

    /// Spawn a preview compute task. Returns immediately.
    ///
    /// Idempotent on the cache key: if another task already populated it,
    /// this one short-circuits after the `contains_key` check. Compute
    /// happens outside any DashMap lock so skim's UI thread (which calls
    /// `preview()` synchronously and reads via `DashMap::get`) is never
    /// blocked on a shard write held across git/pager subprocesses.
    ///
    /// Log mode that hits the disk cache also enqueues a refresh task to
    /// recompute the embedded ref decorations before the next visit (see
    /// [`Self::spawn_log_refresh`]).
    pub(super) fn spawn_preview(
        &self,
        spawn_gen: &SpawnGeneration,
        item: Arc<ListItem>,
        mode: PreviewMode,
        dims: (usize, usize),
    ) {
        let cache = Arc::clone(&self.cache);
        let notifier = Arc::clone(&self.notifier);
        let (w, h) = dims;
        let repo = self.repo();
        let pending = Arc::clone(&self.pending);
        let spawn_gen = spawn_gen.clone();
        self.spawn_task(move || {
            // A superseded spawn's queued task is doomed — its fill would
            // drop — so skip the compute, not just the write. `fill` remains
            // the correctness gate; this (and the same check in the other
            // task bodies) only stops a refresh from paying for the prior
            // spawn's still-queued git/LLM work.
            if !spawn_gen.is_current() {
                return;
            }
            let cache_key = (item.branch_name().to_string(), mode);
            if cache.contains_key(&cache_key) {
                return;
            }
            let (value, log_disk_hit) =
                PickerRow::compute_and_page_preview(&repo, &item, mode, w, h);
            Self::fill(&cache, &notifier, &spawn_gen, cache_key, value);
            if log_disk_hit {
                Self::spawn_log_refresh(
                    &cache,
                    &notifier,
                    &pending,
                    &spawn_gen,
                    item,
                    &repo,
                    (w, h),
                );
            }
        });
    }

    /// Enqueue the background re-render that follows a Log disk-cache hit
    /// (recomputing the embedded ref decorations — see the `LogCacheEntry`
    /// docstring for why the SHA-keyed cache drifts). Lands on
    /// `COLLECT_POOL`'s FIFO injector, behind in-flight foreground
    /// precompute. Shared by the pool producer (`spawn_preview`) and the
    /// demand worker.
    fn spawn_log_refresh(
        cache: &PreviewCache,
        notifier: &Arc<PreviewNotifier>,
        pending: &Arc<AtomicUsize>,
        spawn_gen: &SpawnGeneration,
        item: Arc<ListItem>,
        repo: &Repository,
        dims: (usize, usize),
    ) {
        pending.fetch_add(1, Ordering::SeqCst);
        let guard = PendingGuard(Arc::clone(pending));
        let cache = Arc::clone(cache);
        let notifier = Arc::clone(notifier);
        let repo = repo.clone();
        let spawn_gen = spawn_gen.clone();
        COLLECT_POOL.spawn_fifo(move || {
            let _g = guard;
            if !spawn_gen.is_current() {
                return;
            }
            let (w, h) = dims;
            let rendered = PickerRow::refresh_log_preview(&repo, &item, w, h);
            // Skip empty results so a transient `git log` failure
            // doesn't poison the in-memory cache with "" and wipe
            // out the value the producer just inserted.
            if !rendered.is_empty() {
                Self::fill(
                    &cache,
                    &notifier,
                    &spawn_gen,
                    (item.branch_name().to_string(), PreviewMode::Log),
                    rendered,
                );
            }
        });
    }

    /// Spawn an LLM summary task. Returns immediately.
    pub(super) fn spawn_summary(
        &self,
        spawn_gen: &SpawnGeneration,
        item: Arc<ListItem>,
        llm_command: String,
    ) {
        let cache = Arc::clone(&self.cache);
        let notifier = Arc::clone(&self.notifier);
        let repo = self.repo();
        let spawn_gen = spawn_gen.clone();
        self.spawn_task(move || {
            if !spawn_gen.is_current() {
                return;
            }
            let summary = summary::generate_summary_for_item(&item, &llm_command, &repo);
            Self::fill(
                &cache,
                &notifier,
                &spawn_gen,
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
    pub(super) fn spawn_compute<F>(
        &self,
        spawn_gen: &SpawnGeneration,
        key: PreviewCacheKey,
        compute: F,
    ) where
        F: FnOnce(&Repository) -> Option<String> + Send + 'static,
    {
        let cache = Arc::clone(&self.cache);
        let notifier = Arc::clone(&self.notifier);
        let repo = self.repo();
        let spawn_gen = spawn_gen.clone();
        self.spawn_task(move || {
            if !spawn_gen.is_current() || cache.contains_key(&key) {
                return;
            }
            if let Some(value) = compute(&repo)
                && !value.is_empty()
            {
                Self::fill(&cache, &notifier, &spawn_gen, key, value);
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
    /// The remaining [`LOCAL_GIT_MODES`] for items 1..N and their summaries
    /// are deferred to [`Self::spawn_deferred_precompute`], which fires
    /// after the row pipeline tears down.
    pub(super) fn spawn_initial_precompute(
        &self,
        spawn_gen: &SpawnGeneration,
        items: &[Arc<ListItem>],
        preview_dims: (usize, usize),
        llm_command: Option<&str>,
    ) {
        let Some(first) = items.first() else { return };

        // First item: all modes + summary.
        for mode in LOCAL_GIT_MODES {
            self.spawn_preview(spawn_gen, Arc::clone(first), mode, preview_dims);
        }
        if let Some(llm) = llm_command {
            self.spawn_summary(spawn_gen, Arc::clone(first), llm.to_string());
        }

        // Items 1..N: default tab only. Other modes wait for drain.
        for item in items.iter().skip(1) {
            self.spawn_preview(spawn_gen, Arc::clone(item), INITIAL_MODE, preview_dims);
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
    /// [`Self::spawn_initial_precompute`]; what's left is the rest of
    /// [`LOCAL_GIT_MODES`] plus summaries.
    ///
    /// Spawn order: mode-major across previews, then summaries last —
    /// each LLM call can take seconds. Called from outside any rayon
    /// worker (the picker-collect bg thread), so submissions land on
    /// rayon's FIFO injector and workers pick previews before summaries.
    pub(super) fn spawn_deferred_precompute(
        &self,
        spawn_gen: &SpawnGeneration,
        rest: &[Arc<ListItem>],
        preview_dims: (usize, usize),
        llm_command: Option<&str>,
    ) {
        for mode in LOCAL_GIT_MODES.into_iter().filter(|m| *m != INITIAL_MODE) {
            for item in rest {
                self.spawn_preview(spawn_gen, Arc::clone(item), mode, preview_dims);
            }
        }
        if let Some(llm) = llm_command {
            for item in rest {
                self.spawn_summary(spawn_gen, Arc::clone(item), llm.to_string());
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

    /// Block until all pool-spawned tasks complete.
    ///
    /// Used by the dry-run path and tests; production never waits — tasks
    /// are fire-and-forget while skim runs. Polls at 10ms resolution; tasks
    /// typically take tens to hundreds of ms, so a condvar isn't worth the
    /// complexity.
    ///
    /// Scope: the `pending` counter covers everything spawned through
    /// [`Self::spawn_task`] and [`Self::spawn_log_refresh`]. The demand
    /// worker's own computes are outside it — they're keystroke-driven, and
    /// neither the dry-run path nor `wait_for_idle` callers drive
    /// `preview()`; a test exercising the demand path polls the cache
    /// instead (see `preview_miss_is_served_by_demand_worker`).
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

impl Drop for PreviewOrchestrator {
    /// Close the demand channel so the worker exits and drops its captures
    /// (preview cache, repo) when the picker ends — `wt switch` keeps
    /// running (hooks, the switch itself) after the picker returns, and a
    /// parked thread would otherwise pin them until process exit.
    fn drop(&mut self) {
        self.demand.close();
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
        orch.spawn_preview(
            &orch.generation(),
            Arc::clone(&item),
            PreviewMode::WorkingTree,
            (80, 24),
        );
        orch.spawn_preview(
            &orch.generation(),
            Arc::clone(&item),
            PreviewMode::Log,
            (80, 24),
        );
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
        orch.spawn_preview(
            &orch.generation(),
            Arc::clone(&item),
            PreviewMode::WorkingTree,
            (80, 24),
        );
        orch.wait_for_idle();
        let first = orch
            .cache
            .get(&("main".to_string(), PreviewMode::WorkingTree))
            .unwrap()
            .value()
            .clone();

        // Second spawn should hit `contains_key` and skip.
        orch.spawn_preview(
            &orch.generation(),
            Arc::clone(&item),
            PreviewMode::WorkingTree,
            (80, 24),
        );
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

        let orch = orch_for(&t);
        orch.spawn_summary(
            &orch.generation(),
            Arc::clone(&item),
            "/bin/cat".to_string(),
        );
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
        orch.spawn_preview(
            &orch.generation(),
            Arc::clone(&item),
            PreviewMode::Log,
            (80, 24),
        );
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
        orch.spawn_preview(
            &orch.generation(),
            Arc::clone(&item),
            PreviewMode::BranchDiff,
            (80, 24),
        );
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
        orch.spawn_compute(
            &orch.generation(),
            ("pr:7".to_string(), PreviewMode::Log),
            |_| Some("commit list".to_string()),
        );
        orch.wait_for_idle();
        assert_eq!(
            orch.cache
                .get(&("pr:7".to_string(), PreviewMode::Log))
                .map(|v| v.clone()),
            Some("commit list".to_string())
        );

        // A second spawn for the same key short-circuits on `contains_key`, so
        // the original value survives even though this closure would overwrite.
        orch.spawn_compute(
            &orch.generation(),
            ("pr:7".to_string(), PreviewMode::Log),
            |_| Some("REPLACED".to_string()),
        );
        orch.wait_for_idle();
        assert_eq!(
            orch.cache
                .get(&("pr:7".to_string(), PreviewMode::Log))
                .map(|v| v.clone()),
            Some("commit list".to_string()),
            "duplicate key short-circuits"
        );

        // `None` (forge failure) and `Some("")` both leave the slot empty.
        orch.spawn_compute(
            &orch.generation(),
            ("pr:9".to_string(), PreviewMode::Log),
            |_| None,
        );
        orch.spawn_compute(
            &orch.generation(),
            ("pr:8".to_string(), PreviewMode::Log),
            |_| Some(String::new()),
        );
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

    /// End-to-end demand path: a `preview()` cache miss on a local-git tab is
    /// computed by the demand worker and served — without any precompute
    /// spawn touching the pool. Pins the fix for the "navigated-to tab waits
    /// behind the whole precompute queue" latency: the placeholder used to
    /// sit until the deferred tier happened to reach the key.
    #[test]
    fn preview_miss_is_served_by_demand_worker() {
        use super::super::items::{LocalCheckout, LocalContent, PickerRow};
        use skim::prelude::{PreviewContext, SkimItem};
        use std::sync::Mutex;
        use std::sync::atomic::AtomicBool;

        let (t, item) = dirty_worktree_item();
        let orch = orch_for(&t);

        let row = PickerRow {
            search_base: String::new(),
            gutter: '@',
            rendered: Arc::new(Mutex::new(String::new())),
            branch_name: "main".to_string(),
            output_token: "main".to_string(),
            preview_cache: Arc::clone(&orch.cache),
            pr_status: Arc::new(Mutex::new(None)),
            notifier: Arc::clone(orch.notifier()),
            local: Some(LocalCheckout {
                item: Arc::clone(&item),
                demand: Arc::clone(orch.demand()),
                spawn_gen: orch.generation(),
                has_upstream: false,
                summaries_enabled: false,
                local_content: Arc::new(Mutex::new(LocalContent::default())),
                morphed: Arc::new(AtomicBool::new(false)),
            }),
        };

        // The picker opens on WorkingTree — the process-global default tab,
        // which this test deliberately leaves untouched (other tests read
        // it). This render misses and posts the demand. No assertion on the
        // returned placeholder: the worker races the render's own cache read
        // by design, so on a stalled thread the first render could already
        // carry the diff.
        let ctx = PreviewContext {
            query: "",
            cmd_query: "",
            width: 80,
            height: 24,
            current_index: 0,
            current_selection: "",
            selected_indices: &[],
            selections: &[],
        };
        drop(row.preview(ctx));

        // The demand worker fills the key with the real diff. Bounded poll so
        // a dead worker fails instead of hanging the suite.
        let key = ("main".to_string(), PreviewMode::WorkingTree);
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while !orch.cache.contains_key(&key) {
            assert!(
                std::time::Instant::now() < deadline,
                "demand worker never filled the awaited key"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let served = orch.cache.get(&key).unwrap().clone();
        assert!(
            served.contains("README"),
            "demand-computed working-tree diff served: {served:?}"
        );
    }

    /// A key a racing pool task already filled is not recomputed: serve_demand
    /// (the worker's loop body, driven directly so nothing races) returns
    /// before compute and the existing value survives.
    #[test]
    fn serve_demand_skips_already_filled_key() {
        let (t, item) = dirty_worktree_item();
        let orch = orch_for(&t);
        let key = ("main".to_string(), PreviewMode::WorkingTree);
        orch.fill_external(&orch.generation(), key.clone(), "already here".to_string());

        PreviewOrchestrator::serve_demand(
            &orch.cache,
            orch.notifier(),
            &orch.pending,
            &orch.repo(),
            DemandRequest {
                item,
                mode: PreviewMode::WorkingTree,
                dims: (80, 24),
                spawn_gen: orch.generation(),
            },
        );

        assert_eq!(
            orch.cache.get(&key).unwrap().clone(),
            "already here",
            "a filled key is skipped, not recomputed"
        );
    }

    /// A panicking compute must not unwind out of serve_demand — on the real
    /// worker thread that would silently kill demand serving for the rest of
    /// the session. `Pr` mode's compute arm is `unreachable!`, a deterministic
    /// panic source; the panic is contained and the key stays unfilled.
    #[test]
    fn serve_demand_contains_a_panicking_compute() {
        let (t, item) = dirty_worktree_item();
        let orch = orch_for(&t);

        PreviewOrchestrator::serve_demand(
            &orch.cache,
            orch.notifier(),
            &orch.pending,
            &orch.repo(),
            DemandRequest {
                item,
                mode: PreviewMode::Pr,
                dims: (80, 24),
                spawn_gen: orch.generation(),
            },
        );

        assert!(
            !orch
                .cache
                .contains_key(&("main".to_string(), PreviewMode::Pr)),
            "a panicked compute fills nothing"
        );
    }

    /// A Log disk hit served on demand enqueues the same decoration refresh
    /// the pool path does (see `log_disk_hit_triggers_background_refresh`):
    /// the stale seeded entry is overwritten once the pending-tracked refresh
    /// drains.
    #[test]
    fn serve_demand_log_disk_hit_enqueues_refresh() {
        let (t, item) = dirty_worktree_item();
        let repo = Repository::at(t.path()).unwrap();

        let stale = super::super::preview_cache::LogCacheEntry {
            raw_log: "STALE_MARKER\n".to_string(),
            stats: std::collections::HashMap::new(),
        };
        super::super::preview_cache::write_log(&repo, item.head(), 80, 24, &stale);

        let orch = orch_for(&t);
        PreviewOrchestrator::serve_demand(
            &orch.cache,
            orch.notifier(),
            &orch.pending,
            &orch.repo(),
            DemandRequest {
                item,
                mode: PreviewMode::Log,
                dims: (80, 24),
                spawn_gen: orch.generation(),
            },
        );
        orch.wait_for_idle();

        let in_memory = orch
            .cache
            .get(&("main".to_string(), PreviewMode::Log))
            .expect("in-memory entry present")
            .clone();
        assert!(
            !in_memory.contains("STALE_MARKER"),
            "the demand-served disk hit was refreshed: {in_memory:?}"
        );
    }

    /// A refresh supersedes the prior spawn's producers: every producer path
    /// carrying the pre-refresh token — pool preview/summary/compute tasks,
    /// the log-refresh follow-up, and the `fill` choke point itself — leaves
    /// the cleared cache untouched, while the rebuilt spawn's token fills
    /// normally. Pins the fix for the stale-drain race the module docs'
    /// *Spawn generations* section describes.
    #[test]
    fn refresh_supersedes_stale_spawn_fills() {
        let (t, item) = dirty_worktree_item();
        let orch = orch_for(&t);
        let stale = orch.generation();

        orch.refresh(Repository::at(t.path()).unwrap());

        // Queued task bodies drop the doomed work before computing.
        orch.spawn_preview(
            &stale,
            Arc::clone(&item),
            PreviewMode::WorkingTree,
            (80, 24),
        );
        orch.spawn_summary(&stale, Arc::clone(&item), "/bin/cat".to_string());
        orch.spawn_compute(&stale, ("pr:1".to_string(), PreviewMode::Log), |_| {
            Some("stale".to_string())
        });
        PreviewOrchestrator::spawn_log_refresh(
            &orch.cache,
            orch.notifier(),
            &orch.pending,
            &stale,
            Arc::clone(&item),
            &orch.repo(),
            (80, 24),
        );
        orch.wait_for_idle();
        // The choke point drops a stale write even when a task slipped past
        // the early checks (queued while current, filling after the bump).
        PreviewOrchestrator::fill(
            &orch.cache,
            orch.notifier(),
            &stale,
            ("main".to_string(), PreviewMode::WorkingTree),
            "stale".to_string(),
        );
        assert!(
            orch.cache.is_empty(),
            "a superseded spawn must not re-seed the cleared cache"
        );

        orch.spawn_preview(
            &orch.generation(),
            Arc::clone(&item),
            PreviewMode::WorkingTree,
            (80, 24),
        );
        orch.wait_for_idle();
        assert!(
            orch.cache
                .contains_key(&("main".to_string(), PreviewMode::WorkingTree)),
            "the live spawn's fill lands"
        );
    }

    /// `refresh` rebinds the compute repo, so post-refresh producers (and the
    /// demand worker's per-request read) resolve `RepoCache` values — notably
    /// the BranchDiff comparison base — from the rebuilt spawn's repo, not
    /// session-start state.
    #[test]
    fn refresh_rebinds_compute_repo() {
        let t = TestRepo::new();
        let orch = orch_for(&t);
        let t2 = TestRepo::new();
        let repo2 = Repository::at(t2.path()).unwrap();

        orch.refresh(repo2.clone());

        assert_eq!(
            orch.repo().discovery_path(),
            repo2.discovery_path(),
            "repo cell rebound to the rebuilt spawn's repo"
        );
    }

    /// A request from a row a refresh superseded is refused at the channel
    /// (its compute would drop at `fill` anyway); the live spawn's request
    /// parks. Driven on a worker-less channel so the parked/empty slot can be
    /// asserted without racing a serve.
    #[test]
    fn stale_generation_request_is_refused() {
        let (t, item) = dirty_worktree_item();
        let orch = orch_for(&t);
        let demand = PreviewDemand::new();
        let stale = orch.generation();

        orch.refresh(Repository::at(t.path()).unwrap());

        demand.request(Arc::clone(&item), PreviewMode::WorkingTree, (80, 24), stale);
        assert!(
            demand.state.lock().unwrap().request.is_none(),
            "a superseded row's request is refused"
        );

        demand.request(item, PreviewMode::WorkingTree, (80, 24), orch.generation());
        assert!(
            demand.state.lock().unwrap().request.is_some(),
            "the live spawn's request parks"
        );
    }

    /// The one stale demand that can outlive `request`'s check — parked while
    /// its spawn was current, taken after a refresh — is dropped before its
    /// compute, so the live spawn's first demand never queues behind it and
    /// the cleared cache stays untouched.
    #[test]
    fn parked_request_across_refresh_is_dropped() {
        let (t, item) = dirty_worktree_item();
        let orch = orch_for(&t);
        let stale = orch.generation();

        orch.refresh(Repository::at(t.path()).unwrap());
        PreviewOrchestrator::serve_demand(
            &orch.cache,
            orch.notifier(),
            &orch.pending,
            &orch.repo(),
            DemandRequest {
                item,
                mode: PreviewMode::WorkingTree,
                dims: (80, 24),
                spawn_gen: stale,
            },
        );

        assert!(
            orch.cache.is_empty(),
            "a request parked across a refresh computes but its fill drops"
        );
    }

    /// Rows hold the demand channel beyond the orchestrator's life (skim can
    /// still call `preview()` while teardown races), so a request after
    /// `Drop` closed the channel must be inert, not parked forever.
    #[test]
    fn request_after_orchestrator_drop_is_inert() {
        let (t, item) = dirty_worktree_item();
        let orch = orch_for(&t);
        let demand = Arc::clone(orch.demand());
        let cache = Arc::clone(&orch.cache);
        let spawn_gen = orch.generation();
        drop(orch);

        demand.request(item, PreviewMode::WorkingTree, (80, 24), spawn_gen);

        assert!(
            demand.state.lock().unwrap().request.is_none(),
            "a closed channel records nothing"
        );
        assert!(cache.is_empty(), "nothing fills after close");
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
            &orch.generation(),
            ("main".to_string(), PreviewMode::WorkingTree),
            "diff".to_string(),
        );
        assert!(
            matches!(rx.try_recv(), Ok(Event::RunPreview)),
            "the awaited fill injects a RunPreview"
        );

        // Fills for other rows / other tabs must not poke — no preview thrash.
        orch.fill_external(
            &orch.generation(),
            ("feature".to_string(), PreviewMode::WorkingTree),
            "x".to_string(),
        );
        orch.fill_external(
            &orch.generation(),
            ("main".to_string(), PreviewMode::Log),
            "y".to_string(),
        );
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
