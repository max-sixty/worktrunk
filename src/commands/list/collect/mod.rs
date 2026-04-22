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
//! ### Fixed Command Count (O(1), not O(N))
//!
//! Pre-skeleton runs a **fixed number of git commands** regardless of worktree count.
//! This is achieved through:
//! - **Batching** — timestamp fetch passes all SHAs to one `git log --no-walk` command
//! - **Parallelization** — independent commands run concurrently via `join!` macro
//!
//! **Steady-state (6-8 commands):**
//!
//! | Command | Purpose | Parallel |
//! |---------|---------|----------|
//! | `git worktree list --porcelain` | Enumerate worktrees | ✓ |
//! | `git config worktrunk.default-branch` | Cached default branch | ✓ |
//! | `git config --bool core.bare` | Bare repo check for expected-path logic | ✓ |
//! | `git rev-parse --show-toplevel` | Worktree root for project config | ✓ |
//! | `git config remote.*.url` (1-3 calls) | Project identifier (for config + path check) | ✓ |
//! | `git for-each-ref refs/heads` | Only with `--branches` flag | ✓ |
//! | `git for-each-ref refs/remotes` | Only with `--remotes` flag | ✓ |
//! | `git log --no-walk --format='%H\0%ct\0%s' SHA1 SHA2 ...` | **Batched** commit details (timestamp + subject) | Sequential (needs SHAs) |
//!
//! The batched `git log` fetches subjects too, which the skeleton itself
//! doesn't strictly need — only timestamps are required for sort order. It
//! rides along for free: git has to resolve each commit object anyway, and
//! the subject bytes add no measurable latency to the round trip. The
//! subjects populate `cache.commit_details` so the post-skeleton
//! `CommitDetailsTask` is a pure cache hit instead of N `git log -1` forks.
//!
//! **Non-git operations (negligible latency):**
//! - Path canonicalization — detect current worktree
//! - Project config file read — check if URL column needed (no template expansion)
//! - Config resolution — merge project-specific settings (uses cached project identifier)
//!
//! ### First-Run Behavior
//!
//! When `worktrunk.default-branch` is not cached, `default_branch()` runs additional
//! commands to detect it:
//! 1. Query primary remote (origin/HEAD or `git ls-remote`)
//! 2. Fall back to local inference (check init.defaultBranch, common names)
//! 3. Cache result to `git config worktrunk.default-branch`
//!
//! Subsequent runs use the cached value — only one `git config` call.
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
//! │    ├─ integration_target()                  (10ms)
//! │    ├─ start_fsmonitor_daemon × N worktrees  (6ms each, all parallel)
//! │  )                                          // ~10ms total (max of all spawns)
//! Worker thread spawns
//! ```
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
//! The 23-command pre-skeleton count is above the "6-8 commands" target
//! above — worth an audit. Most of the extras come from per-worktree probes
//! that creep into the phase.
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
//! the underlying operations differ in what their output depends on.
//!
//! | Directory | Module | Key | Staleness |
//! |-----------|--------|-----|-----------|
//! | `merge-tree-conflicts/` | `git::repository::sha_cache` | `{sha1}-{sha2}.json` (sorted) | Never — content-addressed |
//! | `merge-add-probe/` | `git::repository::sha_cache` | `{branch_sha}-{target_sha}.json` | Never — content-addressed |
//! | `is-ancestor/` | `git::repository::sha_cache` | `{base_sha}-{head_sha}.json` | Never — content-addressed |
//! | `has-added-changes/` | `git::repository::sha_cache` | `{branch_sha}-{target_sha}.json` | Never — content-addressed |
//! | `diff-stats/` | `git::repository::sha_cache` | `{base_sha}-{head_sha}.json` | Never — content-addressed |
//! | `ci-status/` | `commands::list::ci_status::cache` | `{branch}.json` | TTL 30–60s + HEAD SHA check |
//! | `summaries/` | `summary` | `{branch}.json` | `diff_hash` mismatch |
//!
//! ### Key schemes
//!
//! - **SHA-pair**: pure function of two commit SHAs. Never stale, no TTL, no invalidation.
//!   Used by all `sha_cache` kinds (merge-tree conflicts, merge-add probes, ancestry
//!   checks, file-change probes, diff stats).
//! - **Branch + TTL + HEAD**: external mutable state (CI API, remote refs). TTL bounds
//!   staleness; the HEAD check invalidates early when the branch moves.
//! - **Branch + content hash**: deterministic function of a mutable input (e.g. an LLM call
//!   over a diff). Invalidates on hash mismatch.
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
//! | `CiStatus` | `ci_status::cache` |
//! | `SummaryGenerate` | `summary` |
//!
//! Every other task re-runs on each invocation.
//!
//! ### Already optimized (not cache candidates)
//!
//! - `AheadBehind` — batch-optimized via single `git for-each-ref %(ahead-behind:main)`
//!   (~11ms for all branches); per-branch tasks read the in-memory cache
//! - `CommittedTreesMatch` — single `git rev-parse` resolving both tree SHAs (~1ms)
//! - `Upstream` — upstream names batch-fetched via single `git for-each-ref
//!   %(upstream:short)`; per-branch tasks read the in-memory cache
//!
//! ### Cached via tree SHA
//!
//! `WorkingTreeConflicts` uses `git write-tree` to snapshot the index as a tree SHA,
//! then checks for merge conflicts via `has_merge_conflicts_by_tree`. The tree SHA is
//! content-addressed and stable — identical index state produces the same SHA.
//!
//! When there are unstaged modifications or untracked files, the task copies the
//! index to a temp file, runs `git add -A` to stage all working tree content,
//! then `write-tree`.
//!
//! The cache key is `(base_commit_sha, branch_head_sha+tree_sha)`. The branch HEAD
//! SHA captures the merge-base dependency. On cache miss, `has_merge_conflicts_by_tree`
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

use anstyle::Style;
use color_print::cformat;
use crossbeam_channel as chan;
use dunce::canonicalize;
use once_cell::sync::OnceCell;
use rayon::prelude::*;
use worktrunk::git::{LocalBranch, Repository, WorktreeInfo};
use worktrunk::styling::{
    INFO_SYMBOL, eprintln, format_with_gutter, hint_message, warning_message,
};

use crate::commands::is_worktree_at_expected_path;

use super::model::{DisplayFields, ItemKind, ListItem, StatusSymbols, WorktreeData};
use super::progressive_table::ProgressiveTable;

// Re-exports for sibling modules (columns.rs, render.rs, layout.rs)
pub(crate) use tasks::parse_port_from_url;
pub(crate) use types::TaskKind;

// Internal imports
pub(crate) use execution::ExpectedResults;
use execution::{work_items_for_branch, work_items_for_worktree};
use results::drain_results;
use types::DrainOutcome;
use types::{TaskError, TaskResult};

struct TableRenderPlan {
    progressive_table: Option<ProgressiveTable>,
    header: String,
    rows: Vec<String>,
    summary: String,
}

impl TableRenderPlan {
    fn render(mut self) -> anyhow::Result<()> {
        if let Some(mut table) = self.progressive_table.take() {
            if table.is_tty() {
                table.finalize(self.rows, self.summary)?;
            } else {
                print_buffered_table(&self.header, &self.rows, &self.summary);
            }
        } else {
            print_buffered_table(&self.header, &self.rows, &self.summary);
        }
        Ok(())
    }
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
#[derive(Clone, Default)]
pub struct CollectOptions {
    /// Tasks to skip (not compute). Empty set means compute everything.
    ///
    /// This controls both:
    /// - Work item generation (in `work_items_for_worktree`/`work_items_for_branch`)
    /// - Column visibility (layout filters columns via `ColumnSpec::requires_task`)
    pub skip_tasks: std::collections::HashSet<TaskKind>,

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
    /// Integration target (`default_branch`, or its upstream when ahead).
    /// `None` when the default branch is unset or stale.
    pub integration_target: Option<String>,
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
#[cfg_attr(not(unix), allow(dead_code))]
pub trait PickerProgressHandler: Send + Sync {
    /// Fired once after items are initialized and layout is computed, but
    /// before any task results arrive. `rendered` is one entry per item,
    /// with fast fields (branch, path, head) populated and blank
    /// placeholders for slow cells. `header` is the column-header line;
    /// the handler calls `render()` / `plain_text()` as needed.
    fn on_skeleton(
        &self,
        items: Vec<super::model::ListItem>,
        rendered: Vec<String>,
        header: worktrunk::styling::StyledLine,
    );

    /// Fired after a single task result updates row `idx`. `rendered` is the
    /// new line — write it through the item's shared state so skim picks it
    /// up on the next heartbeat.
    fn on_update(&self, idx: usize, rendered: String);

    /// Fired at the 200ms reveal deadline. Entry per row: `Some(line)` for
    /// rows still at skeleton state (placeholder needs promoting to `·`),
    /// `None` for rows that already received real data via `on_update`.
    fn on_reveal(&self, rendered: Vec<Option<String>>);
}

/// Controls how show flags (branches/remotes/full) are determined in [`collect`].
#[cfg_attr(not(unix), allow(dead_code))]
pub enum ShowConfig {
    /// Flags already resolved by the caller (used by the picker).
    Resolved {
        show_branches: bool,
        show_remotes: bool,
        skip_tasks: HashSet<TaskKind>,
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

/// Per-row render cache shared by the `wt list` progressive table and the
/// picker's `PickerProgressHandler`. Both sinks write through the same dedup
/// path so one rendering pass serves both.
///
/// `set_result` records a new render and returns `Some(line)` only when it
/// differs from the cached value.
///
/// `set_reveal` runs after `layout.placeholder` is promoted from blank to
/// `·`. Every row is re-rendered (skeleton for rows with no data yet to
/// avoid surfacing seeded defaults like "55y"; `format_list_item_line` for
/// rows that received at least one result, so still-pending cells pick up
/// the promoted `·`). Dedup against the cache keeps emitted updates minimal.
struct RowCache {
    last: Vec<String>,
    has_data: Vec<bool>,
}

impl RowCache {
    fn new(n: usize) -> Self {
        Self {
            last: vec![String::new(); n],
            has_data: vec![false; n],
        }
    }

    fn set_result(&mut self, idx: usize, rendered: String) -> Option<String> {
        self.has_data[idx] = true;
        if self.last[idx] == rendered {
            None
        } else {
            self.last[idx] = rendered.clone();
            Some(rendered)
        }
    }

    fn set_reveal(
        &mut self,
        items: &[super::model::ListItem],
        layout: &super::layout::LayoutConfig,
    ) -> Vec<Option<String>> {
        items
            .iter()
            .enumerate()
            .map(|(idx, item)| {
                let new = if self.has_data[idx] {
                    layout.format_list_item_line(item)
                } else {
                    layout.render_skeleton_row(item).render()
                };
                if self.last[idx] == new {
                    None
                } else {
                    self.last[idx] = new.clone();
                    Some(new)
                }
            })
            .collect()
    }
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

/// Collect worktree data with optional progressive rendering.
///
/// When `show_progress` is true, renders a skeleton immediately and updates as data arrives.
/// When false, behavior depends on `render_table`:
/// - If `render_table` is true: renders final table (buffered mode)
/// - If `render_table` is false: returns data without rendering (JSON mode)
pub fn collect(
    repo: &Repository,
    show_config: ShowConfig,
    show_progress: bool,
    render_table: bool,
) -> anyhow::Result<Option<super::model::ListData>> {
    worktrunk::shell_exec::trace_instant("List collect started");

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
    let worktrees_cell: OnceCell<anyhow::Result<Vec<WorktreeInfo>>> = OnceCell::new();
    let default_branch_cell: OnceCell<Option<String>> = OnceCell::new();
    let url_template_cell: OnceCell<Option<String>> = OnceCell::new();

    rayon::scope(|s| {
        s.spawn(|_| {
            let _ = worktrees_cell.set(repo.list_worktrees());
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
    let worktrees = worktrees_cell
        .into_inner()
        .unwrap()
        .context("Failed to list worktrees")?;
    if worktrees.is_empty() {
        return Ok(None);
    }
    let default_branch = default_branch_cell.into_inner().unwrap();
    let url_template = url_template_cell.into_inner().unwrap();

    // Resolve show flags: merge CLI overrides with config (warmed in parallel phase)
    let (
        show_branches,
        show_remotes,
        skip_tasks,
        command_timeout,
        collect_deadline,
        list_width,
        progressive_handler,
    ) = match show_config {
        ShowConfig::Resolved {
            show_branches,
            show_remotes,
            skip_tasks,
            command_timeout,
            collect_deadline,
            list_width,
            progressive_handler,
        } => (
            show_branches,
            show_remotes,
            skip_tasks,
            command_timeout,
            collect_deadline,
            list_width,
            progressive_handler,
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
            let skip_tasks: HashSet<TaskKind> = if show_full {
                HashSet::new()
            } else {
                [
                    TaskKind::BranchDiff,
                    TaskKind::CiStatus,
                    TaskKind::SummaryGenerate,
                ]
                .into_iter()
                .collect()
            };
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
                skip_tasks,
                command_timeout,
                collect_deadline,
                None,
                None,
            )
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
    let worktree_branches = worktree_branch_set(&worktrees);
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

    // Filter local branches to those without worktrees (CPU-only, no git commands)
    let branches_without_worktrees: Vec<(String, String)> = if show_branches {
        fetched_local
            .unwrap_or(&[])
            .iter()
            .filter(|b| !worktree_branches.contains(b.name.as_str()))
            .map(|b| (b.name.clone(), b.commit_sha.clone()))
            .collect()
    } else {
        Vec::new()
    };

    if warn_stale_default && let Some(branch) = default_branch.as_deref() {
        eprintln!(
            "{}",
            warning_message(cformat!(
                "Configured default branch <bold>{branch}</> does not exist locally"
            ))
        );
        eprintln!(
            "{}",
            hint_message(cformat!(
                "To reset, run <underline>wt config state default-branch clear</>"
            ))
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
    // from worktrees + branches. Populates `repo.cache.commit_details` so
    // post-skeleton `CommitDetailsTask` hits the cache instead of spawning
    // `git log -1` per row.
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
    let timestamps: std::collections::HashMap<String, i64> = repo
        .commit_details_many(&all_shas)
        .unwrap_or_default()
        .into_iter()
        .map(|(sha, (ts, _))| (sha, ts))
        .collect();

    // Sort worktrees: current first, main second, then by timestamp descending
    let sorted_worktrees = sort_worktrees_with_cache(
        worktrees.clone(),
        &main_worktree,
        current_worktree_path.as_ref(),
        &timestamps,
    );

    // Sort branches by timestamp (most recent first)
    let branches_without_worktrees =
        sort_by_timestamp_desc_with_cache(branches_without_worktrees, &timestamps, |(_, sha)| {
            sha.as_str()
        });
    let remote_branches =
        sort_by_timestamp_desc_with_cache(remote_branches, &timestamps, |(_, sha)| sha.as_str());

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
                display: DisplayFields::default(),
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
            .map(|(name, sha)| ListItem::new_branch(sha.clone(), name.clone())),
    );

    // If no URL template configured, add UrlStatus to skip_tasks
    let mut effective_skip_tasks = skip_tasks.clone();
    if url_template.is_none() {
        effective_skip_tasks.insert(TaskKind::UrlStatus);
    }

    // Skip SummaryGenerate unless summary is enabled and an LLM command is configured
    let config = repo.config();
    let llm_command = config.commit_generation.command.clone();
    if !config.list.summary() || llm_command.is_none() {
        effective_skip_tasks.insert(TaskKind::SummaryGenerate);
    }

    // Calculate layout from items (worktrees, local branches, and remote branches).
    // The picker passes an explicit width because the list only gets part of the
    // terminal — the rest belongs to the preview pane.
    let layout = match list_width {
        Some(width) => super::layout::calculate_layout_with_width(
            &all_items,
            &effective_skip_tasks,
            width,
            &main_worktree.path,
            url_template.as_deref(),
        ),
        None => super::layout::calculate_layout_from_basics(
            &all_items,
            &effective_skip_tasks,
            &main_worktree.path,
            url_template.as_deref(),
        ),
    };

    // Single-line invariant: use safe width to prevent line wrapping
    let max_width = crate::display::terminal_width();

    // Create collection options from skip set. `integration_target` is
    // patched in after the parallel phase below extracts it — at this
    // point we haven't yet resolved it, but task spawning doesn't happen
    // until line 1090+ so late population is safe.
    let mut options = CollectOptions {
        skip_tasks: effective_skip_tasks,
        url_template: url_template.clone(),
        llm_command,
        default_branch: default_branch.clone(),
        integration_target: None,
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

    // Create progressive table if showing progress.
    //
    // Skeleton renders with `PLACEHOLDER_BLANK` (space) so commands that finish
    // under ~200ms never flash the `·` loading indicator. After
    // `PLACEHOLDER_REVEAL_DELAY` the placeholder is promoted to `·` via the
    // drain tick below.
    let mut progressive_table = if show_progress {
        layout.placeholder.set(super::render::PLACEHOLDER_BLANK);

        let dim = Style::new().dimmed();

        // Build skeleton rows for both worktrees and branches
        // All items need skeleton rendering since computed data (timestamp, ahead/behind, etc.)
        // hasn't been loaded yet. Using format_list_item_line would show default values like "55y".
        let skeletons: Vec<String> = all_items
            .iter()
            .map(|item| layout.render_skeleton_row(item).render())
            .collect();

        let initial_footer = format!("{INFO_SYMBOL} {dim}{footer_base} (loading...){dim:#}");

        let mut table = ProgressiveTable::new(
            layout.format_header_line(),
            skeletons,
            initial_footer,
            max_width,
        );
        table.render_skeleton()?;
        worktrunk::shell_exec::trace_instant("Skeleton rendered");
        Some(table)
    } else {
        None
    };

    // Picker mirrors `wt list`'s blank→`·` reveal. The placeholder starts
    // blank so fast completions don't flash loading dots; the Reveal event
    // below promotes it to `·` at 200ms. `show_progress=false` (the picker
    // path today) skips the block above, so set it here unconditionally
    // when a handler is present.
    if progressive_handler.is_some() {
        layout.placeholder.set(super::render::PLACEHOLDER_BLANK);
    }

    // Deliver the skeleton to the picker handler. Rendered strings use the
    // blank placeholder so skim's initial render mirrors the `wt list`
    // pre-reveal look.
    if let Some(handler) = progressive_handler.as_ref() {
        let skeletons: Vec<String> = all_items
            .iter()
            .map(|item| layout.render_skeleton_row(item).render())
            .collect();
        handler.on_skeleton(all_items.clone(), skeletons, layout.render_header_line());
        // Mirror the `wt list` progressive-table marker so `wt-perf phases`
        // sees the same boundary across both commands.
        worktrunk::shell_exec::trace_instant("Skeleton rendered");
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

    // Early exit for benchmarking skeleton render time / time-to-first-output
    if std::env::var_os("WORKTRUNK_SKELETON_ONLY").is_some()
        || std::env::var_os("WORKTRUNK_FIRST_OUTPUT").is_some()
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
    let integration_target_cell: OnceCell<Option<String>> = OnceCell::new();

    rayon::scope(|s| {
        // Previous branch lookup (for gutter symbol)
        s.spawn(|_| {
            let _ = previous_branch_cell.set(repo.switch_previous());
        });

        // Integration target (upstream if ahead of local, else local)
        s.spawn(|_| {
            let _ = integration_target_cell.set(repo.integration_target());
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
    let integration_target = integration_target_cell.into_inner().flatten();

    // Patch integration_target into options now that it's resolved. When
    // default_branch is None (unset or stale), also null it out — tasks
    // otherwise see a target derived from the stale value and emit
    // "ambiguous argument" noise.
    options.integration_target = options
        .default_branch
        .as_ref()
        .and(integration_target.clone());

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

    // Batch-fetch ahead/behind counts for all local branches in a single
    // `git for-each-ref` call. Primes the Repository cache so each
    // `AheadBehindTask` hits the cache instead of spawning its own
    // `git rev-list --count`. One git call replaces N.
    //
    // Note: `resolved_refs`, `commit_shas`, and upstream tracking info are
    // already primed by `local_branches()` (called during pre-skeleton
    // phase), so `Branch::upstream()` is an in-memory lookup from here on.
    //
    // On git < 2.36 (no `%(ahead-behind:)` support) or if default_branch is
    // unknown, skip the batch — individual tasks fall back to direct calls.
    if let Some(ref db) = default_branch {
        repo.batch_ahead_behind(db);
    }

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
    worktrunk::shell_exec::trace_instant("Spawning worker thread");
    std::thread::spawn(move || {
        worktrunk::shell_exec::trace_instant("Parallel execution started");
        all_work_items.into_par_iter().for_each(|item| {
            worktrunk::shell_exec::set_command_timeout(command_timeout);
            let result = item.execute();
            let _ = tx_worker.send(result);
        });
    });

    // Drop the original sender so drain_results knows when all spawned threads are done
    drop(tx);

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
    let mut row_cache = RowCache::new(n_items);

    let drain_deadline =
        collect_deadline.unwrap_or_else(|| std::time::Instant::now() + results::DRAIN_TIMEOUT);

    // Reveal fires only when a downstream consumer is listening.
    let reveal_at = (progressive_state.is_some() || progressive_handler.is_some())
        .then_some(placeholder_reveal_at);

    let drain_outcome = drain_results(
        rx,
        &mut all_items,
        &mut errors,
        &expected_results,
        drain_deadline,
        integration_target.as_deref(),
        |event| {
            let dim = Style::new().dimmed();
            let total_results = expected_results.count();

            match event {
                results::DrainEvent::Result { item_idx, item } => {
                    let rendered = layout.format_list_item_line(item);
                    let changed = row_cache.set_result(item_idx, rendered);

                    if let Some(state_cell) = progressive_state.as_ref() {
                        let mut s = state_cell.borrow_mut();
                        if !s.first_result_traced {
                            s.first_result_traced = true;
                            worktrunk::shell_exec::trace_instant("First result received");
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

                        if let Some(line) = &changed {
                            s.table.update_row(item_idx, line.clone());
                        }

                        if let Err(e) = s.table.flush() {
                            log::debug!("Progressive table flush failed: {}", e);
                        }
                    }

                    if let Some(handler) = progressive_handler.as_ref()
                        && let Some(line) = changed
                    {
                        handler.on_update(item_idx, line);
                    }
                }
                results::DrainEvent::Reveal { items } => {
                    layout.placeholder.set(super::render::PLACEHOLDER);
                    let updates = row_cache.set_reveal(items, &layout);

                    if let Some(state_cell) = progressive_state.as_ref() {
                        let mut s = state_cell.borrow_mut();
                        for (idx, update) in updates.iter().enumerate() {
                            if let Some(line) = update {
                                s.table.update_row(idx, line.clone());
                            }
                        }
                        if let Err(e) = s.table.flush() {
                            log::debug!("Progressive table reveal flush failed: {}", e);
                        }
                    }

                    if let Some(handler) = progressive_handler.as_ref() {
                        handler.on_reveal(updates);
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
                            log::debug!("Progressive table flush failed: {}", e);
                        }
                    }
                    // Picker has no stall UI; heartbeat keeps it responsive.
                }
            }
        },
        reveal_at,
    );
    worktrunk::shell_exec::trace_instant("All results drained");

    // Extract progressive state back out. `progressive_table` is re-bound so
    // post-drain code (finalize / error rendering) works unchanged.
    let (progressive_table, progress_overflow) = match progressive_state {
        Some(cell) => {
            let s = cell.into_inner();
            (Some(s.table), s.progress_overflow)
        }
        None => (None, false),
    };
    // Reveal the placeholder synchronously for any path where the drain
    // finished before the reveal could fire — keeps subsequent renders
    // (including `finalize`) consistent with the post-reveal placeholder.
    layout.placeholder.set(super::render::PLACEHOLDER);

    // Handle timeout if it occurred.
    // Budget-based deadlines (collect_deadline) are intentional truncation — don't warn.
    // Only warn for the default DRAIN_TIMEOUT (120s), which indicates a hung command.
    if collect_deadline.is_none()
        && let DrainOutcome::TimedOut {
            received_count,
            items_with_missing,
        } = drain_outcome
    {
        // Warning: what happened + gutter showing which tasks blocked
        let mut diag = format!(
            "wt list timed out after {}s ({received_count} results received)",
            results::DRAIN_TIMEOUT.as_secs()
        );

        if !items_with_missing.is_empty() {
            diag.push_str("; blocked tasks:");
            let missing_lines: Vec<String> = items_with_missing
                .iter()
                .take(5)
                .map(|result| {
                    let missing_names: Vec<&str> =
                        result.missing_kinds.iter().map(|k| k.into()).collect();
                    cformat!("<bold>{}</>: {}", result.name, missing_names.join(", "))
                })
                .collect();
            diag.push_str(&format!(
                "\n{}",
                format_with_gutter(&missing_lines.join("\n"), None)
            ));
        }

        eprintln!("{}", warning_message(&diag));

        eprintln!(
            "{}",
            hint_message(cformat!(
                "A git command likely hung; run <underline>wt list -v</> for details or <underline>wt list -vv</> to create a diagnostic file"
            ))
        );
    }

    // The drain calls `refresh_status_symbols` after every *successful*
    // result, but items with zero successful results (all tasks errored
    // or timed out) never hit that path. Sweep every item so that
    // synchronously-derivable gates (worktree_state from metadata,
    // pre-seeded main_state for unborn/prunable items) still materialize.
    // The call is idempotent — already-resolved gates are skipped.
    for item in all_items.iter_mut() {
        item.refresh_status_symbols(integration_target.as_deref());
    }

    // Count errors for summary
    let error_count = errors.len();
    let timed_out_count = errors.iter().filter(|e| e.is_timeout()).count();

    let table_render = render_table.then(|| TableRenderPlan {
        progressive_table,
        header: layout.format_header_line(),
        rows: all_items
            .iter()
            .map(|item| layout.format_list_item_line(item))
            .collect(),
        summary: super::format_summary_message(
            &all_items,
            show_branches || show_remotes,
            layout.hidden_column_count,
            error_count,
            timed_out_count,
        ),
    });

    if let Some(table_render) = table_render {
        table_render.render()?;
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
        eprintln!("{}", warning_message(&warning));

        // Show issue reporting hint (free function - doesn't collect diagnostic data)
        eprintln!("{}", hint_message(crate::diagnostic::issue_hint()));
    }

    // Populate display fields for all items (used by JSON output and statusline)
    for item in &mut all_items {
        item.finalize_display();
    }

    // all_items now contains both worktrees and branches (if requested)
    let items = all_items;

    // Table rendering complete (when render_table=true):
    // - Progressive + TTY: rows morphed in place, footer became summary
    // - Progressive + Non-TTY: rendered final table (no intermediate output)
    // - Buffered: rendered final table
    // JSON mode (render_table=false): no rendering, data returned for serialization
    worktrunk::shell_exec::trace_instant("List collect complete");

    Ok(Some(super::model::ListData { items }))
}

// ============================================================================
// Sorting Helpers
// ============================================================================

/// Sort items by timestamp descending using pre-fetched timestamps.
fn sort_by_timestamp_desc_with_cache<T, F>(
    items: Vec<T>,
    timestamps: &std::collections::HashMap<String, i64>,
    get_sha: F,
) -> Vec<T>
where
    F: Fn(&T) -> &str,
{
    // Embed timestamp in tuple to avoid parallel Vec and index lookups
    let mut with_ts: Vec<_> = items
        .into_iter()
        .map(|item| {
            let ts = *timestamps.get(get_sha(&item)).unwrap_or(&0);
            (item, ts)
        })
        .collect();
    with_ts.sort_by_key(|(_, ts)| std::cmp::Reverse(*ts));
    with_ts.into_iter().map(|(item, _)| item).collect()
}

/// Sort worktrees: current first, main second, then by timestamp descending.
/// Uses pre-fetched timestamps for efficiency.
fn sort_worktrees_with_cache(
    worktrees: Vec<WorktreeInfo>,
    main_worktree: &WorktreeInfo,
    current_path: Option<&std::path::PathBuf>,
    timestamps: &std::collections::HashMap<String, i64>,
) -> Vec<WorktreeInfo> {
    // Embed timestamp and priority in tuple to avoid parallel Vec and index lookups
    let mut with_sort_key: Vec<_> = worktrees
        .into_iter()
        .map(|wt| {
            let priority = if current_path.is_some_and(|cp| &wt.path == cp) {
                0 // Current first
            } else if wt.path == main_worktree.path {
                1 // Main second
            } else {
                2 // Rest by timestamp
            };
            let ts = *timestamps.get(&wt.head).unwrap_or(&0);
            (wt, priority, ts)
        })
        .collect();

    with_sort_key.sort_by_key(|(_, priority, ts)| (*priority, std::cmp::Reverse(*ts)));
    with_sort_key.into_iter().map(|(wt, _, _)| wt).collect()
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
        display: DisplayFields::default(),
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
    // Extract worktree data (skip if not a worktree item)
    let Some(data) = item.worktree_data() else {
        return Ok(());
    };

    // Get integration target for status symbol computation (cached in repo)
    // None if default branch cannot be determined - status symbols will be skipped
    let target = repo.integration_target();

    // Populate default_branch / integration_target if the caller didn't.
    // Tasks read these through `TaskContext`; `None` here tells them to
    // skip (see collect()'s stale-default-branch path). Single-item callers
    // like statusline pass `CollectOptions::default()` and expect the
    // repo-derived values.
    if options.default_branch.is_none() {
        options.default_branch = repo.default_branch();
    }
    if options.integration_target.is_none() {
        options.integration_target = target.clone();
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

    // Spawn collection in background thread (executes only)
    std::thread::spawn(move || {
        work_items.into_par_iter().for_each(|w| {
            let result = w.execute();
            let _ = tx.send(result);
        });
    });

    // Drain task results (blocking until complete). `drain_results`
    // writes each result onto the item and calls `compute_status_symbols`
    // after every write, so the callback here is just a no-op — there is
    // no progressive table to refresh on the statusline path.
    let drain_outcome = drain_results(
        rx,
        std::slice::from_mut(item),
        &mut errors,
        &expected_results,
        std::time::Instant::now() + results::DRAIN_TIMEOUT,
        target.as_deref(),
        |_event| {},
        None,
    );

    // Handle timeout (silent for statusline - just log it)
    if let DrainOutcome::TimedOut { received_count, .. } = drain_outcome {
        log::warn!(
            "populate_item timed out after {}s ({received_count} results received)",
            results::DRAIN_TIMEOUT.as_secs()
        );
    }

    // Log errors silently (statusline shouldn't spam warnings)
    if !errors.is_empty() {
        log::warn!("populate_item had {} task errors", errors.len());
        for error in &errors {
            let kind_str: &'static str = error.kind.into();
            log::debug!(
                "  - item {}: {} ({})",
                error.item_idx,
                kind_str,
                error.message
            );
        }
    }

    // Ensure status symbols are refreshed even if all tasks errored
    // (the drain only calls refresh on the success path).
    item.refresh_status_symbols(target.as_deref());

    // Populate display fields (including status_line for statusline command)
    item.finalize_display();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Strip ANSI escape sequences so snapshots read as plain text.
    fn strip_ansi(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '\x1b' {
                out.push(c);
                continue;
            }
            // CSI: ESC [ ... (letter terminator)
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        }
        out
    }

    #[test]
    fn test_format_stall_footer_single_pending() {
        let rendered =
            format_stall_footer("Showing 3 worktrees", 5, 12, 1, TaskKind::CiStatus, "feat");
        insta::assert_snapshot!(
            strip_ansi(&rendered),
            @"○ Showing 3 worktrees (5/12 loaded, no recent progress; waiting on ci-status for feat)"
        );
    }

    #[test]
    fn test_format_stall_footer_many_pending() {
        let rendered =
            format_stall_footer("Showing 3 worktrees", 5, 12, 3, TaskKind::CiStatus, "feat");
        insta::assert_snapshot!(
            strip_ansi(&rendered),
            @"○ Showing 3 worktrees (5/12 loaded, no recent progress; waiting on 3 tasks, including ci-status for feat)"
        );
    }

    /// `set_result` marks the row as having data and dedups by comparing
    /// against the cached render; `set_reveal` picks skeleton-vs-format
    /// per row based on `has_data` and also dedups. These two behaviors
    /// are load-bearing for the picker's partial-row reveal correctness
    /// (see the RowCache doc comment).
    #[test]
    fn test_row_cache_dedup_and_reveal() {
        use super::super::layout::calculate_layout_with_width;
        use super::super::model::ListItem;
        use std::collections::HashSet;
        use std::path::Path;

        let items = vec![
            ListItem::new_branch("aaa".into(), "row-zero".into()),
            ListItem::new_branch("bbb".into(), "row-one".into()),
        ];
        let skip_tasks: HashSet<TaskKind> = HashSet::new();
        let layout = calculate_layout_with_width(&items, &skip_tasks, 80, Path::new("/tmp"), None);

        let mut cache = RowCache::new(2);

        // First set_result: cache was empty, so the new line is emitted.
        let first = cache.set_result(0, "row-zero-line-v1".into());
        assert_eq!(first.as_deref(), Some("row-zero-line-v1"));

        // Same render again → dedup: None.
        let dup = cache.set_result(0, "row-zero-line-v1".into());
        assert_eq!(dup, None);

        // Different render → Some again.
        let changed = cache.set_result(0, "row-zero-line-v2".into());
        assert_eq!(changed.as_deref(), Some("row-zero-line-v2"));

        // set_reveal after the placeholder flip. Row 0 has data: use
        // format_list_item_line; the result is different from the cached
        // synthetic string above so it's emitted as Some. Row 1 has no
        // data: use render_skeleton_row; cache was empty so it's emitted.
        layout.placeholder.set(super::super::render::PLACEHOLDER);
        let updates = cache.set_reveal(&items, &layout);
        assert_eq!(updates.len(), 2);
        assert!(
            updates[0].is_some(),
            "row 0 had data but cached string was synthetic; reveal must emit new render"
        );
        assert!(
            updates[1].is_some(),
            "row 1 had no data; reveal must emit skeleton render"
        );

        // Second reveal with no intervening changes: both rows dedup to None.
        let updates2 = cache.set_reveal(&items, &layout);
        assert_eq!(updates2, vec![None, None]);
    }
}
