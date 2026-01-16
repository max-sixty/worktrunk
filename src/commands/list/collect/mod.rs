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
//! - **Batching** — timestamp fetch passes all SHAs to one `git show` command
//! - **Parallelization** — independent commands run concurrently via `join!` macro
//!
//! **Steady-state (5-7 commands):**
//!
//! | Command | Purpose | Parallel |
//! |---------|---------|----------|
//! | `git worktree list --porcelain` | Enumerate worktrees | Sequential (required first) |
//! | `git config worktrunk.default-branch` | Cached default branch | ✓ |
//! | `git config --bool core.bare` | Bare repo check for expected-path logic | ✓ |
//! | `git rev-parse --show-toplevel` | Worktree root for project config | ✓ |
//! | `git for-each-ref refs/heads` | Only with `--branches` flag | ✓ |
//! | `git for-each-ref refs/remotes` | Only with `--remotes` flag | ✓ |
//! | `git show -s --format='%H %ct' SHA1 SHA2 ...` | **Batched** timestamps | Sequential (needs SHAs) |
//!
//! **Non-git operations (negligible latency):**
//! - Path canonicalization — detect current worktree
//! - Project config file read — check if URL column needed (no template expansion)
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
//! │    ├─ get_switch_previous()                 (5ms)
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
//! **Invalid default branch warning:** `invalid_default_branch_config()` reads the value
//! cached by `default_branch()` during pre-skeleton. It's a pure cache read.
//!
//! When adding new features, ask: "Can this be computed after skeleton?" If yes, defer it.
//! The skeleton shows `·` placeholder for gutter symbols, filled in when data loads.
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

mod execution;
mod results;
mod tasks;
mod types;

use anyhow::Context;
use color_print::cformat;
use crossbeam_channel as chan;
use dunce::canonicalize;
use once_cell::sync::OnceCell;
use rayon::prelude::*;
use std::sync::Arc;
use worktrunk::git::{Repository, WorktreeInfo};
use worktrunk::styling::{INFO_SYMBOL, format_with_gutter, hint_message, warning_message};

use crate::commands::is_worktree_at_expected_path;

use super::model::{DisplayFields, ItemKind, ListItem, WorktreeData};

// Re-exports for sibling modules (columns.rs, render.rs, layout.rs)
pub(crate) use tasks::parse_port_from_url;
pub(crate) use types::TaskKind;

// Internal imports
pub(crate) use execution::ExpectedResults;
use execution::{work_items_for_branch, work_items_for_worktree};
use results::drain_results;
use types::{DrainOutcome, StatusContext};
use types::{TaskError, TaskResult};

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

    /// Branches to skip expensive tasks for (behind > threshold).
    ///
    /// Presence in set = skip expensive tasks for this branch (HasFileChanges,
    /// IsAncestor, WouldMergeAdd, BranchDiff, MergeTreeConflicts).
    ///
    /// ## Why "commits behind" as the heuristic
    ///
    /// The expensive operations (`git merge-tree`, `git diff`) scale with:
    /// - **Files changed on both sides** — each needs 3-way merge or diff
    /// - **Size of those files** — content loading and merge algorithm
    ///
    /// Commit count isn't directly in the algorithm, but "commits behind" is a
    /// cheap proxy: more commits on main since divergence → more files main has
    /// touched → more potential overlap with the branch's changes.
    ///
    /// We use "behind" rather than "ahead" because feature branches typically
    /// have small ahead counts, so behind dominates. A more accurate heuristic
    /// would be `min(files_changed_on_main, files_changed_on_branch)`, but
    /// computing that requires per-branch git commands, defeating the optimization.
    ///
    /// The batch `git for-each-ref --format='%(ahead-behind:...)'` gives us all
    /// counts in a single command, making this heuristic essentially free.
    ///
    /// ## Implementation
    ///
    /// Built by filtering `batch_ahead_behind()` results on local branches only.
    /// Remote-only branches are never in this set (they use individual git commands).
    /// The threshold (default 50) is applied at construction time. Ahead/behind
    /// counts are cached in Repository and looked up by AheadBehindTask.
    ///
    /// **Display implications:** When tasks are skipped:
    /// - BranchDiff column shows `…` instead of diff stats
    /// - Status symbols (conflict `✗`, integrated `⊂`) may be missing or incorrect
    ///   since they depend on skipped tasks
    ///
    /// Note: `wt select` doesn't show the BranchDiff column, so `…` isn't visible there.
    /// This is similar to how `✗` conflict only shows with `--full` even in `wt list`.
    ///
    /// TODO: Consider adding a visible indicator in Status column when integration
    /// checks are skipped, so users know the `⊂` symbol may be incomplete.
    pub stale_branches: std::collections::HashSet<String>,
}

fn worktree_branch_set(worktrees: &[WorktreeInfo]) -> std::collections::HashSet<&str> {
    worktrees
        .iter()
        .filter_map(|wt| wt.branch.as_deref())
        .collect()
}

/// Collect worktree data with optional progressive rendering.
///
/// When `show_progress` is true, renders a skeleton immediately and updates as data arrives.
/// When false, behavior depends on `render_table`:
/// - If `render_table` is true: renders final table (buffered mode)
/// - If `render_table` is false: returns data without rendering (JSON mode)
///
/// The `command_timeout` parameter, if set, limits how long individual git commands can run.
/// This is useful for `wt select` to show the TUI faster by skipping slow operations.
///
/// TODO: Now that we skip expensive tasks for stale branches (see `skip_expensive_for_stale`),
/// the timeout may be unnecessary. Consider removing it if it doesn't provide value.
///
/// The `skip_expensive_for_stale` parameter enables batch-fetching ahead/behind counts and
/// skipping expensive merge-base operations for branches far behind the default branch.
/// This dramatically improves performance for repos with many stale branches.
#[allow(clippy::too_many_arguments)]
pub fn collect(
    repo: &Repository,
    show_branches: bool,
    show_remotes: bool,
    skip_tasks: &std::collections::HashSet<TaskKind>,
    show_progress: bool,
    render_table: bool,
    config: &worktrunk::config::WorktrunkConfig,
    command_timeout: Option<std::time::Duration>,
    skip_expensive_for_stale: bool,
) -> anyhow::Result<Option<super::model::ListData>> {
    use super::progressive_table::ProgressiveTable;
    worktrunk::shell_exec::trace_instant("List collect started");

    // Phase 1: Parallel fetch of ALL independent git data
    //
    // Key insight: most operations don't depend on each other. By running them all
    // in parallel via rayon::scope, we minimize wall-clock time. Dependencies:
    //
    // - worktree list: independent (needed for filtering and SHAs)
    // - default_branch: independent (git config + verify)
    // - is_bare: independent (git config, cached for later use)
    // - url_template: independent (loads project config via show-toplevel)
    // - local_branches: independent (for-each-ref, but filtering needs worktrees)
    // - remote_branches: independent (for-each-ref)
    //
    // After this scope completes, we have all raw data and can do CPU-only work.
    let worktrees_cell: OnceCell<anyhow::Result<Vec<WorktreeInfo>>> = OnceCell::new();
    let default_branch_cell: OnceCell<Option<String>> = OnceCell::new();
    let url_template_cell: OnceCell<Option<String>> = OnceCell::new();
    let local_branches_cell: OnceCell<anyhow::Result<Vec<(String, String)>>> = OnceCell::new();
    let remote_branches_cell: OnceCell<anyhow::Result<Vec<(String, String)>>> = OnceCell::new();

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
            if show_branches {
                let _ = local_branches_cell.set(repo.list_local_branches());
            }
        });
        s.spawn(|_| {
            if show_remotes {
                let _ = remote_branches_cell.set(repo.list_untracked_remote_branches());
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

    // Filter local branches to those without worktrees (CPU-only, no git commands)
    let branches_without_worktrees = if show_branches {
        let all_local = local_branches_cell.into_inner().unwrap()?;
        let worktree_branches = worktree_branch_set(&worktrees);
        all_local
            .into_iter()
            .filter(|(name, _)| !worktree_branches.contains(name.as_str()))
            .collect()
    } else {
        Vec::new()
    };
    let remote_branches = if show_remotes {
        remote_branches_cell.into_inner().unwrap()?
    } else {
        Vec::new()
    };

    // Detect current worktree by checking if repo path is inside any worktree.
    // This avoids a git command - we just compare canonicalized paths.
    let repo_path_canonical = canonicalize(repo.discovery_path()).ok();
    let current_worktree_path = repo_path_canonical.as_ref().and_then(|repo_path| {
        worktrees.iter().find_map(|wt| {
            canonicalize(&wt.path)
                .ok()
                .filter(|wt_path| repo_path.starts_with(wt_path))
        })
    });
    // Show warning if user configured a default branch that doesn't exist locally
    if let Some(configured) = repo.invalid_default_branch_config() {
        let msg =
            cformat!("Configured default branch <bold>{configured}</> does not exist locally");
        crate::output::print(warning_message(msg))?;
        let hint = cformat!("To reset, run <bright-black>wt config state default-branch clear</>");
        crate::output::print(hint_message(hint))?;
    }

    // Main worktree is the worktree on the default branch (if exists), else first non-prunable worktree.
    // find_home returns None if all worktrees are prunable or the list is empty.
    // TODO: show ellipsis or indicator when default_branch is None and columns are empty
    let main_worktree =
        WorktreeInfo::find_home(&worktrees, default_branch.as_deref().unwrap_or(""))
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("No worktrees found"))?;

    // Defer previous_branch lookup until after skeleton - set is_previous later
    // (skeleton shows placeholder gutter, actual symbols appear when data loads)

    // Phase 3: Batch fetch timestamps (needs all SHAs from worktrees + branches)
    let all_shas: Vec<&str> = worktrees
        .iter()
        .map(|wt| wt.head.as_str())
        .chain(
            branches_without_worktrees
                .iter()
                .map(|(_, sha)| sha.as_str()),
        )
        .chain(remote_branches.iter().map(|(_, sha)| sha.as_str()))
        .collect();
    let timestamps = repo.commit_timestamps(&all_shas).unwrap_or_default();

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
            let branch_worktree_mismatch = !is_worktree_at_expected_path(wt, repo, config);

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
                is_ancestor: None,
                is_orphan: None,
                upstream: None,
                pr_status: None,
                url: None,
                url_active: None,
                status_symbols: None,
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

    // Calculate layout from items (worktrees, local branches, and remote branches)
    let layout = super::layout::calculate_layout_from_basics(
        &all_items,
        &effective_skip_tasks,
        &main_worktree.path,
        url_template.as_deref(),
    );

    // Single-line invariant: use safe width to prevent line wrapping
    let max_width = crate::display::get_terminal_width();

    // Create collection options from skip set
    let mut options = CollectOptions {
        skip_tasks: effective_skip_tasks,
        url_template: url_template.clone(),
        ..Default::default()
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

    // Create progressive table if showing progress
    let mut progressive_table = if show_progress {
        use anstyle::Style;
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

    // Early exit for benchmarking skeleton render time
    if std::env::var("WORKTRUNK_SKELETON_ONLY").is_ok() {
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
            let _ = previous_branch_cell.set(repo.get_switch_previous());
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

    // Batch-fetch ahead/behind counts to identify branches that are far behind.
    // This allows skipping expensive merge-base operations for diverged branches, dramatically
    // improving performance on repos with many stale branches (e.g., wt select).
    //
    // Uses `git for-each-ref --format='%(ahead-behind:...)'` (git 2.36+) which gets all
    // counts in a single command. On older git versions, returns empty and all tasks run.
    // Skip if default_branch is unknown.
    if skip_expensive_for_stale && let Some(ref db) = default_branch {
        // Branches more than 50 commits behind skip expensive operations.
        // 50 is low enough to catch truly stale branches while keeping info for
        // recently-diverged ones.
        //
        // "Behind" is a proxy for the actual cost driver: files changed on both
        // sides since the merge-base. More commits on main → more files touched →
        // more overlap with the branch. See `CollectOptions::stale_branches` for
        // detailed rationale.
        let threshold: usize = std::env::var("WORKTRUNK_TEST_SKIP_EXPENSIVE_THRESHOLD")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(50);
        // batch_ahead_behind populates the Repository cache with all counts
        let ahead_behind = repo.batch_ahead_behind(db);
        // Filter to stale branches (behind > threshold). The set indicates which
        // branches should skip expensive tasks; counts come from the cache.
        options.stale_branches = ahead_behind
            .into_iter()
            .filter_map(|(branch, (_, behind))| (behind > threshold).then_some(branch))
            .collect();
    }

    // Note: URL template expansion is deferred to task spawning (in collect_worktree_progressive
    // and collect_branch_progressive). This parallelizes the work and minimizes time-to-skeleton.

    // Cache last rendered (unclamped) message per row to avoid redundant updates.
    let mut last_rendered_lines: Vec<String> = vec![String::new(); all_items.len()];

    // Create channel for task results
    let (tx, rx) = chan::unbounded::<Result<TaskResult, TaskError>>();

    // Collect errors for display after rendering
    let mut errors: Vec<TaskError> = Vec::new();

    // Collect all work items upfront, then execute in a single Rayon pool.
    // This avoids nested parallelism (Rayon par_iter → thread::scope per worktree)
    // which could create 100+ threads. Instead, we have one pool with the configured
    // thread count (default 2x CPU cores unless overridden by RAYON_NUM_THREADS).
    let sorted_worktrees_clone = sorted_worktrees.clone();
    let tx_worker = tx.clone();
    let expected_results_clone = expected_results.clone();

    // Clone repo for the worker thread (shares cache via Arc)
    let repo_clone = repo.clone();

    // Prepare branch data if needed (before moving into closure)
    let branch_data: Vec<(usize, String, String)> = if show_branches || show_remotes {
        let mut all_branches = Vec::new();
        if show_branches {
            all_branches.extend(
                branches_without_worktrees
                    .iter()
                    .enumerate()
                    .map(|(idx, (name, sha))| (branch_start_idx + idx, name.clone(), sha.clone())),
            );
        }
        if show_remotes {
            all_branches.extend(
                remote_branches
                    .iter()
                    .enumerate()
                    .map(|(idx, (name, sha))| (remote_start_idx + idx, name.clone(), sha.clone())),
            );
        }
        all_branches
    } else {
        Vec::new()
    };

    worktrunk::shell_exec::trace_instant("Spawning worker thread");
    std::thread::spawn(move || {
        // Phase 1: Generate all work items (sequential, fast)
        // Work items are collected upfront so we can process them all in a single par_iter.
        let mut all_work_items = Vec::new();

        // Worktree work items
        for (idx, wt) in sorted_worktrees_clone.iter().enumerate() {
            all_work_items.extend(work_items_for_worktree(
                &repo_clone,
                wt,
                idx,
                &options,
                &expected_results_clone,
                &tx_worker,
            ));
        }

        // Branch work items (local + remote)
        for (item_idx, branch_name, commit_sha) in &branch_data {
            all_work_items.extend(work_items_for_branch(
                &repo_clone,
                branch_name,
                commit_sha,
                *item_idx,
                &options,
                &expected_results_clone,
            ));
        }

        // Sort work items: network tasks last to avoid blocking local operations
        all_work_items.sort_by_key(|item| item.kind.is_network());

        // Phase 2: Execute all work items in parallel
        worktrunk::shell_exec::trace_instant("Parallel execution started");
        all_work_items.into_par_iter().for_each(|item| {
            worktrunk::shell_exec::set_command_timeout(command_timeout);
            let result = item.execute();
            let _ = tx_worker.send(result);
        });
    });

    // Drop the original sender so drain_results knows when all spawned threads are done
    drop(tx);

    // Track completed results for footer progress
    let mut completed_results = 0;
    let mut progress_overflow = false;
    let mut first_result_traced = false;

    // Drain task results with conditional progressive rendering
    let drain_outcome = drain_results(
        rx,
        &mut all_items,
        &mut errors,
        &expected_results,
        |item_idx, item, ctx| {
            // Trace first result arrival
            if !first_result_traced {
                first_result_traced = true;
                worktrunk::shell_exec::trace_instant("First result received");
            }

            // Compute/recompute status symbols as data arrives (both modes).
            // This is idempotent and updates status as new data (like upstream) arrives.
            if let Some(ref target) = integration_target {
                ctx.apply_to(item, target.as_str());
            }

            // Progressive mode only: update UI
            if let Some(ref mut table) = progressive_table {
                use anstyle::Style;
                let dim = Style::new().dimmed();

                completed_results += 1;
                let total_results = expected_results.count();

                // Catch counting bugs: completed should never exceed expected
                debug_assert!(
                    completed_results <= total_results,
                    "completed ({completed_results}) > expected ({total_results}): \
                     task result sent without registering expectation"
                );
                if completed_results > total_results {
                    progress_overflow = true;
                }

                // Update footer progress
                let footer_msg = format!(
                    "{INFO_SYMBOL} {dim}{footer_base} ({completed_results}/{total_results} loaded){dim:#}"
                );
                table.update_footer(footer_msg);

                // Re-render the row with caching (now includes status if computed)
                let rendered = layout.format_list_item_line(item);

                // Compare using full line so changes beyond the clamp (e.g., CI) still refresh.
                if rendered != last_rendered_lines[item_idx] {
                    last_rendered_lines[item_idx] = rendered.clone();
                    table.update_row(item_idx, rendered);
                }

                // Flush updates to terminal
                if let Err(e) = table.flush() {
                    log::debug!("Progressive table flush failed: {}", e);
                }
            }
        },
    );
    worktrunk::shell_exec::trace_instant("All results drained");

    // Handle timeout if it occurred
    if let DrainOutcome::TimedOut {
        received_count,
        items_with_missing,
    } = drain_outcome
    {
        // Build diagnostic message showing what's MISSING (more useful for debugging)
        let mut diag = format!("wt list timed out after 30s ({received_count} results received)");

        if !items_with_missing.is_empty() {
            diag.push_str("\nMissing results:");
            let missing_lines: Vec<String> = items_with_missing
                .iter()
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

        diag.push_str(
            "\n\nThis likely indicates a git command hung. Run with -v for details, -vv to create a diagnostic file.",
        );

        crate::output::print(warning_message(&diag))?;

        // Show issue reporting hint (free function - doesn't collect diagnostic data)
        crate::output::print(hint_message(crate::diagnostic::issue_hint()))?;
    }

    // Compute status symbols for prunable worktrees (skipped during task spawning).
    // They didn't receive any task results, so status_symbols is still None.
    for item in &mut all_items {
        if item.status_symbols.is_none()
            && let Some(data) = item.worktree_data()
            && data.is_prunable()
        {
            // Use default context - no tasks ran, so no conflict/status info
            let ctx = StatusContext::default();
            if let Some(ref target) = integration_target {
                ctx.apply_to(item, target.as_str());
            }
        }
    }

    // Count errors for summary
    let error_count = errors.len();
    let timed_out_count = errors.iter().filter(|e| e.is_timeout()).count();

    // Finalize progressive table or render buffered output
    if let Some(mut table) = progressive_table {
        // Build final summary string
        let final_msg = super::format_summary_message(
            &all_items,
            show_branches || show_remotes,
            layout.hidden_column_count,
            error_count,
            timed_out_count,
        );

        if table.is_tty() {
            // Interactive: do final render pass and update footer to summary
            for (item_idx, item) in all_items.iter().enumerate() {
                let rendered = layout.format_list_item_line(item);
                table.update_row(item_idx, rendered);
            }
            table.finalize(final_msg)?;
        } else {
            // Non-TTY: output to stdout (same as buffered mode)
            // Progressive skeleton was suppressed; now output the final table
            crate::output::stdout(layout.format_header_line())?;
            for item in &all_items {
                crate::output::stdout(layout.format_list_item_line(item))?;
            }
            crate::output::stdout("")?;
            crate::output::stdout(final_msg)?;
        }
    } else if render_table {
        // Buffered mode: render final table
        let final_msg = super::format_summary_message(
            &all_items,
            show_branches || show_remotes,
            layout.hidden_column_count,
            error_count,
            timed_out_count,
        );

        crate::output::stdout(layout.format_header_line())?;
        for item in &all_items {
            crate::output::stdout(layout.format_list_item_line(item))?;
        }
        crate::output::stdout("")?;
        crate::output::stdout(final_msg)?;
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
        crate::output::print(warning_message(&warning))?;

        // Show issue reporting hint (free function - doesn't collect diagnostic data)
        crate::output::print(hint_message(crate::diagnostic::issue_hint()))?;
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

    Ok(Some(super::model::ListData {
        items,
        main_worktree_path: main_worktree.path.clone(),
    }))
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
        is_ancestor: None,
        is_orphan: None,
        upstream: None,
        pr_status: None,
        url: None,
        url_active: None,
        status_symbols: None,
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
    options: CollectOptions,
) -> anyhow::Result<()> {
    // Extract worktree data (skip if not a worktree item)
    let Some(data) = item.worktree_data() else {
        return Ok(());
    };

    // Get integration target for status symbol computation (cached in repo)
    // None if default branch cannot be determined - status symbols will be skipped
    let target = repo.integration_target();

    // Create channel for task results
    let (tx, rx) = chan::unbounded::<Result<TaskResult, TaskError>>();

    // Track expected results (populated at spawn time)
    let expected_results = Arc::new(ExpectedResults::default());

    // Collect errors (logged silently for statusline)
    let mut errors: Vec<TaskError> = Vec::new();

    // Extract data for background thread (can't send borrows across threads)
    let wt = WorktreeInfo {
        path: data.path.clone(),
        head: item.head.clone(),
        branch: item.branch.clone(),
        bare: false,
        detached: false,
        locked: None,
        prunable: None,
    };
    let repo_clone = repo.clone();
    let expected_results_clone = expected_results.clone();

    // Spawn collection in background thread
    std::thread::spawn(move || {
        // Generate work items for this single worktree
        let mut work_items = work_items_for_worktree(
            &repo_clone,
            &wt,
            0, // Single item, always index 0
            &options,
            &expected_results_clone,
            &tx,
        );

        // Sort: network tasks last
        work_items.sort_by_key(|item| item.kind.is_network());

        // Execute all tasks in parallel
        work_items.into_par_iter().for_each(|item| {
            let result = item.execute();
            let _ = tx.send(result);
        });
    });

    // Drain task results (blocking until complete)
    let drain_outcome = drain_results(
        rx,
        std::slice::from_mut(item),
        &mut errors,
        &expected_results,
        |_item_idx, item, ctx| {
            if let Some(ref t) = target {
                ctx.apply_to(item, t);
            }
        },
    );

    // Handle timeout (silent for statusline - just log it)
    if let DrainOutcome::TimedOut { received_count, .. } = drain_outcome {
        log::warn!("populate_item timed out after 30s ({received_count} results received)");
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

    // Populate display fields (including status_line for statusline command)
    item.finalize_display();

    Ok(())
}
