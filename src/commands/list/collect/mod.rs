//! Worktree data collection with parallelized git operations.
//!
//! This module provides an efficient approach to collecting worktree data:
//! - All tasks flattened into a single Rayon work queue
//! - Network tasks (CI, URL) sorted to run last
//! - Progressive updates via channels (update UI as each task completes)
//!
//! ## Skeleton Performance
//!
//! The skeleton (placeholder table with loading indicators) must render as fast as possible
//! to give users immediate feedback. Every git command before skeleton adds latency.
//!
//! ### Forks on the Critical Path
//!
//! A steady-state run reaches the skeleton through five `git` subprocess
//! forks (six in repos with `extensions.worktreeConfig=true` — see #3
//! below). Fork *count* is O(1) — independent of worktree or branch
//! count — because each batches as much as it can. Fork *work* scales with
//! N (refs read, SHAs resolved); on a fast Linux system N=40 lands
//! ~30–80 ms.
//!
//! | # | Command | Source | Role |
//! |---|---------|--------|------|
//! | 1 | `git rev-parse --git-common-dir --is-inside-work-tree --show-toplevel --git-dir --symbolic-full-name HEAD` | [`Repository::prewarm`] (`prewarm_rev_parse`) | Five facts in one fork: shared `.git`, in-worktree gate, worktree root, per-worktree `.git/worktrees/<name>`, current branch. Populates process-global caches. Parallel with #2 at process startup. |
//! | 2 | `git config --list -z` (cwd = discovery path) | [`Repository::prewarm`] (`prewarm_git_config`) | Whole merged config (system + global + local) in one shot, NUL-delimited so values containing `\n` or `=` parse unambiguously. Stashed in `GIT_CONFIG_PRELOAD` keyed by discovery path; every later `config_last("…")` reads from memory. Parallel with #1. |
//! | 3 | `git config --list -z` (cwd = `git_common_dir`) | [`Repository::all_config`] | **Conditional.** [`Repository::at`] consumes #2's preload into `cache.all_config`, so `all_config()` is a memory hit on a normal repo. This fork only fires when `prewarm_git_config` declined the preload because `extensions.worktreeConfig=true` — there `--list` from a linked worktree misses the main-worktree `config.worktree` overrides (most importantly `core.bare = true` for the `myproject/.git + sibling worktrees` layout), so `all_config` re-forks from the common dir to see the full merged set. See `prewarm_git_config` for the full reasoning. |
//! | 4 | `git worktree list --porcelain` | [`Repository::list_worktrees`] | Path, HEAD SHA, branch, and flags per worktree — the row source for the skeleton. The picker prelude triggers this once for `num_items_estimate`; collect's rayon scope then hits the cache. |
//! | 5 | `git for-each-ref --format=… refs/heads/` | [`Repository::local_branches`] inside collect's `rayon::scope` | Local branch tips (name, SHA, committer date, upstream) for branch-only rows (`branches=true`) and for the stale-default-branch check. `remotes=true` adds a sibling `refs/remotes/` fork. The scope joins; this fork gates the skeleton. |
//! | 6 | `git log --no-walk --no-show-signature --format=… SHA₁ … SHA_N` | collect, after the scope | Batched commit metadata for every worktree HEAD + branch tip. See breakdown below. |
//!
//! Things that *look* like forks but aren't, on the steady-state path:
//! `git config worktrunk.default-branch`, `git config --bool core.bare`,
//! `git config remote.*.url`, [`Repository::is_bare`] — each is a
//! `config_last("…")` lookup served from #2's in-memory map (or from #3
//! when the conditional fork above did fire). They appear in the rayon
//! scope as logical fetches but no subprocess fires for them warm. The
//! same is true for [`Repository::default_branch`] once
//! `worktrunk.default-branch` is cached (the steady-state case).
//!
//! Things that fire **once per repo, ever**:
//!
//! - [`Repository::default_branch`] falling through to
//!   `git ls-remote --symref <remote> HEAD` when neither
//!   `worktrunk.default-branch` nor `refs/remotes/<remote>/HEAD` is set.
//!   100 ms – 2 s on the wire. The result persists to
//!   `worktrunk.default-branch` so subsequent runs are a cache hit. This is
//!   worktrunk's one accepted wire-path exception — see CLAUDE.md →
//!   "Network Access".
//!
//! ### #6 — the batched commit-details fork
//!
//! ```text
//! git log --no-walk --no-show-signature \
//!   --format=%H%x00%h%x00%ct%x00%s \
//!   SHA₁ SHA₂ … SHA_N
//! ```
//!
//! - `--no-walk` — don't traverse history. Without it, `git log SHA₁ SHA₂`
//!   walks the ancestry of each starting point and prints thousands of
//!   commits. With it, one record per named SHA. Turns this from
//!   O(history) into O(N refs).
//! - `--no-show-signature` — skip GPG verification. If `gpg.program` is set
//!   and any of these commits are signed, the default forks `gpg` per
//!   commit; disabled here.
//! - `--format=%H%x00%h%x00%ct%x00%s` — four fields per commit,
//!   NUL-separated because subjects can contain anything except NUL:
//!   - `%H` full SHA, `%h` abbreviated SHA, `%ct` committer date (Unix
//!     epoch), `%s` subject (first line of message).
//!
//! SHAs come from #4 (worktree HEADs) ∪ #5 (branch tips), deduplicated.
//! Argv length scales with N — Linux `ARG_MAX` (~128 KB) bounds the
//! unchunked form at roughly 3,000 SHAs.
//!
//! What each field feeds:
//! - `%ct` — sort order on the skeleton. This is the *only* reason #6 is
//!   pre-skeleton; the skeleton can't pick row order without it.
//! - `%ct` again, post-skeleton — Age column ("3 hours ago").
//! - `%s` post-skeleton — Message column.
//! - `%h` post-skeleton — abbreviated-SHA cell.
//!
//! Subjects and abbreviated SHAs ride along for free: git resolves each
//! commit object to read its timestamp anyway, so the extra bytes add no
//! measurable latency to the round trip. Without this batch you'd be
//! forking `git log -1` per SHA later — same data, N forks instead of one.
//! The full `(timestamp, subject)` map is handed to the post-skeleton loop
//! that populates `ListItem.commit` directly.
//!
//! When the batch fails (e.g., a listed SHA was deleted mid-run), the
//! failure is surfaced once and Age/Message cells render placeholders for
//! that run.
//!
//! ### Non-git work on the path
//!
//! Negligible latency, but worth naming:
//! - Path canonicalization — detect current worktree.
//! - Project config file read (`.config/wt.toml`) via
//!   `ProjectConfig::load`. [`Repository::url_template`] reads `list.url`
//!   from this file — TOML I/O, not git config, and no subprocess.
//! - Config resolution — merge project-specific settings (uses cached
//!   project identifier).
//! - CI column width hint — one read of `.git/wt/cache/pr-number/max.json`
//!   (skipped when the CI task is skipped); the skeleton can't size the CI
//!   column without it.
//!
//! ### First-run behavior
//!
//! On a fresh clone with `worktrunk.default-branch` unset,
//! [`Repository::default_branch`] adds one extra step on the path: try
//! `refs/remotes/<remote>/HEAD` locally, fall through to
//! `git ls-remote --symref` (the wire-path exception above), fall further
//! back to local inference (`init.defaultBranch`, common branch names).
//! The result is cached to `worktrunk.default-branch` so every subsequent
//! run is a memory hit served from #2.
//!
//! ### Post-Skeleton Operations
//!
//! After the skeleton renders, remaining setup runs before spawning the worker thread.
//! These operations are parallelized using `rayon::scope` with single-level parallelism:
//!
//! ```text
//! Skeleton render
//! ├─ is_builtin_fsmonitor_enabled()             (5ms, sequential - gate)
//! ├─ rayon::scope(
//! │    ├─ switch_previous()                     (5ms)
//! │    ├─ integration_targets()                 (10ms)
//! │    ├─ start_fsmonitor_daemon × N worktrees  (6ms each, all parallel)
//! │  )                                          // ~10ms total (max of all spawns)
//! ├─ populate ListItem.commit from cache        (cache-hit lookups, sub-ms)
//! Worker thread spawns
//! └─ paint Age/Message columns                  (workers already running)
//! ```
//!
//! **Why the Age/Message paint:** those two columns carry no task — their
//! data is the `ListItem.commit` populated above — so without an explicit
//! repaint they'd sit on the skeleton placeholder until the row's first *task*
//! result happened to redraw it, lagging behind the slower task-driven columns
//! (and a cache-warm Summary preview). The paint runs *after* the worker pool
//! is spawned — so the slow git subprocesses (the long pole) aren't delayed by
//! it — but *before* the drain renders any result, so Age/Message still reach
//! the screen ahead of every task-driven column. Reading `all_items` here is
//! race-free: the worker thread only sends results through the channel, and the
//! drain (the sole `all_items` mutator) hasn't started. `render_skeleton_row`
//! fills Age/Message from `item.commit` while every task column keeps its
//! placeholder.
//!
//! **Why fsmonitor check is sequential:** It gates whether daemon starts are needed.
//! The check is fast (~5ms) and must complete before we know which spawns to add.
//!
//! **Why fsmonitor starts are in the parallel scope:** The `git fsmonitor--daemon start`
//! command returns quickly after signaling the daemon. By the time the worker thread
//! starts executing `git status` commands, daemons have had time to initialize.
//!
//! **Stale default branch warning:** The post-skeleton `warn_stale_default`
//! check compares `default_branch()` (resolved pre-skeleton) against the
//! local branch list — reusing the list fetched for `--branches`, otherwise
//! adding one `for-each-ref` fork when the persisted default isn't a worktree branch.
//!
//! When adding new features, ask: "Can this be computed after skeleton?" If yes, defer it.
//! The skeleton shows `·` placeholder for gutter symbols, filled in when data loads.
//!
//! ### Measured Phase Timings
//!
//! Representative medians on the worktrunk dev repo (7 worktrees, 6
//! branches, warm caches, release build, `--progressive` forced so the
//! progressive-table path fires even with stdout piped).
//!
//! | Phase | median | cmds |
//! |-------|-------:|-----:|
//! | `List collect started → Skeleton rendered` (pre-skeleton) | ~60ms | 23 |
//! | `Skeleton rendered → Spawning worker thread` (rayon::scope + work-item setup) | ~41ms | 7 |
//! | `Spawning worker thread → Parallel execution started` | <100µs | 0 |
//! | `Parallel execution started → First result received` | <100µs | 0 |
//! | `First result received → All results drained` (parallel work) | ~436ms | 154 |
//! | `All results drained → List collect complete` (final render) | ~344µs | 0 |
//! | Wall clock | ~549ms | — |
//!
//! The 23-command pre-skeleton count is well above the five-to-six
//! critical-path forks documented above — worth an audit. Most of the extras
//! come from per-worktree probes that creep into the phase.
//!
//! Reproduce end-to-end via
//! `cargo bench --bench time_to_first_output -- list`; for a per-phase
//! breakdown, capture a trace and run the phase-duration SQL query from
//! `benches/CLAUDE.md`:
//!
//! ```bash
//! RUST_LOG=debug ./target/release/wt -C <repo> list --progressive \
//!   2> >(cargo run -p wt-perf --release -q -- trace > trace.json)
//! ```
//!
//! ## Unified Collection Architecture
//!
//! Progressive and buffered modes use the same collection and rendering code.
//! The only difference is whether intermediate updates are shown during collection:
//! - Progressive: renders a skeleton table and updates rows/footer as data arrives (TTY),
//!   or renders once at the end (non-TTY)
//! - Buffered: collects silently, then renders the final table
//!
//! Both modes render the final table in `collect()`, ensuring a single canonical rendering path.
//!
//! **Flat parallelism**: All tasks (for all worktrees and branches) are collected into a single
//! work queue and processed via Rayon's thread pool. This avoids nested parallelism and keeps
//! utilization high regardless of worktree count (pool size is set at startup; default is 2x CPU
//! cores unless `RAYON_NUM_THREADS` is set).
//!
//! **Task ordering**: Work items are sorted so local git operations run first, network tasks
//! (CI status, URL health checks) run last. This ensures the table fills in quickly with local
//! data while slower network requests complete in the background.
//!
//! ## Caching
//!
//! Sibling caches live under `.git/wt/cache/`. Each uses a different key scheme because
//! the underlying operations differ in what their output depends on. All three share
//! [`worktrunk::cache`] for the filesystem mechanics (read, write, clear, count) so
//! there's one implementation of the torn-write semantics and error policy across
//! every kind.
//!
//! | Directory | Module | Key | Staleness |
//! |-----------|--------|-----|-----------|
//! | `merge-tree-conflicts/` | `git::repository::sha_cache` | `{sha1}-{sha2}.json` (sorted) | Never — content-addressed |
//! | `merge-add-probe/` | `git::repository::sha_cache` | `{branch_sha}-{target_sha}.json` | Never — content-addressed |
//! | `is-ancestor/` | `git::repository::sha_cache` | `{base_sha}-{head_sha}.json` | Never — content-addressed |
//! | `has-added-changes/` | `git::repository::sha_cache` | `{branch_sha}-{target_sha}.json` | Never — content-addressed |
//! | `diff-stats/` | `git::repository::sha_cache` | `{base_sha}-{head_sha}.json` | Never — content-addressed |
//! | `ahead-behind/` | `git::repository::sha_cache` | `{base_sha}-{head_sha}.json` | Never — content-addressed |
//! | `ci-status/` | `commands::list::ci_status::cache` | `{branch}.json` | TTL 30–60s + HEAD SHA check |
//! | `summary/{branch}/` | `summary` | `{diff_hash}.json` | Miss if no file exists for the current hash; siblings pruned on write |
//!
//! ### Key schemes
//!
//! - **SHA-pair**: pure function of two commit SHAs. Never stale, no TTL, no invalidation.
//!   Used by all `sha_cache` kinds (merge-tree conflicts, merge-add probes, ancestry
//!   checks, file-change probes, diff stats, ahead/behind counts).
//! - **Branch + TTL + HEAD**: external mutable state (CI API, remote refs). TTL bounds
//!   staleness; the HEAD check invalidates early when the branch moves.
//! - **Branch + content-addressed hash in filename**: content hash (SHA-256
//!   prefix of the combined diff) lives in the filename, so a cache hit is
//!   "file exists for this hash". Prune-on-write removes stale sibling hashes
//!   for the branch, keeping the cache bounded at ~1 entry per branch without
//!   needing an LRU sweep.
//!
//! ### Which tasks hit which cache
//!
//! | Task | Cache |
//! |------|-------|
//! | `MergeTreeConflicts` | `sha_cache` (merge-tree-conflicts) |
//! | `WorkingTreeConflicts` | `sha_cache` (merge-tree-conflicts, tree-SHA keyed) |
//! | `WouldMergeAdd` | `sha_cache` (merge-add-probe) |
//! | `IsAncestor` | `sha_cache` (is-ancestor) |
//! | `HasFileChanges` | `sha_cache` (has-added-changes) |
//! | `BranchDiff` | `sha_cache` (diff-stats, skipped when sparse checkout is active) |
//! | `AheadBehind`, `Upstream` | `sha_cache` (ahead-behind); on a cold cache both columns are pre-filled from `for-each-ref %(ahead-behind:SHA)` walks — one against the default branch (`main↕`, in `RefSnapshot::capture_ahead_behind`) and one per unique upstream SHA (`Remote⇅`, in `Repository::prime_upstream_ahead_behind_cache`) |
//! | `CiStatus` | `ci_status::cache` |
//! | `SummaryGenerate` | `summary` |
//!
//! Every other task re-runs on each invocation.
//!
//! ### Already optimized (not cache candidates)
//!
//! - `CommittedTreesMatch` — single `git rev-parse` resolving both tree SHAs (~1ms)
//!
//! ### Cached via tree SHA
//!
//! `WorkingTreeConflicts` uses `git write-tree` to snapshot the index as a tree SHA,
//! then checks for merge conflicts via `has_merge_conflicts_by_tree_with_base_sha`. The tree SHA is
//! content-addressed and stable — identical index state produces the same SHA.
//!
//! When there are unstaged modifications or untracked files, the task copies the
//! index to a temp file, runs `git add -A` to stage all working tree content,
//! then `write-tree`.
//!
//! The cache key is `(base_commit_sha, branch_head_sha+tree_sha)`. The branch HEAD
//! SHA captures the merge-base dependency. On cache miss, `has_merge_conflicts_by_tree_with_base_sha`
//! creates an ephemeral commit via `git commit-tree` for merge-tree; on cache hit,
//! no commit is created. This makes the cache-hit path a single `git write-tree`
//! (~15ms) instead of the previous `git stash create` (~50-265ms).
//!
//! ### Fundamentally uncacheable
//!
//! Some task outputs depend on state outside the commit graph:
//!
//! - `WorkingTreeDiff` — uncommitted changes and index state
//! - `GitOperation` — presence of `.git/rebase-merge`, `.git/rebase-apply`, or `MERGE_HEAD`
//! - `UserMarker` — local git config value
//! - `UrlStatus` — TCP connect to a local dev server port; real-time by nature
//!
//! All but `UrlStatus` are cheap enough that caching would not pay back. `UrlStatus` is
//! bounded at 50ms per item; a stale "active" result when the server just died is worse
//! than the probe cost.

mod execution;
mod results;
mod tasks;
mod types;

use anyhow::Context;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::LazyLock;

use anstyle::Style;
use color_print::cformat;
use crossbeam_channel as chan;
use dunce::canonicalize;
use once_cell::sync::OnceCell;
use rayon::prelude::*;
use worktrunk::git::{ErrorExt, LocalBranch, Repository, WorktreeInfo};
use worktrunk::styling::{
    INFO_SYMBOL, eprintln, format_with_gutter, hint_message, warning_message,
};

use crate::commands::is_worktree_at_expected_path;

use super::model::{CommitDetails, ItemKind, ListItem, StatusSymbols, WorktreeData};
use super::progressive::RenderTarget;
use super::progressive_table::ProgressiveTable;

// Re-exports for sibling modules (columns.rs, render.rs, layout.rs)
pub(crate) use tasks::parse_port_from_url;
pub(crate) use types::TaskKind;

// Internal imports
pub(crate) use execution::ExpectedResults;
use execution::{work_items_for_branch, work_items_for_worktree};
use results::drain_results;
use types::DrainOutcome;
use types::{MissingResult, TaskError, TaskResult};

/// Dedicated rayon pool for git-heavy worktree collection and preview
/// pre-compute, kept off the global pool.
///
/// The picker (skim) runs its per-keystroke fuzzy matcher and result sort on
/// the **global** rayon pool. Worktrunk's collection floods that same pool with
/// blocking git subprocess tasks (status, diff, rev-list, merge-base…), one
/// batch per worktree, plus the preview orchestrator's per-mode `git diff` /
/// `git log` tasks. With only `2× CPU` global workers, all blocked on
/// subprocesses, skim's matcher queues behind the flood and the picker freezes
/// for seconds on the first keystroke, scaling with worktree count.
///
/// Routing collection and preview work through this pool leaves the global pool
/// free for skim. Same isolation pattern as `copy::COPY_POOL` and
/// `remove_dir::REMOVE_POOL`, but sized like the global pool
/// ([`collect_pool_num_threads`], i.e. `2× CPU`, honoring `RAYON_NUM_THREADS`)
/// so collection throughput is unchanged. The goal is separation, not a
/// smaller pool.
pub(crate) static COLLECT_POOL: LazyLock<rayon::ThreadPool> = LazyLock::new(|| {
    let num_threads = collect_pool_num_threads(std::env::var_os("RAYON_NUM_THREADS").is_some());
    rayon::ThreadPoolBuilder::new()
        .num_threads(num_threads)
        .build()
        .expect("failed to build collect thread pool")
});

/// The `num_threads` argument for [`COLLECT_POOL`]. When `RAYON_NUM_THREADS`
/// is set, return 0 so Rayon reads and validates the env var itself.
/// Otherwise return [`crate::rayon_thread_count`] (`2× CPU`), since Rayon's
/// own default is `1× CPU`. Takes the env presence as a parameter so both
/// branches are unit-testable without mutating process-global environment.
fn collect_pool_num_threads(rayon_num_threads_set: bool) -> usize {
    if rayon_num_threads_set {
        0
    } else {
        crate::rayon_thread_count()
    }
}

struct TableRenderPlan {
    progressive_table: Option<ProgressiveTable>,
    header: String,
    rows: Vec<String>,
    summary: String,
}

impl TableRenderPlan {
    fn render(mut self) -> anyhow::Result<bool> {
        if std::env::var_os("WORKTRUNK_FIRST_OUTPUT").is_some() {
            // `progressive_table` is always `None` here: `collect()` early-exits
            // for `WORKTRUNK_FIRST_OUTPUT` whenever progressive rendering is on
            // (`show_progress || progressive_handler.is_some()`), so this render
            // path runs only in buffered mode.
            print_first_buffered_line(&self.header)?;
            return Ok(true);
        }

        if let Some(mut table) = self.progressive_table.take() {
            table.finalize(self.rows, self.summary)?;
        } else {
            print_buffered_table(&self.header, &self.rows, &self.summary);
        }
        Ok(false)
    }
}

fn print_first_buffered_line(header: &str) -> anyhow::Result<()> {
    use std::io::Write as _;

    let mut stdout = std::io::stdout();
    writeln!(stdout, "{header}")?;
    stdout.flush()?;
    Ok(())
}

fn print_buffered_table(header: &str, rows: &[String], summary: &str) {
    println!("{header}");
    for row in rows {
        println!("{row}");
    }
    println!();
    println!("{summary}");
}

/// Options for controlling what data to collect.
///
/// This is operation parameters for a single `wt list` invocation, not a cache.
/// For cached repo data, see Repository's global cache.
#[derive(Clone)]
pub struct CollectOptions {
    /// The background tasks to run, derived from the columns that will render
    /// (`columns::required_tasks_for_render`). `collect` plans this once from the
    /// `[list] columns` selection and the gates; nothing hand-writes a task set.
    ///
    /// This drives both work-item generation (the spawn loops in
    /// `work_items_for_worktree` / `work_items_for_branch`) and column visibility
    /// (the layout filter calls `ColumnKind::renders_given_run`). A task runs iff
    /// some rendered column needs it, so an unrendered column's tasks never run.
    ///
    /// There is no blanket default: `collect` plans this from the `[list] columns`
    /// selection, and single-item callers (statusline) declare their columns via
    /// [`CollectOptions::for_columns`]. A caller states which columns it renders;
    /// the tasks follow.
    pub tasks: std::collections::HashSet<TaskKind>,

    /// URL template from project config (e.g., "http://localhost:{{ branch | hash_port }}").
    /// Expanded per-item in task spawning (post-skeleton) to minimize time-to-skeleton.
    pub url_template: Option<String>,

    /// LLM command for summary generation (from commit.generation config).
    /// None if not configured — SummaryGenerate task will be skipped.
    pub llm_command: Option<String>,

    /// Default branch resolved for this list invocation. `None` when unset
    /// or when the persisted value was stale (branch deleted externally).
    /// Tasks read this through `TaskContext::default_branch` so a stale
    /// persisted value degrades silently (empty cells) here rather than
    /// emitting a cascade of "ambiguous argument" errors from every task.
    pub default_branch: Option<String>,
    /// Integration targets resolved for this list invocation. The diverged
    /// case carries both local and upstream so per-branch tasks can OR
    /// over them, matching `Repository::integration_reason`. `None` when
    /// the default branch is unset or stale.
    pub integration_targets: Option<worktrunk::git::IntegrationTargets>,

    /// Captured ref state for this list invocation.
    ///
    /// Built once during the pre-skeleton phase (or during single-item
    /// population) and shared (cheaply, behind `Arc`) into every task.
    /// Tasks resolve target ref names to commit SHAs through this snapshot,
    /// then call the `_by_sha` variants of cached methods — bypassing the
    /// ambient ref→SHA cache entirely. The full list path may include
    /// batched ahead/behind data; single-item callers intentionally use a
    /// plain ref snapshot and let per-row tasks fall back to per-pair
    /// queries. `None` when capture failed (degraded mode).
    pub snapshot: Option<std::sync::Arc<worktrunk::git::RefSnapshot>>,

    /// Whether `WorkingTreeDiffTask` should include untracked files in
    /// `HEAD±`. Set by `wt list --full` and `wt statusline`; consumed
    /// in `tasks.rs` where the cost/cutover rationale lives.
    pub include_untracked_in_working_diff: bool,
}

impl CollectOptions {
    /// Build options whose task plan is derived from the columns that will
    /// render under `gates` — the canonical "declare the columns, get the tasks"
    /// entry, so nothing hand-writes a task set. The context fields (url
    /// template, llm command, snapshot, …) start empty; callers set what they
    /// need (e.g. `CollectOptions { url_template, ..for_columns(cols, &gates) }`).
    ///
    /// `collect` builds its options directly — it has the context fields already
    /// resolved — so this is for the single-item callers (statusline).
    pub fn for_columns(
        rendered: impl IntoIterator<Item = super::columns::ColumnKind>,
        gates: &super::columns::ColumnGates,
    ) -> Self {
        Self {
            tasks: super::columns::required_tasks_for_render(rendered, gates),
            url_template: None,
            llm_command: None,
            default_branch: None,
            integration_targets: None,
            snapshot: None,
            include_untracked_in_working_diff: false,
        }
    }
}

fn worktree_branch_set(worktrees: &[WorktreeInfo]) -> HashSet<&str> {
    worktrees
        .iter()
        .filter_map(|wt| wt.branch.as_deref())
        .collect()
}

/// Progressive callback used by the picker to mirror `wt list`'s skeleton-first
/// rendering into the skim TUI.
///
/// `collect()` owns the layout and re-renders each row as task results land.
/// The handler receives pre-rendered strings so it doesn't need to share the
/// layout across threads (`LayoutConfig` is `!Sync` via an interior
/// `Cell<&'static str>`).
pub trait PickerProgressHandler: Send + Sync {
    /// Fired once after items are initialized and layout is computed, but
    /// before any task results arrive. `rendered` is one entry per item,
    /// with fast fields (branch, path, head) populated and blank
    /// placeholders for slow cells. `header` is the column-header line;
    /// the handler calls `render()` / `plain_text()` as needed. `grid` is
    /// the layout's column geometry, for rows rendered outside collect
    /// (the picker's `--prs` rows align their cells to it).
    fn on_skeleton(
        &self,
        items: Vec<super::model::ListItem>,
        rendered: Vec<String>,
        header: worktrunk::styling::StyledLine,
        grid: super::layout::ColumnGrid,
    );

    /// Fired after a single task result updates row `idx`. `rendered` is the
    /// new line — write it through the item's shared state and wake the picker
    /// to repaint (skim 4.x renders on demand, not on a timer). `item` is the
    /// row's current model carrying the just-updated fields; the picker reads
    /// `pr_status` from it to feed the live `pr` preview tab, which cannot see
    /// the frozen skeleton snapshot.
    fn on_update(&self, idx: usize, rendered: String, item: &super::model::ListItem);

    /// Rewrite every row's rendered line from `rendered` and repaint. The
    /// handler writes one slot per row — slot writes are idempotent — then
    /// pokes skim. Two callers, both handing over a full set of freshly
    /// rendered rows:
    /// - the post-skeleton commit paint (Age/Message filled from the
    ///   pre-skeleton batch, every task column still on its placeholder),
    /// - the 200ms reveal (placeholder promoted from blank to `·`: rows that
    ///   have data use `format_list_item_line`, rows still at skeleton state
    ///   use the skeleton renderer).
    fn repaint_rows(&self, rendered: Vec<String>);

    /// Stash a pre-formatted warning line. Skim owns the terminal while
    /// collect runs on the picker's bg thread, so eprintln from collect
    /// would corrupt the rendered frame. The picker drains stashed lines
    /// after `Skim::run_with` returns, when stderr is safe again.
    fn stash_warning(&self, line: String);

    /// Fired once before `collect` returns `Ok(Some(_))`. Lets the picker
    /// kick off the deferred tier of background work — secondary preview
    /// modes for items 1..N, and LLM summaries for items 1..N.
    /// `COLLECT_POOL` serves both pipelines. Deferring this tier
    /// until drain-end keeps low-priority preview submissions out of
    /// the injector while row tasks dominate worker deques.
    ///
    /// Not fired on the `WORKTRUNK_SKELETON_ONLY` / `WORKTRUNK_FIRST_OUTPUT`
    /// benchmark early-exit, nor on the zero-worktree `Ok(None)` return
    /// (which exits before `on_skeleton`). Default: no-op.
    fn on_collect_complete(&self) {}

    /// Hand the picker a clone of the collect layout, right after `on_skeleton`.
    /// `LayoutConfig` is `!Sync`, so the handler stows it behind a lock for the
    /// collector to read at `alt-x` time, where it renders a `/ branch` row on
    /// the same grid as the worktree rows (the in-place morph). Default: no-op —
    /// only the picker needs it. Column geometry is stable after skeleton (the
    /// `--prs` rows already rely on that via `grid`), so one clone here suffices.
    fn provide_layout(&self, _layout: &super::layout::LayoutConfig) {}
}

/// Controls how show flags (branches/remotes/full) are determined in [`collect`].
pub enum ShowConfig {
    /// Flags already resolved by the caller (used by the picker).
    ///
    /// The picker is always `wt list --full` (`show_full` is implicit): it
    /// fetches every field for its preview tabs, so it has no skip set to pass —
    /// `collect` derives the task set from the columns like every other caller.
    Resolved {
        show_branches: bool,
        show_remotes: bool,
        command_timeout: Option<std::time::Duration>,
        /// Wall-clock deadline for the collect phase. `None` uses the default
        /// [`DRAIN_TIMEOUT`](results::DRAIN_TIMEOUT) and shows a warning on timeout.
        collect_deadline: Option<std::time::Instant>,
        /// Width used when computing the layout. `None` falls back to the
        /// terminal width; the picker passes an explicit width because the
        /// list only gets part of the terminal (the rest is preview).
        list_width: Option<usize>,
        /// Progressive callback for the picker. When set, `collect` emits
        /// skeleton + per-update events through it. Results still flow into
        /// the returned `ListData` as usual.
        progressive_handler: Option<std::sync::Arc<dyn PickerProgressHandler>>,
    },
    /// Raw CLI flags; config resolution deferred to collect's parallel phase
    /// so project_identifier runs concurrently with other git operations.
    /// Timeouts are resolved from config internally.
    DeferredToParallel {
        cli_branches: bool,
        cli_remotes: bool,
        cli_full: bool,
    },
}

/// On the reveal tick, every row is re-rendered. Rows that have already
/// received at least one task result use `format_list_item_line` so still-
/// pending cells pick up the promoted `·`; rows with no data yet stay on
/// the skeleton renderer to avoid flashing seeded defaults like "55y". The
/// `has_data` bitmap is the only state needed to make that choice. Callers
/// pass the post-reveal placeholder (always [`super::render::PLACEHOLDER`]
/// in production — the reveal tick is what promotes blank to dot).
fn render_reveal(
    has_data: &[bool],
    items: &[super::model::ListItem],
    layout: &super::layout::LayoutConfig,
    placeholder: &str,
) -> Vec<String> {
    items
        .iter()
        .enumerate()
        .map(|(idx, item)| {
            if has_data[idx] {
                layout.format_list_item_line(item, placeholder)
            } else {
                layout.render_skeleton_row(item, placeholder).render()
            }
        })
        .collect()
}

/// Build the progressive-table footer shown while the drain is stalled.
///
/// Pure so it can be snapshot-tested without spinning up the live table.
/// `first_name` is a branch / display name from the pending set;
/// `pending_count` is the total outstanding-result count (≥ 1).
fn format_stall_footer(
    footer_base: &str,
    completed: usize,
    total: usize,
    pending_count: usize,
    first_kind: TaskKind,
    first_name: &str,
) -> String {
    let dim = Style::new().dimmed();
    let kind_str: &'static str = first_kind.into();
    let waiting_clause = if pending_count == 1 {
        cformat!("waiting on <underline>{kind_str}</> for <underline>{first_name}</>")
    } else {
        cformat!(
            "waiting on {pending_count} tasks, including <underline>{kind_str}</> for <underline>{first_name}</>"
        )
    };
    cformat!(
        "{INFO_SYMBOL} {dim}{footer_base} ({completed}/{total} loaded, no recent progress; {waiting_clause}){dim:#}"
    )
}

/// Emit the drain-timeout warning + hint when the default 120s
/// `DRAIN_TIMEOUT` was hit. No-op for `Complete` outcomes or when an
/// explicit `collect_deadline` was supplied — those are intentional
/// truncations the caller controls.
///
/// Split out so tests can drive it with a synthetic `DrainOutcome::TimedOut`
/// without spinning up a real 120s drain.
fn handle_drain_timeout(
    drain_outcome: DrainOutcome,
    collect_deadline: Option<std::time::Instant>,
    emit: &dyn Fn(String),
) {
    if collect_deadline.is_none()
        && let DrainOutcome::TimedOut {
            received_count,
            items_with_missing,
        } = drain_outcome
    {
        let diag = format_drain_timeout_diag(received_count, &items_with_missing);
        emit(warning_message(&diag).to_string());
        emit(
            hint_message(cformat!(
                "A git command likely hung; for details, re-run with <underline>-v</>; for a diagnostic file, re-run with <underline>-vv</>"
            ))
            .to_string(),
        );
    }
}

/// Build the drain-timeout diagnostic shown when the default 120s
/// `DRAIN_TIMEOUT` is hit. Returns the pre-formatted warning text — the
/// caller wraps it in `warning_message` and routes through the picker stash
/// or stderr. Pure so tests can exercise it without spinning up a 120s drain.
fn format_drain_timeout_diag(
    received_count: usize,
    items_with_missing: &[MissingResult],
) -> String {
    let mut diag = format!(
        "Listing worktrees timed out after {}s ({received_count} results received)",
        results::DRAIN_TIMEOUT.as_secs()
    );

    if !items_with_missing.is_empty() {
        const MAX_SHOWN: usize = 5;
        diag.push_str("; blocked tasks:");
        let mut missing_lines: Vec<String> = items_with_missing
            .iter()
            .take(MAX_SHOWN)
            .map(|result| {
                let missing_names: Vec<&str> =
                    result.missing_kinds.iter().map(|k| k.into()).collect();
                cformat!("<bold>{}</>: {}", result.name, missing_names.join(", "))
            })
            .collect();
        if let Some(extra) = items_with_missing
            .len()
            .checked_sub(MAX_SHOWN)
            .filter(|n| *n > 0)
        {
            missing_lines.push(format!("… and {extra} more"));
        }
        diag.push_str(&format!(
            "\n{}",
            format_with_gutter(&missing_lines.join("\n"), None)
        ));
    }
    diag
}

/// Collect worktree data with rendering driven by `render_target`.
///
/// - [`RenderTarget::Table { progressive: true }`]: renders a skeleton
///   immediately and updates rows in place as data arrives, then morphs the
///   skeleton into the final table.
/// - [`RenderTarget::Table { progressive: false }`]: collects silently, then
///   prints the final table once.
/// - [`RenderTarget::Json`]: collects silently and returns data without
///   writing to stdout. Used by `--format=json` and the picker (which has its
///   own progressive UI driven via `ShowConfig::Resolved::progressive_handler`).
pub fn collect(
    repo: &Repository,
    show_config: ShowConfig,
    render_target: RenderTarget,
) -> anyhow::Result<Option<super::model::ListData>> {
    let show_progress = matches!(render_target, RenderTarget::Table { progressive: true });
    let render_table = matches!(render_target, RenderTarget::Table { .. });
    worktrunk::trace::instant("List collect started");

    // Determine what to fetch speculatively in the parallel phase.
    //
    // For Resolved: respect the caller's flags (fetch only what's requested).
    // For DeferredToParallel: always fetch local branches speculatively (~7ms,
    // hidden by parallelism) since config resolution happens after. Remote
    // branches are only fetched if the CLI flag is set (can be expensive).
    let (fetch_branches, fetch_remotes) = match &show_config {
        ShowConfig::Resolved {
            show_branches,
            show_remotes,
            ..
        } => (*show_branches, *show_remotes),
        ShowConfig::DeferredToParallel { cli_remotes, .. } => {
            // Always fetch local branches: ~7ms hidden by parallelism, needed if
            // config says branches=true (which we won't know until after this phase).
            // Only fetch remotes when CLI-requested (can be expensive, rarely config-only).
            let fetch_branches = true;
            let fetch_remotes = *cli_remotes;
            (fetch_branches, fetch_remotes)
        }
    };

    // Phase 1: Parallel fetch of ALL independent git data
    //
    // Key insight: most operations don't depend on each other. By running them all
    // in parallel via rayon::scope, we minimize wall-clock time. Dependencies:
    //
    // - worktree list: independent (needed for filtering and SHAs)
    // - default_branch: independent (git config + verify)
    // - is_bare: independent (git config, cached for later use)
    // - url_template: independent (loads project config via show-toplevel)
    // - project_identifier: independent (git config for remote URL; warms cache
    //   for is_worktree_at_expected_path and config resolution)
    // - local_branches: independent (one `for-each-ref refs/heads/`; cached on
    //   `RepoCache` so later consumers read it without re-scanning)
    // - remote_branches: independent (one `for-each-ref refs/remotes/`; cached
    //   on `RepoCache`)
    //
    // After this scope completes, we have all raw data and can do CPU-only work.
    let default_branch_cell: OnceCell<Option<String>> = OnceCell::new();
    let url_template_cell: OnceCell<Option<String>> = OnceCell::new();

    rayon::scope(|s| {
        s.spawn(|_| {
            // Prime the worktree list on `RepoCache`; consumers below read it
            // through `repo.list_worktrees()`.
            let _ = repo.list_worktrees();
        });
        s.spawn(|_| {
            let _ = default_branch_cell.set(repo.default_branch());
        });
        s.spawn(|_| {
            // Populate is_bare cache (value used later via repo_path)
            let _ = repo.is_bare();
        });
        s.spawn(|_| {
            let _ = url_template_cell.set(repo.url_template());
        });
        s.spawn(|_| {
            // Warm project_identifier + user config caches — used by
            // is_worktree_at_expected_path and config resolution. Running this here
            // avoids sequential git commands later on the critical path.
            let _ = repo.config();
        });
        s.spawn(|_| {
            if fetch_branches {
                // Prime the local-branch inventory on `RepoCache`; consumers
                // below read it through `repo.local_branches()`.
                let _ = repo.local_branches();
            }
        });
        s.spawn(|_| {
            if fetch_remotes {
                // Prime the remote-branch inventory on `RepoCache`.
                let _ = repo.remote_branches();
            }
        });
    });

    // Extract results
    let worktrees: &[WorktreeInfo] = repo.list_worktrees().context("Failed to list worktrees")?;
    if worktrees.is_empty() {
        return Ok(None);
    }
    // Both cells are unconditionally `set()` inside the rayon scope above, so
    // `into_inner()` is always `Some`. Use `.flatten()` rather than `.unwrap()`
    // to honor the no-unwrap rule and match the sibling cells below.
    let default_branch = default_branch_cell.into_inner().flatten();
    let url_template = url_template_cell.into_inner().flatten();

    // Resolve show flags: merge CLI overrides with config (warmed in parallel phase)
    let (
        show_branches,
        show_remotes,
        show_full,
        command_timeout,
        collect_deadline,
        list_width,
        progressive_handler,
        include_untracked_in_working_diff,
    ) = match show_config {
        ShowConfig::Resolved {
            show_branches,
            show_remotes,
            command_timeout,
            collect_deadline,
            list_width,
            progressive_handler,
        } => (
            show_branches,
            show_remotes,
            // Picker is the only `Resolved` caller and is `wt list --full`: it
            // fetches every field for its preview tabs regardless of which
            // columns render. Like default `wt list` (but unlike `--full`) it
            // opts out of the untracked-inclusive working diff — the last tuple
            // field — so the two `show_full`-shaped values aren't the same bucket.
            true,
            command_timeout,
            collect_deadline,
            list_width,
            progressive_handler,
            false,
        ),
        ShowConfig::DeferredToParallel {
            cli_branches,
            cli_remotes,
            cli_full,
        } => {
            let config = repo.config();
            let show_branches = cli_branches || config.list.branches();
            let show_remotes = cli_remotes || config.list.remotes();
            let show_full = cli_full || config.list.full();
            // Resolve timeouts from merged config (--full disables both)
            let (command_timeout, collect_deadline) = if show_full {
                (None, None)
            } else {
                let task_timeout = config.list.task_timeout();
                let deadline = config.list.timeout().map(|d| std::time::Instant::now() + d);
                (task_timeout, deadline)
            };
            (
                show_branches,
                show_remotes,
                show_full,
                command_timeout,
                collect_deadline,
                None,
                None,
                show_full,
            )
        }
    };

    // The picker (`wt switch`) drives a skim TUI that owns the terminal while
    // collect runs on a background thread. Any stderr write from collect
    // would overlay the picker's rendered frame and corrupt skim's clear
    // math, so warnings go through the handler's stash instead — picker
    // drains and emits them after `Skim::run_with` returns.
    let emit_warning = |line: String| {
        if let Some(h) = progressive_handler.as_ref() {
            h.stash_warning(line);
        } else {
            eprintln!("{line}");
        }
    };

    // Opportunistic stale-default-branch check: `default_branch` above is
    // the persisted value, now trusted without validation on the hot path.
    // Cross-check against the enumerated branch set and surface a warning
    // if it's been deleted externally. When `show_branches` is off but a
    // persisted default is set and isn't a worktree branch, scan the local
    // branch inventory anyway (one `for-each-ref` fork, cached afterwards)
    // so the warning fires on plain `wt list` too — otherwise downstream
    // tasks resolve against the stale ref and emit a cascade of "ambiguous
    // argument" noise instead of one clean warning.
    let worktree_branches = worktree_branch_set(worktrees);
    let needs_stale_check = default_branch
        .as_deref()
        .is_some_and(|b| !worktree_branches.contains(b));
    let fetched_local: Option<&[LocalBranch]> = if show_branches || needs_stale_check {
        Some(repo.local_branches()?)
    } else {
        None
    };
    let warn_stale_default = needs_stale_check
        && fetched_local.is_some_and(|all| {
            !all.iter()
                .any(|b| Some(b.name.as_str()) == default_branch.as_deref())
        });

    // Filter local branches to those without worktrees (CPU-only, no git
    // commands). With `show_branches` off there are no branch-only rows.
    let branches_without_worktrees: Vec<(String, String)> = fetched_local
        .unwrap_or(&[])
        .iter()
        .filter(|_| show_branches)
        .filter(|b| !worktree_branches.contains(b.name.as_str()))
        .map(|b| (b.name.clone(), b.commit_sha.clone()))
        .collect();

    if warn_stale_default && let Some(branch) = default_branch.as_deref() {
        emit_warning(
            warning_message(cformat!(
                "Configured default branch <bold>{branch}</> does not exist locally"
            ))
            .to_string(),
        );
        emit_warning(
            hint_message(cformat!(
                "To reset, run <underline>wt config state default-branch clear</>"
            ))
            .to_string(),
        );
    }

    // When the persisted default is stale, drop it for downstream tasks.
    // Tasks that resolve against it (ahead-behind, merge-tree-conflicts,
    // etc.) would otherwise emit a cascade of "ambiguous argument" errors;
    // passing `None` here preserves the old None-returns silent-skip
    // behavior that callers already handle for repos with no default branch.
    let default_branch = if warn_stale_default {
        None
    } else {
        default_branch
    };
    // Remote branches that aren't tracked by any local branch. Filtering
    // happens over the cached inventories — no extra subprocess.
    let remote_branches: Vec<(String, String)> = if show_remotes {
        let tracked: HashSet<&str> = repo
            .local_branches()?
            .iter()
            .filter_map(|b| b.upstream_short.as_deref())
            .collect();
        repo.remote_branches()?
            .iter()
            .filter(|r| !tracked.contains(r.short_name.as_str()))
            .map(|r| (r.short_name.clone(), r.commit_sha.clone()))
            .collect()
    } else {
        Vec::new()
    };

    // Detect current worktree using git rev-parse --show-toplevel (via WorkingTree::root).
    // This correctly handles worktrees placed inside other worktrees (e.g., .worktrees/ layout)
    // by letting git resolve the actual worktree root rather than using prefix matching.
    // Canonicalize both paths to handle symlinks (e.g., macOS /var -> /private/var).
    let current_worktree_path = repo.current_worktree().root().ok().and_then(|root| {
        worktrees
            .iter()
            .find(|wt| canonicalize(&wt.path).map(|p| p == root).unwrap_or(false))
            .map(|wt| wt.path.clone())
    });
    // Main worktree is the primary worktree (for sorting and is_main display).
    // - Normal repos: the main worktree (repo root)
    // - Bare repos: the default branch's worktree
    let primary_path = repo.primary_worktree()?;
    let main_worktree = primary_path
        .as_ref()
        .and_then(|p| worktrees.iter().find(|wt| wt.path == *p))
        .or_else(|| worktrees.iter().find(|wt| !wt.is_prunable()))
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("No worktrees found"))?;

    // Defer previous_branch lookup until after skeleton - set is_previous later
    // (skeleton shows placeholder gutter, actual symbols appear when data loads)

    // Phase 3: Batch fetch commit details (timestamp + subject) for all SHAs
    // from worktrees + branches. The returned map is the single canonical
    // source of commit details — sorting below and post-skeleton item
    // population both read from it. A batch failure is surfaced as a
    // warning and degrades to empty-cell placeholders (no hidden per-SHA
    // recovery forks).
    //
    // Filter out null OIDs from unborn branches — a single null OID would cause
    // `git log --no-walk` to fail for ALL shas in the batch.
    let all_shas: Vec<&str> = worktrees
        .iter()
        .map(|wt| wt.head.as_str())
        .chain(
            branches_without_worktrees
                .iter()
                .map(|(_, sha)| sha.as_str()),
        )
        .chain(remote_branches.iter().map(|(_, sha)| sha.as_str()))
        .filter(|sha| *sha != worktrunk::git::NULL_OID)
        .collect();
    let commit_details_map = repo.commit_details_many(&all_shas).unwrap_or_else(|err| {
        // Surface git's actual stderr (when available via the typed leaf)
        // rather than our `CommandError` summary.
        let detail = err.display_message();
        emit_warning(
            warning_message(cformat!("Failed to batch-fetch commit details: {detail}")).to_string(),
        );
        std::collections::HashMap::new()
    });

    // Sort worktrees: current first, main second, then by timestamp descending
    let sorted_worktrees = sort_worktrees_with_cache(
        worktrees,
        &main_worktree,
        current_worktree_path.as_ref(),
        &commit_details_map,
    );

    // Sort branches by timestamp (most recent first)
    let branches_without_worktrees = sort_by_timestamp_desc_with_cache(
        branches_without_worktrees,
        &commit_details_map,
        |(_, sha)| sha.as_str(),
    );
    let remote_branches =
        sort_by_timestamp_desc_with_cache(remote_branches, &commit_details_map, |(_, sha)| {
            sha.as_str()
        });

    // Pre-canonicalize main_worktree.path for is_main comparison
    // (paths from git worktree list may differ based on symlinks or working directory)
    let main_worktree_canonical = canonicalize(&main_worktree.path).ok();

    // URL template already fetched in parallel join (layout needs to know if column is needed)
    // Initialize worktree items with identity fields and None for computed fields
    let mut all_items: Vec<ListItem> = sorted_worktrees
        .iter()
        .map(|wt| {
            // Canonicalize paths for comparison - git worktree list may return different
            // path representations depending on symlinks or which directory you run from
            let wt_canonical = canonicalize(&wt.path).ok();
            let is_main = match (&wt_canonical, &main_worktree_canonical) {
                (Some(wt_c), Some(main_c)) => wt_c == main_c,
                // Fallback to direct comparison if canonicalization fails
                _ => wt.path == main_worktree.path,
            };
            let is_current = current_worktree_path
                .as_ref()
                .is_some_and(|cp| wt_canonical.as_ref() == Some(cp));
            // is_previous set to false initially - computed after skeleton
            let is_previous = false;

            // Check if worktree is at its expected path based on config template
            let branch_worktree_mismatch =
                !is_worktree_at_expected_path(wt, repo, repo.user_config());

            let mut worktree_data =
                WorktreeData::from_worktree(wt, is_main, is_current, is_previous);
            worktree_data.branch_worktree_mismatch = branch_worktree_mismatch;

            // URL expanded post-skeleton to minimize time-to-skeleton
            ListItem {
                head: wt.head.clone(),
                short_sha: String::new(),
                branch: wt.branch.clone(),
                commit: None,
                counts: None,
                branch_diff: None,
                committed_trees_match: None,
                has_file_changes: None,
                would_merge_add: None,
                is_patch_id_match: None,
                is_ancestor: None,
                is_orphan: None,
                upstream: None,
                pr_status: None,
                url: None,
                url_active: None,
                summary: None,
                has_merge_tree_conflicts: None,
                user_marker: None,
                status_symbols: StatusSymbols::default(),
                statusline: None,
                custom_values: Vec::new(),
                kind: ItemKind::Worktree(Box::new(worktree_data)),
            }
        })
        .collect();

    // Initialize branch items (local and remote) - URLs expanded post-skeleton
    let branch_start_idx = all_items.len();
    all_items.extend(
        branches_without_worktrees
            .iter()
            .map(|(name, sha)| ListItem::new_branch(sha.clone(), name.clone())),
    );

    let remote_start_idx = all_items.len();
    all_items.extend(
        remote_branches
            .iter()
            .map(|(name, sha)| ListItem::new_remote_branch(sha.clone(), name.clone())),
    );

    // Gate inputs for the task-planning decision below. `llm_command` also flows
    // into each `TaskContext` (the per-item SummaryGenerate guard) further down.
    let config = repo.config();
    let llm_command = config.commit_generation.command.clone();

    // Custom [list.custom-columns] values expand before layout: their inputs
    // (branch, worktree identity, vars from the bulk config snapshot) are
    // already in memory, so cells paint with the skeleton and column widths
    // are measured from content like Branch/Path. Pure CPU — no subprocess.
    //
    // A broken column definition aborts `wt list` with the error. The picker
    // shares this path but runs collect on a background thread while skim
    // owns the terminal, so it can't surface an abort — it stashes a warning
    // (drained after the picker closes) and renders without custom columns.
    let custom_columns =
        match super::custom_columns::resolve_custom_columns(&config.list.custom_columns, repo) {
            Ok(columns) => columns,
            Err(e) if progressive_handler.is_some() => {
                emit_warning(warning_message(format!("Custom columns disabled: {e}")).to_string());
                Vec::new()
            }
            Err(e) => return Err(e),
        };
    if !custom_columns.is_empty() {
        let all_vars = repo.all_vars_from_snapshot()?;
        super::custom_columns::expand_custom_columns(
            &custom_columns,
            &mut all_items,
            &all_vars,
            repo,
        );
    }

    // `[list] columns` selects/reorders the columns to render. Names address
    // built-ins or custom columns (by header → resolved index), so the custom
    // names are passed in resolution order. Like custom columns, a bad name
    // aborts `wt list` but only degrades the picker (which can't surface an
    // abort mid-render), so the same `progressive_handler` fork applies. An
    // empty selection means "use the default column set".
    let custom_names: Vec<&str> = custom_columns.iter().map(|c| c.name.as_str()).collect();
    let selected_columns =
        match super::columns::parse_selected_columns(&config.list.columns, &custom_names) {
            Ok(columns) => columns,
            Err(e) if progressive_handler.is_some() => {
                emit_warning(warning_message(format!("Column selection ignored: {e}")).to_string());
                Vec::new()
            }
            Err(e) => return Err(e),
        };

    // Decide, in one place, which background tasks to run: the union of the
    // tasks every column the table will render needs. This is the canonical
    // "what do we need" stage — the spawn loop fires exactly this set, and the
    // layout filter renders exactly the columns it feeds.
    //
    // The rendered set is the `[list] columns` selection for the table; the
    // picker and JSON ignore it (`all_columns`) because their consumers — the
    // picker's preview tabs, JSON's every-field contract in `src/cli/mod.rs` —
    // need the full data set, not just what renders. The gates (`--full`,
    // `[list] summary` + `[commit.generation]`, a url template) then drop a
    // column and its tasks regardless of selection.
    //
    // So a branch/path `ls` alias over many dirty worktrees runs no `git status`
    // / diffs / ahead-behind walks (#3133), while a column gated off elsewhere
    // stays off — selection narrows the work, never forces it on.
    let gates = super::columns::ColumnGates {
        show_full,
        summary_enabled: config.list.summary(),
        has_llm_command: llm_command.is_some(),
        has_url_template: url_template.is_some(),
    };
    let prune_to_selection =
        render_table && progressive_handler.is_none() && !selected_columns.is_empty();
    let tasks = if prune_to_selection {
        super::columns::required_tasks_for_render(selected_columns.iter().copied(), &gates)
    } else {
        super::columns::required_tasks_for_render(super::columns::all_columns(), &gates)
    };

    // The picker primes its CI cells from the local cache so the column paints
    // instantly, then the live `CiStatus` task (which the picker keeps — see
    // `handle_picker`) overwrites each cell as results stream in. Uncached rows
    // stay pending until the fetch reports, exactly like the other progressive
    // columns. `wt list` drives its progressive render through `progressive_state`,
    // not a handler, so the prime is picker-only.
    if progressive_handler.is_some() {
        super::ci_status::populate_from_cache(repo, &mut all_items);
    }

    // CI column width hint: the largest PR/MR number any previous fetch saw
    // (one small file read — cheap enough for the pre-skeleton budget, and
    // the skeleton can't size the column without it). Whatever fetch wrote a
    // cache entry also ratcheted this maximum, so the hint already covers any
    // number the prime above reads back.
    let max_pr_number = tasks
        .contains(&TaskKind::CiStatus)
        .then(|| super::ci_status::MaxPrNumber::read(repo))
        .flatten();

    // Calculate layout from items (worktrees, local branches, and remote branches).
    // The picker passes an explicit width because the list only gets part of the
    // terminal — the rest belongs to the preview pane.
    let layout = super::layout::calculate_layout_with_width(
        &all_items,
        &tasks,
        list_width
            .or_else(crate::display::terminal_width)
            .unwrap_or(usize::MAX),
        &main_worktree.path,
        url_template.as_deref(),
        max_pr_number,
        super::layout::ColumnSelection {
            custom: &custom_columns,
            selected: (!selected_columns.is_empty()).then_some(selected_columns.as_slice()),
        },
    );

    // Single-line invariant: with no detectable width, an unlimited width
    // keeps rows untruncated rather than wrapping at a guessed width
    let max_width = crate::display::terminal_width().unwrap_or(usize::MAX);

    // Create collection options from the planned task set. `integration_targets`
    // is patched in after the parallel phase below extracts it — at this
    // point we haven't yet resolved it, but task spawning doesn't happen
    // until line 1090+ so late population is safe.
    let mut options = CollectOptions {
        tasks,
        url_template: url_template.clone(),
        llm_command,
        default_branch: default_branch.clone(),
        integration_targets: None,
        snapshot: None,
        include_untracked_in_working_diff,
    };

    // Track expected results per item - populated as spawns are queued
    let expected_results = std::sync::Arc::new(ExpectedResults::default());
    let num_worktrees = all_items
        .iter()
        .filter(|item| item.worktree_data().is_some())
        .count();
    let num_local_branches = branches_without_worktrees.len();
    let num_remote_branches = remote_branches.len();

    let footer_base =
        if (show_branches && num_local_branches > 0) || (show_remotes && num_remote_branches > 0) {
            let mut parts = vec![format!("{} worktrees", num_worktrees)];
            if show_branches && num_local_branches > 0 {
                parts.push(format!("{} branches", num_local_branches));
            }
            if show_remotes && num_remote_branches > 0 {
                parts.push(format!("{} remote branches", num_remote_branches));
            }
            format!("Showing {}", parts.join(", "))
        } else {
            let plural = if num_worktrees == 1 { "" } else { "s" };
            format!("Showing {} worktree{}", num_worktrees, plural)
        };

    // Track which placeholder rendering currently uses. Progressive renderers
    // (table or picker) start blank so commands that finish under
    // `PLACEHOLDER_REVEAL_DELAY` never flash the `·` loading indicator; the
    // reveal event below promotes it to [`super::render::PLACEHOLDER`].
    // Non-progressive callers (e.g. JSON, buffered table) render once at the
    // end and use the dot directly.
    let progressive_active = show_progress || progressive_handler.is_some();
    let mut placeholder: &'static str = if progressive_active {
        super::render::PLACEHOLDER_BLANK
    } else {
        super::render::PLACEHOLDER
    };

    // Create progressive table if showing progress.
    let mut progressive_table = if show_progress {
        let dim = Style::new().dimmed();

        // Build skeleton rows for both worktrees and branches
        // All items need skeleton rendering since computed data (timestamp, ahead/behind, etc.)
        // hasn't been loaded yet. Using format_list_item_line would show default values like "55y".
        let skeletons: Vec<String> = all_items
            .iter()
            .map(|item| layout.render_skeleton_row(item, placeholder).render())
            .collect();

        let initial_footer = format!("{INFO_SYMBOL} {dim}{footer_base} (loading...){dim:#}");

        let mut table = ProgressiveTable::new(
            layout.format_header_line(),
            skeletons,
            initial_footer,
            max_width,
        );
        table.render_skeleton()?;
        worktrunk::trace::instant("Skeleton rendered");
        Some(table)
    } else {
        None
    };

    // Deliver the skeleton to the picker handler. Rendered strings use the
    // blank placeholder so skim's initial render mirrors the `wt list`
    // pre-reveal look.
    if let Some(handler) = progressive_handler.as_ref() {
        let skeletons: Vec<String> = all_items
            .iter()
            .map(|item| layout.render_skeleton_row(item, placeholder).render())
            .collect();
        handler.on_skeleton(
            all_items.clone(),
            skeletons,
            layout.render_header_line(),
            layout.column_grid(),
        );
        // Hand the picker the layout so `alt-x` can render a `/ branch` row on
        // this same grid without re-collecting. No-op for non-picker handlers.
        handler.provide_layout(&layout);
        // Mirror the `wt list` progressive-table marker so `wt-perf phases`
        // sees the same boundary across both commands.
        worktrunk::trace::instant("Skeleton rendered");
    }

    /// Delay before the `·` loading indicator replaces blank placeholders.
    /// Tuned so commands that finish promptly never flash the dots.
    /// Overridable at runtime via `WORKTRUNK_PLACEHOLDER_REVEAL_MS` (milliseconds)
    /// for interactive testing — useful to inflate the delay high enough to see
    /// the reveal visually (e.g. `WORKTRUNK_PLACEHOLDER_REVEAL_MS=2000 wt list`).
    const PLACEHOLDER_REVEAL_DELAY: std::time::Duration = std::time::Duration::from_millis(200);
    let reveal_delay = std::env::var("WORKTRUNK_PLACEHOLDER_REVEAL_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(std::time::Duration::from_millis)
        .unwrap_or(PLACEHOLDER_REVEAL_DELAY);
    let placeholder_reveal_at = std::time::Instant::now() + reveal_delay;

    // Early exit for benchmarking skeleton render time / progressive
    // time-to-first-output. Buffered/piped TTFP continues to the first real
    // table write below, because there is no skeleton output in that mode.
    if std::env::var_os("WORKTRUNK_SKELETON_ONLY").is_some()
        || (std::env::var_os("WORKTRUNK_FIRST_OUTPUT").is_some()
            && (show_progress || progressive_handler.is_some()))
    {
        return Ok(None);
    }

    // === Post-skeleton computations (deferred to minimize time-to-skeleton) ===
    //
    // These operations run in parallel using rayon::scope with single-level parallelism.
    // See module docs for the timing diagram.

    // Collect worktree paths for fsmonitor starts (macOS only, fast, no git commands).
    // Git's builtin fsmonitor has race conditions under parallel load - pre-starting
    // daemons before parallel operations avoids hangs.
    #[cfg(target_os = "macos")]
    let fsmonitor_worktrees: Vec<_> = if repo.is_builtin_fsmonitor_enabled() {
        sorted_worktrees
            .iter()
            .filter(|wt| !wt.is_prunable())
            .collect()
    } else {
        vec![]
    };
    #[cfg(not(target_os = "macos"))]
    let fsmonitor_worktrees: Vec<&WorktreeInfo> = vec![];

    // Single-level parallelism: all spawns in one rayon::scope.
    // See: https://gitlab.com/gitlab-org/git/-/merge_requests/148 (scalar's fsmonitor workaround)
    // See: https://github.com/jj-vcs/jj/issues/6440 (jj hit same fsmonitor issue)
    let previous_branch_cell: OnceCell<Option<String>> = OnceCell::new();
    let snapshot_cell: OnceCell<Option<std::sync::Arc<worktrunk::git::RefSnapshot>>> =
        OnceCell::new();

    rayon::scope(|s| {
        // Previous branch lookup (for gutter symbol)
        s.spawn(|_| {
            let _ = previous_branch_cell.set(repo.switch_previous());
        });

        // Capture ref state: `for-each-ref refs/heads/ refs/remotes/`,
        // plus — when default_branch is known and the per-base
        // ahead-behind cache doesn't already cover the branches — one
        // `for-each-ref %(ahead-behind:BASE)` walk (scoped to the cold
        // subset; warm runs do neither). Tasks consume the snapshot by
        // SHA, dodging ref→SHA cache staleness.
        //
        // After the snapshot is built, an inner spawn primes the
        // `Remote⇅` cache off its already-scanned inventories — see the
        // nested `s.spawn` below for the rationale.
        //
        // TODO(ahead-behind-pool): the `%(ahead-behind)` walk that runs
        // here on a cold cache is serial — it blocks this scope, and the
        // big task pool can't open until it returns. Nothing downstream of
        // work-item generation actually needs the ahead/behind *counts*
        // (only the per-row `AheadBehindTask` reads them, and it has a
        // per-SHA fallback) — only the cheap `for-each-ref refs/heads/
        // refs/remotes/` ref scan gates work-item generation. So the walk
        // could become a single work item in the pool, overlapping the
        // other ~N workers, instead of a serial prelude. That needs an
        // inter-task dependency (the per-row tasks would wait on it, or it
        // would emit their results directly) — the work-item model has
        // none today.
        s.spawn(|s| {
            let snap = match default_branch.as_deref() {
                Some(db) => repo.capture_refs_with_ahead_behind(db).ok(),
                None => repo.capture_refs().ok(),
            }
            .map(std::sync::Arc::new);

            // Prime the `ahead-behind/` SHA-cache for the `Remote⇅`
            // column, pairing each local branch with its configured
            // upstream. Mirrors the `main↕` priming inside
            // `capture_refs_with_ahead_behind`, but per-upstream-group
            // instead of single-base: one serial `for-each-ref
            // %(ahead-behind:UPSTREAM_SHA)` walk per unique upstream
            // that's both cold and above the same threshold.
            //
            // Nested inside this spawn (rather than a sibling) so it can
            // read the snapshot's already-scanned local/remote
            // inventories — a sibling spawn would race the snapshot's
            // own scans and (when `--remotes` is off) fire a redundant
            // `for-each-ref refs/remotes/`. The primer still runs in
            // parallel with downstream scope work (fsmonitor, etc.); it
            // just gates on its data dependency.
            //
            // Scope to branches that will actually render an
            // `UpstreamTask` row: with `--branches` that's every local;
            // without it that's just the worktree-attached subset.
            // Otherwise plain `wt list` on a repo with many stale
            // tracking branches would block the worker pool on a serial
            // batch for rows nobody sees.
            if let Some(snap_arc) = snap.as_ref() {
                let snap_for_primer = std::sync::Arc::clone(snap_arc);
                s.spawn(move |_| {
                    // Honor `list.task-timeout-ms` for the primer's git
                    // commands — these are the same `for-each-ref
                    // %(ahead-behind)` / `rev-list` invocations that
                    // used to run inside `UpstreamTask`, where the
                    // worker loop sets the per-thread timeout. Without
                    // this, `wt list` could sit at the skeleton on a
                    // pathologically slow git until the (untimed) batch
                    // returned.
                    worktrunk::shell_exec::set_command_timeout(command_timeout);
                    let all_locals = snap_for_primer.local_branches();
                    let filtered_locals: Vec<LocalBranch>;
                    let candidates: &[LocalBranch] = if show_branches {
                        all_locals
                    } else {
                        filtered_locals = all_locals
                            .iter()
                            .filter(|b| worktree_branches.contains(b.name.as_str()))
                            .cloned()
                            .collect();
                        &filtered_locals
                    };
                    repo.prime_upstream_ahead_behind_cache(
                        candidates,
                        snap_for_primer.remote_branches(),
                    );
                });
            }

            let _ = snapshot_cell.set(snap);
        });

        // Fsmonitor daemon starts (one spawn per worktree)
        for wt in &fsmonitor_worktrees {
            s.spawn(|_| {
                repo.start_fsmonitor_daemon_at(&wt.path);
            });
        }
    });

    // Extract results from cells
    let previous_branch = previous_branch_cell.into_inner().flatten();
    let snapshot = snapshot_cell.into_inner().flatten();

    // Resolve integration targets from the snapshot. Same OR semantics
    // as `Repository::integration_reason` (`primary` + optional
    // `secondary` only in the diverged case).
    let integration_targets = snapshot
        .as_deref()
        .and_then(|s| repo.integration_targets(s));

    // Patch integration_targets and snapshot into options. When
    // default_branch is None (unset or stale), null integration_targets
    // out — tasks otherwise see a target derived from the stale value
    // and emit "ambiguous argument" noise.
    options.integration_targets = options
        .default_branch
        .as_ref()
        .and(integration_targets.clone());
    options.snapshot = snapshot;

    // Update is_previous on items
    if let Some(prev) = previous_branch.as_deref() {
        for item in &mut all_items {
            if item.branch.as_deref() == Some(prev)
                && let Some(wt_data) = item.worktree_data_mut()
            {
                wt_data.is_previous = true;
            }
        }
    }

    // Populate commit data on every item directly from the pre-skeleton batch
    // map. No per-SHA recovery — if the batch failed, the warning printed above
    // is the user-visible signal and Age/Message cells render their placeholder.
    //
    // `short_sha` is populated for every row (including prunable), since it's a
    // pure SHA derivation that doesn't need the worktree directory. The
    // timestamp/message bundle is skipped for prunable rows to match the old
    // task-queue UX where probes against a missing worktree dir failed.
    for item in &mut all_items {
        let Some((short_sha, timestamp, commit_message)) = commit_details_map.get(&item.head)
        else {
            continue;
        };
        item.short_sha = short_sha.clone();
        if item.worktree_data().is_some_and(|d| d.is_prunable()) {
            continue;
        }
        item.commit = Some(CommitDetails {
            timestamp: *timestamp,
            commit_message: commit_message.clone(),
        });
    }

    // No need to prime the ambient `cache.ahead_behind` here: the
    // snapshot captured above carries the same batched data, and all
    // tasks consume it by SHA. (Step 5 deletes `cache.ahead_behind`
    // entirely.)

    // Note: URL template expansion is deferred to task spawning (in collect_worktree_progressive
    // and collect_branch_progressive). This parallelizes the work and minimizes time-to-skeleton.

    // Create channel for task results
    let (tx, rx) = chan::unbounded::<Result<TaskResult, TaskError>>();

    // Collect errors for display after rendering
    let mut errors: Vec<TaskError> = Vec::new();

    // Prepare branch data if needed.
    // Tuple: (item_idx, branch_name, commit_sha, is_remote)
    let branch_data: Vec<(usize, String, String, bool)> =
        if show_branches || show_remotes {
            let mut all_branches = Vec::new();
            if show_branches {
                all_branches.extend(branches_without_worktrees.iter().enumerate().map(
                    |(idx, (name, sha))| (branch_start_idx + idx, name.clone(), sha.clone(), false),
                ));
            }
            if show_remotes {
                all_branches.extend(remote_branches.iter().enumerate().map(
                    |(idx, (name, sha))| (remote_start_idx + idx, name.clone(), sha.clone(), true),
                ));
            }
            all_branches
        } else {
            Vec::new()
        };

    // Phase 1: Generate all work items on the main thread. Work item
    // generation is fast (a fixed-size loop per item) and *must* run here
    // because it pre-populates per-item status-feeder sentinels directly on
    // `all_items` — the worker thread can't hold a mutable reference while
    // the drain loop is also mutating items.
    let mut all_work_items = Vec::new();

    // Worktree work items
    for (idx, wt) in sorted_worktrees.iter().enumerate() {
        all_work_items.extend(work_items_for_worktree(
            repo,
            wt,
            idx,
            &options,
            &expected_results,
            &tx,
            &mut all_items[idx],
        ));
    }

    // Branch work items (local + remote)
    for (item_idx, branch_name, commit_sha, is_remote) in &branch_data {
        all_work_items.extend(work_items_for_branch(
            repo,
            execution::BranchSpawn {
                name: branch_name,
                commit_sha,
                item_idx: *item_idx,
                is_remote: *is_remote,
            },
            &options,
            &expected_results,
            &mut all_items[*item_idx],
        ));
    }

    // Sort work items: network tasks last to avoid blocking local operations
    all_work_items.sort_by_key(|item| item.kind.is_network());

    // Phase 2: Execute all work items in a single Rayon pool on a worker
    // thread. Flat parallelism avoids nested-Rayon deadlocks, and the
    // worker-thread split lets the drain loop start consuming results on
    // the main thread immediately.
    let tx_worker = tx.clone();
    worktrunk::trace::instant("Spawning worker thread");
    std::thread::spawn(move || {
        worktrunk::trace::instant("Parallel execution started");
        // Run on the dedicated `COLLECT_POOL` so the blocking git subprocess
        // tasks leave the global pool free for skim's per-keystroke matcher
        // when the picker is open. See `COLLECT_POOL`.
        COLLECT_POOL.install(|| {
            all_work_items.into_par_iter().for_each(|item| {
                worktrunk::shell_exec::set_command_timeout(command_timeout);
                let result = item.execute();
                let _ = tx_worker.send(result);
            });
        });
    });

    // Drop the original sender so drain_results knows when all spawned threads are done
    drop(tx);

    // Workers are running now — paint the commit-derived columns (Age,
    // Message) while the git subprocesses spin up. They carry no task, so
    // without this they'd sit on the skeleton placeholder until the row's
    // first task result happened to redraw it, lagging behind the slower
    // task-driven columns (and the cache-warm Summary preview, the symptom
    // this addresses). Spawning the workers first means the slow git work (the
    // long pole) isn't delayed by this paint, and reading `all_items` here is
    // race-free: the worker thread only sends results through the channel —
    // the drain below is the sole `all_items` mutator and hasn't started. The
    // paint still lands before the drain renders any result, so Age/Message
    // reach the screen ahead of every task-driven column. `render_skeleton_row`
    // fills them from `item.commit` while each task column keeps its blank
    // placeholder.
    if progressive_table.is_some() || progressive_handler.is_some() {
        let commit_rows: Vec<String> = all_items
            .iter()
            .map(|item| layout.render_skeleton_row(item, placeholder).render())
            .collect();
        if let Some(table) = progressive_table.as_mut() {
            for (idx, row) in commit_rows.iter().enumerate() {
                table.update_row(idx, row.clone());
            }
            if let Err(e) = table.flush() {
                tracing::debug!(error = %e, "Progressive table commit-column paint flush failed: {}", e);
            }
        }
        if let Some(handler) = progressive_handler.as_ref() {
            handler.repaint_rows(commit_rows);
        }
    }

    // Drain task results with conditional progressive rendering.
    //
    // Progressive mutable state (table, row cache, counters) is owned by a
    // `RefCell` so the event callback (handling results, the one-shot 200ms
    // reveal, and stall hints) can mutate it. Events never run concurrently —
    // they fire between channel recvs — so the runtime borrow checks are an
    // invariant formalism, never a source of panics.
    // Table-specific state: footer progress counter, overflow guard,
    // first-result tracing. `ProgressiveTable` itself owns stdout so the
    // whole thing is local and non-`Send`.
    struct ProgressiveState {
        table: ProgressiveTable,
        completed_results: usize,
        progress_overflow: bool,
        first_result_traced: bool,
    }

    let n_items = all_items.len();
    let progressive_state = progressive_table.take().map(|table| {
        std::cell::RefCell::new(ProgressiveState {
            table,
            completed_results: 0,
            progress_overflow: false,
            first_result_traced: false,
        })
    });
    let mut has_data = vec![false; n_items];

    let drain_deadline =
        collect_deadline.unwrap_or_else(|| std::time::Instant::now() + results::DRAIN_TIMEOUT);

    // Reveal fires only when a downstream consumer is listening.
    let reveal_at = (progressive_state.is_some() || progressive_handler.is_some())
        .then_some(placeholder_reveal_at);

    let primary_target = options
        .integration_targets
        .as_ref()
        .map(|t| t.primary.as_str());

    let drain_outcome = drain_results(
        rx,
        &mut all_items,
        &mut errors,
        &expected_results,
        drain_deadline,
        primary_target,
        |event| {
            let dim = Style::new().dimmed();
            let total_results = expected_results.count();

            match event {
                results::DrainEvent::Result { item_idx, item } => {
                    has_data[item_idx] = true;

                    // JSON and buffered-table modes render once at the end,
                    // not per-result.
                    if progressive_state.is_none() && progressive_handler.is_none() {
                        return;
                    }

                    // `update_row` and the picker handler both write through
                    // an idempotent slot (terminal line / `Mutex<String>`),
                    // so forwarding every result without dedup is safe; the
                    // table dedups internally against its previous render.
                    let rendered = layout.format_list_item_line(item, placeholder);

                    if let Some(state_cell) = progressive_state.as_ref() {
                        let mut s = state_cell.borrow_mut();
                        if !s.first_result_traced {
                            s.first_result_traced = true;
                            worktrunk::trace::instant("First result received");
                        }

                        s.completed_results += 1;
                        debug_assert!(
                            s.completed_results <= total_results,
                            "completed ({}) > expected ({}): task result sent without registering expectation",
                            s.completed_results,
                            total_results
                        );
                        if s.completed_results > total_results {
                            s.progress_overflow = true;
                        }

                        let completed = s.completed_results;
                        let footer_msg = format!(
                            "{INFO_SYMBOL} {dim}{footer_base} ({completed}/{total_results} loaded){dim:#}"
                        );
                        s.table.update_footer(footer_msg);
                        s.table.update_row(item_idx, rendered.clone());

                        if let Err(e) = s.table.flush() {
                            tracing::debug!(error = %e, "Progressive table flush failed: {}", e);
                        }
                    }

                    if let Some(handler) = progressive_handler.as_ref() {
                        handler.on_update(item_idx, rendered, item);
                    }
                }
                results::DrainEvent::Reveal { items } => {
                    placeholder = super::render::PLACEHOLDER;
                    let updates = render_reveal(&has_data, items, &layout, placeholder);

                    if let Some(state_cell) = progressive_state.as_ref() {
                        let mut s = state_cell.borrow_mut();
                        for (idx, line) in updates.iter().enumerate() {
                            s.table.update_row(idx, line.clone());
                        }
                        if let Err(e) = s.table.flush() {
                            tracing::debug!(error = %e, "Progressive table reveal flush failed: {}", e);
                        }
                    }

                    if let Some(handler) = progressive_handler.as_ref() {
                        handler.repaint_rows(updates);
                    }
                }
                results::DrainEvent::Stall {
                    pending_count,
                    first_kind,
                    first_name,
                } => {
                    // No task has completed for at least `STALL_TIMINGS.threshold`.
                    // Name the signal (silence) rather than claiming "stalled":
                    // the event fires on any 5s lull and reports outstanding
                    // work, not a root cause.
                    if let Some(state_cell) = progressive_state.as_ref() {
                        let mut s = state_cell.borrow_mut();
                        let footer_msg = format_stall_footer(
                            &footer_base,
                            s.completed_results,
                            total_results,
                            pending_count,
                            first_kind,
                            first_name,
                        );
                        if s.table.update_footer(footer_msg)
                            && let Err(e) = s.table.flush()
                        {
                            tracing::debug!(error = %e, "Progressive table flush failed: {}", e);
                        }
                    }
                    // Picker has no stall UI; per-update repaints keep it
                    // responsive without a stall message.
                }
            }
        },
        reveal_at,
    );
    worktrunk::trace::instant("All results drained");

    // Extract progressive state back out. `progressive_table` is re-bound so
    // post-drain code (finalize / error rendering) works unchanged.
    let (progressive_table, progress_overflow) = match progressive_state {
        Some(cell) => {
            let s = cell.into_inner();
            (Some(s.table), s.progress_overflow)
        }
        None => (None, false),
    };
    // Force the dot for any post-drain render. The reveal event only fires
    // once the 200ms deadline has passed; if every result arrives before
    // then, the closure left `placeholder` on `PLACEHOLDER_BLANK`, and the
    // final table / finalize / timeout paths must not inherit that.
    placeholder = super::render::PLACEHOLDER;

    // Handle timeout if it occurred. Budget-based deadlines
    // (collect_deadline) are intentional truncation — don't warn. Only
    // warn for the default DRAIN_TIMEOUT (120s), which indicates a hung
    // command.
    handle_drain_timeout(drain_outcome, collect_deadline, &emit_warning);

    // The drain calls `refresh_status_symbols` after every *successful*
    // result, but items with zero successful results (all tasks errored
    // or timed out) never hit that path. Sweep every item so that
    // synchronously-derivable gates (worktree_state from metadata,
    // pre-seeded main_state for unborn/prunable items) still materialize.
    // The call is idempotent — already-resolved gates are skipped.
    for item in all_items.iter_mut() {
        item.refresh_status_symbols(primary_target);
    }

    // Count errors for summary
    let error_count = errors.len();
    let timed_out_count = errors.iter().filter(|e| e.is_timeout()).count();

    let table_render = render_table.then(|| TableRenderPlan {
        progressive_table,
        header: layout.format_header_line(),
        rows: all_items
            .iter()
            .map(|item| layout.format_list_item_line(item, placeholder))
            .collect(),
        summary: super::format_summary_message(
            &all_items,
            show_branches || show_remotes,
            layout.hidden_column_count,
            error_count,
            timed_out_count,
        ),
    });

    if let Some(table_render) = table_render
        && table_render.render()?
    {
        return Ok(None);
    }

    // Status symbols are now computed during data collection (both modes), no fallback needed

    // Display collection errors/warnings (after table rendering)
    // Filter out timeout errors - they're shown in the summary footer
    let non_timeout_errors: Vec<_> = errors.iter().filter(|e| !e.is_timeout()).collect();

    if !non_timeout_errors.is_empty() || progress_overflow {
        let mut warning_parts = Vec::new();

        if !non_timeout_errors.is_empty() {
            // Sort for deterministic output (tasks complete in arbitrary order)
            let mut sorted_errors = non_timeout_errors;
            sorted_errors.sort_by_key(|e| (e.item_idx, e.kind));
            let error_lines: Vec<String> = sorted_errors
                .iter()
                .map(|error| {
                    let name = all_items[error.item_idx].branch_name();
                    let kind_str: &'static str = error.kind.into();
                    // Take first line only - git errors can be multi-line with usage hints
                    let msg = error.message.lines().next().unwrap_or(&error.message);
                    cformat!("<bold>{}</>: {} ({})", name, kind_str, msg)
                })
                .collect();
            warning_parts.push(format!(
                "Some git operations failed:\n{}",
                format_with_gutter(&error_lines.join("\n"), None)
            ));
        }

        if progress_overflow {
            // Defensive: should never trigger now that immediate URL sends register expectations,
            // but kept to detect future counting bugs
            warning_parts.push("Progress counter overflow (completed > expected)".to_string());
        }

        let warning = warning_parts.join("\n");
        emit_warning(warning_message(&warning).to_string());

        // Show issue reporting hint (free function - doesn't collect diagnostic data)
        emit_warning(hint_message(crate::diagnostic::issue_hint()).to_string());
    }

    // Populate display fields for all items (used by JSON output and statusline)
    for item in &mut all_items {
        item.finalize_display();
    }

    // all_items now contains both worktrees and branches (if requested)
    let items = all_items;

    // Table rendering complete:
    // - `RenderTarget::Table { progressive: true }`: rows morphed in place,
    //   footer became summary
    // - `RenderTarget::Table { progressive: false }`: rendered final table
    // - `RenderTarget::Json`: no stdout rendering; data returned for the
    //   caller to serialize (`wt list --format=json`) or feed into its own
    //   UI (picker via `progressive_handler`)
    worktrunk::trace::instant("List collect complete");

    // Fire `on_collect_complete` after the row pipeline has fully torn
    // down. The picker handler uses this to spawn bulk preview pre-compute
    // for items 1..N without contending with row tasks. No-op for
    // non-picker callers (default trait impl).
    if let Some(handler) = progressive_handler.as_ref() {
        handler.on_collect_complete();
    }

    Ok(Some(super::model::ListData {
        items,
        custom_columns,
    }))
}

// ============================================================================
// Sorting Helpers
// ============================================================================

/// Sort items by timestamp descending using the pre-fetched commit-details map.
fn sort_by_timestamp_desc_with_cache<T, F>(
    items: Vec<T>,
    commit_details: &std::collections::HashMap<String, (String, i64, String)>,
    get_sha: F,
) -> Vec<T>
where
    F: Fn(&T) -> &str,
{
    // Embed timestamp in tuple to avoid parallel Vec and index lookups
    let mut with_ts: Vec<_> = items
        .into_iter()
        .map(|item| {
            let ts = commit_details
                .get(get_sha(&item))
                .map_or(0, |(_, ts, _)| *ts);
            (item, ts)
        })
        .collect();
    with_ts.sort_by_key(|(_, ts)| std::cmp::Reverse(*ts));
    with_ts.into_iter().map(|(item, _)| item).collect()
}

/// Sort worktrees: current first, main second, then by timestamp descending.
/// Uses the pre-fetched commit-details map for efficiency.
fn sort_worktrees_with_cache(
    worktrees: &[WorktreeInfo],
    main_worktree: &WorktreeInfo,
    current_path: Option<&std::path::PathBuf>,
    commit_details: &std::collections::HashMap<String, (String, i64, String)>,
) -> Vec<WorktreeInfo> {
    // Embed timestamp and priority in tuple to avoid parallel Vec and index lookups
    let mut with_sort_key: Vec<_> = worktrees
        .iter()
        .map(|wt| {
            let priority = if current_path.is_some_and(|cp| &wt.path == cp) {
                0 // Current first
            } else if wt.path == main_worktree.path {
                1 // Main second
            } else {
                2 // Rest by timestamp
            };
            let ts = commit_details.get(&wt.head).map_or(0, |(_, ts, _)| *ts);
            (wt, priority, ts)
        })
        .collect();

    with_sort_key.sort_by_key(|(_, priority, ts)| (*priority, std::cmp::Reverse(*ts)));
    with_sort_key
        .into_iter()
        .map(|(wt, _, _)| wt.clone())
        .collect()
}

// ============================================================================
// Public API for single-worktree collection (used by statusline)
// ============================================================================

/// Build a ListItem for a single worktree with identity fields only.
///
/// Computed fields (counts, diffs, CI) are left as None. Use `populate_item()`
/// to fill them in.
pub fn build_worktree_item(
    wt: &WorktreeInfo,
    is_main: bool,
    is_current: bool,
    is_previous: bool,
) -> ListItem {
    ListItem {
        head: wt.head.clone(),
        short_sha: String::new(),
        branch: wt.branch.clone(),
        commit: None,
        counts: None,
        branch_diff: None,
        committed_trees_match: None,
        has_file_changes: None,
        would_merge_add: None,
        is_patch_id_match: None,
        is_ancestor: None,
        is_orphan: None,
        upstream: None,
        pr_status: None,
        url: None,
        url_active: None,
        summary: None,
        has_merge_tree_conflicts: None,
        user_marker: None,
        status_symbols: StatusSymbols::default(),
        statusline: None,
        custom_values: Vec::new(),
        kind: ItemKind::Worktree(Box::new(WorktreeData::from_worktree(
            wt,
            is_main,
            is_current,
            is_previous,
        ))),
    }
}

/// Populate computed fields for items in parallel (blocking).
///
/// Spawns parallel git operations and collects results. Modifies items in place
/// with: commit details, ahead/behind, diffs, upstream, CI, etc.
///
/// # Parameters
/// - `repo`: Repository handle (cloned into background thread, shares cache via Arc)
///
/// This is the blocking version used by statusline. For progressive rendering
/// with callbacks, see the `collect()` function.
pub fn populate_item(
    repo: &Repository,
    item: &mut ListItem,
    mut options: CollectOptions,
) -> anyhow::Result<()> {
    // Populate commit data directly. The main `collect()` path batches this
    // across all items pre-skeleton; the single-item statusline path has no
    // such batch, so fetch the one SHA here. Skip null OIDs (unborn branches).
    // Silent on batch failure: the statusline is a compact prompt element with
    // no room for warnings, and `commit.message` / `commit.timestamp` fall
    // through to their defaults.
    //
    // `short_sha` populates for every row (including prunable). The
    // timestamp/message bundle is skipped for prunable rows to match the old
    // task-queue UX where probes against a missing worktree dir failed.
    let is_prunable = item.worktree_data().is_some_and(|d| d.is_prunable());
    if item.head != worktrunk::git::NULL_OID
        && let Ok(map) = repo.commit_details_many(&[&item.head])
        && let Some((short_sha, timestamp, commit_message)) = map.get(&item.head)
    {
        item.short_sha = short_sha.clone();
        if !is_prunable {
            item.commit = Some(CommitDetails {
                timestamp: *timestamp,
                commit_message: commit_message.clone(),
            });
        }
    }

    // Extract worktree data (skip if not a worktree item)
    let Some(data) = item.worktree_data() else {
        return Ok(());
    };

    // Populate default_branch / snapshot / integration_targets if the
    // caller didn't. Tasks read these through `TaskContext`; `None`
    // values tell them to skip (see collect()'s stale-default-branch
    // path). Single-item callers like statusline build options via
    // `CollectOptions::for_columns` (these fields start `None`) and expect
    // the repo-derived values.
    if options.default_branch.is_none() {
        options.default_branch = repo.default_branch();
    }
    if options.snapshot.is_none() {
        // Statusline needs one row. Avoid the list path's repo-wide
        // `for-each-ref %(ahead-behind:BASE)` prelude here; the
        // AheadBehind/Upstream tasks already have cached per-pair
        // fallbacks that do work only for this item.
        options.snapshot = repo.capture_refs().ok().map(std::sync::Arc::new);
    }
    if options.integration_targets.is_none() {
        options.integration_targets = options
            .snapshot
            .as_deref()
            .and_then(|s| repo.integration_targets(s));
    }

    // Create channel for task results
    let (tx, rx) = chan::unbounded::<Result<TaskResult, TaskError>>();

    // Track expected results (populated at spawn time)
    let expected_results = Arc::new(ExpectedResults::default());

    // Collect errors (logged silently for statusline)
    let mut errors: Vec<TaskError> = Vec::new();

    // Build a minimal WorktreeInfo so the shared work-item generator can
    // run. The item lives on this (main) thread; the worker thread only
    // executes prebuilt work items.
    let wt = WorktreeInfo {
        path: data.path.clone(),
        head: item.head.clone(),
        branch: item.branch.clone(),
        bare: false,
        detached: false,
        locked: None,
        prunable: None,
    };

    // Generate work items on the main thread so the item can be seeded
    // with sentinels for skipped tasks (see `work_items_for_worktree`).
    let mut work_items = work_items_for_worktree(
        repo,
        &wt,
        0, // Single item, always index 0
        &options,
        &expected_results,
        &tx,
        item,
    );

    // Sort: network tasks last
    work_items.sort_by_key(|w| w.kind.is_network());

    // Spawn collection in background thread (executes only). Route through
    // `COLLECT_POOL` for consistency with the multi-item path, though a single
    // item never floods the global pool.
    std::thread::spawn(move || {
        COLLECT_POOL.install(|| {
            work_items.into_par_iter().for_each(|w| {
                let result = w.execute();
                let _ = tx.send(result);
            });
        });
    });

    // Drain task results (blocking until complete). `drain_results`
    // writes each result onto the item and calls `refresh_status_symbols`
    // after every write, so the callback here is just a no-op — there is
    // no progressive table to refresh on the statusline path.
    let drain_outcome = drain_results(
        rx,
        std::slice::from_mut(item),
        &mut errors,
        &expected_results,
        std::time::Instant::now() + results::DRAIN_TIMEOUT,
        options
            .integration_targets
            .as_ref()
            .map(|t| t.primary.as_str()),
        |_event| {},
        None,
    );

    // Handle timeout (silent for statusline - just log it)
    if let DrainOutcome::TimedOut { received_count, .. } = drain_outcome {
        tracing::warn!(
            count = received_count,
            "populate_item timed out after {}s ({received_count} results received)",
            results::DRAIN_TIMEOUT.as_secs()
        );
    }

    // Log errors silently (statusline shouldn't spam warnings)
    if !errors.is_empty() {
        tracing::warn!(
            count = errors.len(),
            "populate_item had {} task errors",
            errors.len()
        );
        for error in &errors {
            let kind_str: &'static str = error.kind.into();
            tracing::debug!(
                item = error.item_idx,
                kind = %kind_str,
                error = %error.message,
                "  - item {}: {} ({})",
                error.item_idx,
                kind_str,
                error.message
            );
        }
    }

    // Ensure status symbols are refreshed even if all tasks errored
    // (the drain only calls refresh on the success path).
    item.refresh_status_symbols(
        options
            .integration_targets
            .as_ref()
            .map(|t| t.primary.as_str()),
    );

    // Populate display fields (including status_line for statusline command)
    item.finalize_display();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ansi_str::AnsiStr;

    #[test]
    fn test_collect_pool_num_threads_honors_env() {
        // Env var set: defer to Rayon (0 means "read RAYON_NUM_THREADS").
        assert_eq!(collect_pool_num_threads(true), 0);
        // Env var unset: explicit 2× CPU, since Rayon's own default is 1×.
        assert_eq!(collect_pool_num_threads(false), crate::rayon_thread_count());
    }

    #[test]
    fn test_format_stall_footer_single_pending() {
        let rendered =
            format_stall_footer("Showing 3 worktrees", 5, 12, 1, TaskKind::CiStatus, "feat");
        insta::assert_snapshot!(
            rendered.ansi_strip(),
            @"○ Showing 3 worktrees (5/12 loaded, no recent progress; waiting on ci-status for feat)"
        );
    }

    #[test]
    fn test_format_stall_footer_many_pending() {
        let rendered =
            format_stall_footer("Showing 3 worktrees", 5, 12, 3, TaskKind::CiStatus, "feat");
        insta::assert_snapshot!(
            rendered.ansi_strip(),
            @"○ Showing 3 worktrees (5/12 loaded, no recent progress; waiting on 3 tasks, including ci-status for feat)"
        );
    }

    /// Drain timeout diagnostic without per-item breakdown — the early-exit
    /// path when no items are blocked.
    #[test]
    fn test_format_drain_timeout_diag_no_items() {
        let rendered = format_drain_timeout_diag(7, &[]);
        insta::assert_snapshot!(
            rendered.ansi_strip(),
            @"Listing worktrees timed out after 120s (7 results received)"
        );
    }

    /// `handle_drain_timeout` emits both warning + hint when the default
    /// timeout fires (`collect_deadline: None` + `TimedOut`). Captures
    /// emissions through the closure to avoid touching real stderr.
    #[test]
    fn test_handle_drain_timeout_emits_on_default_timeout() {
        use std::sync::Mutex;
        let captured: Mutex<Vec<String>> = Mutex::new(Vec::new());
        let emit = |line: String| captured.lock().unwrap().push(line);
        let outcome = DrainOutcome::TimedOut {
            received_count: 4,
            items_with_missing: vec![],
        };
        handle_drain_timeout(outcome, None, &emit);
        let lines = captured.lock().unwrap();
        assert_eq!(lines.len(), 2, "expected warning + hint, got: {lines:?}");
        assert!(
            lines[0]
                .ansi_strip()
                .contains("Listing worktrees timed out after"),
            "warning line: {}",
            lines[0]
        );
        assert!(
            lines[1].ansi_strip().contains("re-run with -v"),
            "hint line: {}",
            lines[1]
        );
    }

    /// An explicit `collect_deadline` is intentional truncation — the helper
    /// must stay silent so user-budgeted deadlines don't surface as bugs.
    #[test]
    fn test_handle_drain_timeout_silent_when_deadline_set() {
        use std::sync::Mutex;
        let captured: Mutex<Vec<String>> = Mutex::new(Vec::new());
        let emit = |line: String| captured.lock().unwrap().push(line);
        let outcome = DrainOutcome::TimedOut {
            received_count: 0,
            items_with_missing: vec![],
        };
        let deadline = std::time::Instant::now();
        handle_drain_timeout(outcome, Some(deadline), &emit);
        assert!(
            captured.lock().unwrap().is_empty(),
            "expected no emissions for budgeted deadline"
        );
    }

    /// `Complete` outcomes are the happy path — the helper must stay silent.
    #[test]
    fn test_handle_drain_timeout_silent_on_complete() {
        use std::sync::Mutex;
        let captured: Mutex<Vec<String>> = Mutex::new(Vec::new());
        let emit = |line: String| captured.lock().unwrap().push(line);
        handle_drain_timeout(DrainOutcome::Complete, None, &emit);
        assert!(
            captured.lock().unwrap().is_empty(),
            "expected no emissions for Complete outcome"
        );
    }

    /// With blocked items, the diagnostic appends a gutter listing each
    /// item's missing task kinds. `take(5)` caps the list; cover that here.
    #[test]
    fn test_format_drain_timeout_diag_with_blocked_items() {
        let items = vec![
            MissingResult {
                item_idx: 0,
                name: "feature-a".to_string(),
                missing_kinds: vec![TaskKind::CiStatus, TaskKind::BranchDiff],
            },
            MissingResult {
                item_idx: 1,
                name: "feature-b".to_string(),
                missing_kinds: vec![TaskKind::AheadBehind],
            },
        ];
        let rendered = format_drain_timeout_diag(3, &items);
        insta::assert_snapshot!(
            rendered.ansi_strip(),
            @r"
        Listing worktrees timed out after 120s (3 results received); blocked tasks:
          feature-a: ci-status, branch-diff
          feature-b: ahead-behind
        "
        );
    }

    /// More than `MAX_SHOWN` blocked tasks truncate to the first five and
    /// append an "… and N more" line, so the count isn't silently hidden.
    #[test]
    fn test_format_drain_timeout_diag_truncates_blocked_items() {
        let items: Vec<MissingResult> = (0..8)
            .map(|i| MissingResult {
                item_idx: i,
                name: format!("feature-{i}"),
                missing_kinds: vec![TaskKind::AheadBehind],
            })
            .collect();
        let rendered = format_drain_timeout_diag(2, &items);
        insta::assert_snapshot!(
            rendered.ansi_strip(),
            @r"
        Listing worktrees timed out after 120s (2 results received); blocked tasks:
          feature-0: ahead-behind
          feature-1: ahead-behind
          feature-2: ahead-behind
          feature-3: ahead-behind
          feature-4: ahead-behind
          … and 3 more
        "
        );
    }

    /// `render_reveal` picks `format_list_item_line` for rows with data and
    /// `render_skeleton_row` for rows without — the choice is load-bearing
    /// for the picker's partial-row reveal correctness, since the
    /// post-reveal placeholder swap has to reach pending cells without
    /// surfacing seeded defaults like "55y" on rows that haven't received
    /// any task results.
    #[test]
    fn test_render_reveal_picks_renderer_per_row() {
        use super::super::layout::calculate_layout_with_width;
        use super::super::model::ListItem;
        use std::path::Path;

        let items = vec![
            ListItem::new_branch("aaa".into(), "row-zero".into()),
            ListItem::new_branch("bbb".into(), "row-one".into()),
        ];
        let tasks = super::super::columns::all_tasks();
        let layout = calculate_layout_with_width(
            &items,
            &tasks,
            80,
            Path::new("/tmp"),
            None,
            None,
            super::super::layout::ColumnSelection {
                custom: &[],
                selected: None,
            },
        );
        let placeholder = super::super::render::PLACEHOLDER;

        // Row 0 has data → format_list_item_line; row 1 doesn't → skeleton.
        let has_data = vec![true, false];
        let updates = render_reveal(&has_data, &items, &layout, placeholder);
        assert_eq!(updates.len(), 2);
        assert_eq!(
            updates[0],
            layout.format_list_item_line(&items[0], placeholder)
        );
        assert_eq!(
            updates[1],
            layout.render_skeleton_row(&items[1], placeholder).render()
        );
    }
}
