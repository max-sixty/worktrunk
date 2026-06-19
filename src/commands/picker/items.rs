//! Skim item implementations.
//!
//! Wrappers for ListItem and header row that implement SkimItem for the interactive selector.

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use anstyle::Reset;
use color_print::cformat;
use dashmap::DashMap;
use skim::prelude::*;
use worktrunk::git::Repository;
use worktrunk::styling::INFO_SYMBOL;

use super::super::list::model::ListItem;
use super::log_formatter::{
    FIELD_DELIM, batch_fetch_stats, format_log_output, process_log_with_dimming, strip_hash_markers,
};
use super::pager::{diff_pager, pipe_through_pager};
use super::preview::{PreviewMode, PreviewStateData};
use super::preview_cache;

/// Cache key for pre-computed previews: (branch_name, mode).
pub(super) type PreviewCacheKey = (String, PreviewMode);

/// Cache for pre-computed previews, keyed by (branch_name, mode).
/// Shared across all WorktreeSkimItems for background pre-computation.
pub(super) type PreviewCache = Arc<DashMap<PreviewCacheKey, String>>;

/// Prefix on a worktree-backed item's `output()` token. Detached worktrees
/// all share the `(detached)` branch label, so `output()` returns the
/// worktree path (which is unique) behind this prefix instead.
pub(super) const WORKTREE_OUTPUT_PREFIX: &str = "worktree-path:";

/// Header item for column names (non-selectable)
pub(super) struct HeaderSkimItem {
    pub display_text: String,
    pub display_text_with_ansi: String,
}

impl SkimItem for HeaderSkimItem {
    fn text(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.display_text)
    }

    fn display<'a>(&'a self, _context: skim::DisplayContext<'a>) -> skim::AnsiString<'a> {
        skim::AnsiString::parse(&self.display_text_with_ansi)
    }

    fn output(&self) -> Cow<'_, str> {
        Cow::Borrowed("") // Headers produce no output if selected
    }
}

/// Common diff rendering: check stat, show stat + full diff if non-empty.
fn compute_diff_preview(
    repo: &Repository,
    args: &[&str],
    no_changes_msg: &str,
    width: usize,
) -> String {
    let mut output = String::new();

    // Check stat output first
    let mut stat_args = args.to_vec();
    stat_args.push("--stat");
    stat_args.push("--color=always");
    let stat_width_arg = format!("--stat-width={}", width);
    stat_args.push(&stat_width_arg);

    if let Ok(stat) = repo.run_command(&stat_args)
        && !stat.trim().is_empty()
    {
        output.push_str(&stat);

        // Build diff args with color
        let mut diff_args = args.to_vec();
        diff_args.push("--color=always");

        if let Ok(diff) = repo.run_command(&diff_args) {
            output.push_str(&diff);
        }
    } else {
        output.push_str(no_changes_msg);
        output.push('\n');
    }

    output
}

/// Wrapper to implement SkimItem for ListItem.
///
/// Progressive updates live inside `rendered` — the picker handler rewrites
/// the ANSI-colored display string in place as task results arrive. Skim
/// redraws from `display()` on its 100ms heartbeat, so new values surface
/// without any explicit re-send through the item channel.
///
/// `search_text` (what the matcher sees) stays based on fast-only fields
/// so cached ranks don't need to re-compute when a slow field lands.
pub(super) struct WorktreeSkimItem {
    /// Stable text used for fuzzy matching — branch name + path. Keeping
    /// this independent of the rendered display means skim's matcher
    /// cache survives progressive updates.
    pub search_text: String,
    /// Current ANSI-colored display line. Starts as the skeleton render;
    /// replaced in place as data arrives.
    pub rendered: Arc<Mutex<String>>,
    /// Branch name used by switch selection and preview cache keys.
    pub branch_name: String,
    /// Skeleton-snapshot of the underlying ListItem. Preview computation
    /// reads only skeleton-time fields (`branch_name`, `head`,
    /// `worktree_data`) and runs git directly for anything else — see
    /// `compute_*_preview` in this file — so the snapshot staying frozen
    /// while slow fields (`counts`, `upstream`) arrive via the list-row
    /// task pipeline (see `commands::list::collect`) is intentional and
    /// correct.
    pub item: Arc<ListItem>,
    /// Shared cache for pre-computed previews (all modes)
    pub preview_cache: PreviewCache,
    /// Whether this branch has an upstream tracking ref, for the tab-4
    /// (remote⇅) empty state. A SYNCHRONOUS skeleton-time fact read from
    /// `Repository::local_branches()` at construction — never from the async
    /// `item.upstream`, which is `None` until the row pipeline lands and would
    /// lock the tab bar into a stale state (see `TabAvailability`).
    pub has_upstream: bool,
    /// Whether `[commit.generation]` summaries are configured, for the tab-5
    /// (summary) empty state. A process-wide static fact (`llm_command.is_some()`).
    pub summaries_enabled: bool,
}

impl SkimItem for WorktreeSkimItem {
    fn text(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.search_text)
    }

    fn display<'a>(&'a self, _context: skim::DisplayContext<'a>) -> skim::AnsiString<'a> {
        // Clone-under-lock so AnsiString's input outlives the guard.
        // `AnsiString::parse` returns `AnsiString<'static>`, so the borrow
        // ends with this function.
        let snapshot = self.rendered.lock().unwrap().clone();
        skim::AnsiString::parse(&snapshot)
    }

    fn output(&self) -> Cow<'_, str> {
        match self.item.worktree_path() {
            Some(path) => Cow::Owned(format!(
                "{WORKTREE_OUTPUT_PREFIX}{}",
                path.to_string_lossy()
            )),
            None => Cow::Borrowed(&self.branch_name),
        }
    }

    fn preview(&self, context: PreviewContext<'_>) -> ItemPreview {
        let mode = PreviewStateData::read_mode();

        // Build preview: tabs header + content. `has_upstream`/`summaries_enabled`
        // are synchronous skeleton-time facts (see `TabAvailability`).
        let avail = TabAvailability::worktree(self.has_upstream, self.summaries_enabled);
        let mut result = render_preview_tabs(mode, avail);
        result.push_str(&self.preview_for_mode(mode, context.width, context.height));

        ItemPreview::AnsiText(result)
    }
}

/// Which preview tabs have renderable content for the selected row.
///
/// Empty tabs are de-emphasized in the bar (dimmed, accelerator dropped).
/// Emptiness MUST be a synchronous, skeleton-time fact: skim computes a
/// preview once per selection and cannot re-query it (see
/// `loading_placeholder`), so a bar built from an async field that is still
/// loading would lock into the wrong state and never re-dim. Hence `upstream`
/// reads `Repository::local_branches()` (the pre-skeleton `for-each-ref`
/// scan), NOT the async `item.upstream`; `summary` reads the process-wide
/// `[commit.generation]` flag, NOT the async `item.summary`; and `pr` is a
/// per-row-type constant — worktree rows can't render PR content (their PR
/// number rides the async CI fetch), `--prs` rows always can — never the
/// async PR-status field.
#[derive(Debug, Clone, Copy)]
pub(super) struct TabAvailability {
    working_tree: bool,
    log: bool,
    branch_diff: bool,
    upstream: bool,
    summary: bool,
    pr: bool,
}

impl TabAvailability {
    /// A worktree-backed row: working-tree/log/branch-diff always render; the
    /// upstream and summary tabs depend on synchronous skeleton-time facts;
    /// PR previews render only on `--prs` rows, so tab 6 is empty here.
    pub(super) fn worktree(has_upstream: bool, summaries_enabled: bool) -> Self {
        Self {
            working_tree: true,
            log: true,
            branch_diff: true,
            upstream: has_upstream,
            summary: summaries_enabled,
            pr: false,
        }
    }

    /// A `--prs` row: it carries no local worktree, so only the PR tab has
    /// content; tabs 1-5 are empty.
    pub(super) fn pull_request() -> Self {
        Self {
            working_tree: false,
            log: false,
            branch_diff: false,
            upstream: false,
            summary: false,
            pr: true,
        }
    }
}

/// Render the preview tab bar, shared by worktree rows and `--prs` rows.
///
/// Every tab always keeps its `N: label` text — only the formatting varies, so
/// the accelerators stay discoverable. Three visual states: the active mode is
/// bold; an inactive tab with content is plain (full brightness); an inactive
/// tab with no content for this row (see `TabAvailability`) is dimmed, marking
/// it as nothing to switch to. The active mode always renders bold, even when
/// empty, so the selected tab stays identifiable.
pub(super) fn render_preview_tabs(mode: PreviewMode, avail: TabAvailability) -> String {
    // Full SGR reset (\x1b[0m). color_print's `</>` emits \x1b[22m (intensity
    // reset), which skim's ANSI parser silently ignores — see
    // `skim-0.20.5/src/ansi.rs` `csi_dispatch`, which handles codes 0/1/2/4/5/7
    // but not 22. Without explicit [0m, dim/bold bleeds across the rest of
    // the line in the list and preview panels. Same reason the
    // `compute_*_preview` helpers below scatter `{reset}` after each styled
    // span.
    //
    // TODO(vendor-skim): a one-line fix in skim's ANSIParser removes this
    // workaround everywhere. See `vendor/NOTES.md` → "SGR 22 (intensity
    // reset) handling"; revisit if more users report preview formatting
    // issues.
    let reset = Reset;

    /// Format one tab, keeping the `N: label` text in every state so the
    /// accelerator never disappears: bold when active, plain when
    /// inactive-with-content, and dim when empty (no content for this row).
    fn format_tab(number: u8, label: &str, is_active: bool, has_content: bool) -> String {
        if is_active {
            cformat!("<bold>{}: {}</>", number, label)
        } else if has_content {
            format!("{number}: {label}")
        } else {
            cformat!("<dim>{}: {}</>", number, label)
        }
    }

    let tab1 = format_tab(
        1,
        "HEAD±",
        mode == PreviewMode::WorkingTree,
        avail.working_tree,
    );
    let tab2 = format_tab(2, "log", mode == PreviewMode::Log, avail.log);
    let tab3 = format_tab(
        3,
        "main…±",
        mode == PreviewMode::BranchDiff,
        avail.branch_diff,
    );
    let tab4 = format_tab(
        4,
        "remote⇅",
        mode == PreviewMode::UpstreamDiff,
        avail.upstream,
    );
    let tab5 = format_tab(5, "summary", mode == PreviewMode::Summary, avail.summary);
    let tab6 = format_tab(6, "pr", mode == PreviewMode::Pr, avail.pr);

    // Controls use dim yellow to distinguish from dimmed (white) tabs.
    // The tab numbers above are the alt-N accelerators (bare digits type
    // into the query); Tab/shift-tab cycle the same tabs.
    let controls = cformat!(
        "<dim,yellow>Enter: switch | Tab/alt-1…6: preview | alt-c: create | Esc: cancel | ctrl-u/d: scroll | alt-p: toggle</>"
    );

    // End each tab and controls with full reset to prevent style bleeding
    // into dividers and preview content
    format!(
        "{tab1}{reset} | {tab2}{reset} | {tab3}{reset} | {tab4}{reset} | {tab5}{reset} | {tab6}{reset}\n{controls}{reset}\n\n"
    )
}

/// The PR tab's pane on a worktree row. A worktree row can't render PR
/// content synchronously (its PR number arrives via the async CI fetch), so
/// the tab is empty in the bar and this points the user at the `--prs` rows,
/// which render the PR/MR. Also the fallback for tab 6 in the compute path.
fn pr_unavailable_placeholder() -> String {
    let reset = Reset;
    cformat!(
        "{INFO_SYMBOL}{reset} PR previews appear on <bold>wt switch --prs</>{reset} rows — the CI column shows this branch's PR number\n"
    )
}

impl WorktreeSkimItem {
    /// Render preview for the given mode with specified dimensions.
    ///
    /// Pure cache read: skim calls this synchronously on the UI thread
    /// (see `skim::model::draw_preview`), so any blocking here gates the
    /// entire first render — list included. Background tasks populate the
    /// cache out-of-band; a miss returns a placeholder, and skim will
    /// re-query on the next selection/query change.
    fn preview_for_mode(&self, mode: PreviewMode, width: usize, _height: usize) -> String {
        let cache_key = (self.branch_name.clone(), mode);
        let content = self
            .preview_cache
            .get(&cache_key)
            .map(|v| v.value().clone())
            .unwrap_or_else(|| Self::loading_placeholder(mode));

        match mode {
            // Summary post-processing is cheap (string formatting, no subprocess).
            // Applied at display time because generate_and_cache_summary() inserts
            // raw LLM output.
            PreviewMode::Summary => super::summary::render_summary(&content, width),
            _ => content,
        }
    }

    /// Placeholder shown while a background task is still computing the
    /// preview for this mode. Skim has no API to re-query preview from
    /// outside user interaction (see `skim::previewer::on_item_change` bail
    /// at `previewer.rs:187`), so the hint tells the user to press the mode
    /// key again to refresh once the background fill lands. `alt-N`
    /// re-runs the same `echo N + refresh-preview` chain, re-reading the
    /// now-populated cache.
    pub(super) fn loading_placeholder(mode: PreviewMode) -> String {
        let (verb, label) = match mode {
            PreviewMode::WorkingTree => ("Loading", "working-tree diff"),
            PreviewMode::Log => ("Loading", "log"),
            PreviewMode::BranchDiff => ("Loading", "branch diff"),
            PreviewMode::UpstreamDiff => ("Loading", "upstream diff"),
            PreviewMode::Summary => ("Generating", "summary"),
            // The PR tab has no background task to wait on; show the static
            // pointer to `--prs` rows instead of a "Loading…" line.
            PreviewMode::Pr => return pr_unavailable_placeholder(),
        };
        let key = mode as u8;
        format!("○ {verb} {label}. Press alt-{key} again to refresh.\n")
    }

    /// Compute preview and apply pager for diff modes. Returns the
    /// display-ready string and (for Log) whether the disk cache was a
    /// hit — the orchestrator uses the flag to schedule a background
    /// refresh.
    ///
    /// Both the inline cache-miss path and background pre-computation use this so
    /// that the cache always stores display-ready content (no pager subprocess
    /// needed at render time).
    pub(super) fn compute_and_page_preview(
        repo: &Repository,
        item: &ListItem,
        mode: PreviewMode,
        width: usize,
        height: usize,
    ) -> (String, bool) {
        match mode {
            PreviewMode::WorkingTree => (
                Self::page_diff(Self::compute_working_tree_preview(repo, item, width), width),
                false,
            ),
            PreviewMode::Log => Self::compute_log_preview(repo, item, width, height),
            PreviewMode::BranchDiff => (
                Self::page_diff(Self::compute_branch_diff_preview(repo, item, width), width),
                false,
            ),
            PreviewMode::UpstreamDiff => (
                Self::page_diff(
                    Self::compute_upstream_diff_preview(repo, item, width),
                    width,
                ),
                false,
            ),
            PreviewMode::Summary => (Self::loading_placeholder(PreviewMode::Summary), false),
            // PR previews never precompute (no git/LLM work); this arm only
            // keeps the match total. Worktree rows render the placeholder.
            PreviewMode::Pr => (pr_unavailable_placeholder(), false),
        }
    }

    fn page_diff(content: String, width: usize) -> String {
        if let Some(pager_cmd) = diff_pager() {
            pipe_through_pager(&content, pager_cmd, width)
        } else {
            content
        }
    }

    /// Compute Tab 1: Working tree preview (uncommitted changes vs HEAD)
    fn compute_working_tree_preview(repo: &Repository, item: &ListItem, width: usize) -> String {
        let branch = item.branch_name();
        let Some(wt_info) = item.worktree_data() else {
            let reset = Reset;
            return cformat!(
                "{INFO_SYMBOL}{reset} <bold>{branch}</>{reset} is branch only — press Enter to create worktree\n"
            );
        };

        let path = wt_info.path.display().to_string();

        let reset = Reset;
        compute_diff_preview(
            repo,
            &["-C", &path, "diff", "HEAD"],
            &cformat!("{INFO_SYMBOL}{reset} <bold>{branch}</>{reset} has no uncommitted changes"),
            width,
        )
    }

    /// Compute Tab 3: Branch diff preview (line diffs in commits ahead of default branch)
    ///
    /// Independent of `item.counts` — `compute_diff_preview`'s empty-diff
    /// fallback covers the ahead=0 case, so the preview is correct even
    /// before the list-row pipeline has populated counts.
    ///
    /// The default branch's SHA comes from [`Repository::default_branch_sha`],
    /// which sources it from the already-warmed local-branch inventory. N
    /// parallel preview tasks all share one inventory scan instead of each
    /// forking `git rev-parse`. The SHA also keeps the disk cache invariant
    /// across `git fetch` (which moves the *ref* but not the captured SHA).
    /// When the SHA isn't available (no default branch, or stale config
    /// pointing at a deleted branch), we fall through to the uncached path
    /// with the branch name in the diff range — same git behavior as
    /// before, just no cache read/write.
    fn compute_branch_diff_preview(repo: &Repository, item: &ListItem, width: usize) -> String {
        let branch = item.branch_name();
        let reset = Reset;
        let Some(default_branch) = repo.default_branch() else {
            return cformat!(
                "{INFO_SYMBOL}{reset} <bold>{branch}</>{reset} has no commits ahead of main\n"
            );
        };

        let base_sha = repo.default_branch_sha();

        if let Some(ref base) = base_sha
            && let Some(cached) = preview_cache::read_branch_diff(repo, base, item.head(), width)
        {
            return cached;
        }

        // Use the resolved SHA in the diff range when available so the
        // cache key and the diff agree on which commit was the base.
        let base_ref = base_sha.as_deref().unwrap_or(&default_branch);
        let merge_base = format!("{base_ref}...{}", item.head());
        let result = compute_diff_preview(
            repo,
            &["diff", &merge_base],
            &cformat!(
                "{INFO_SYMBOL}{reset} <bold>{branch}</>{reset} has no file changes vs <bold>{default_branch}</>{reset}"
            ),
            width,
        );

        if let Some(ref base) = base_sha {
            preview_cache::write_branch_diff(repo, base, item.head(), width, &result);
        }
        result
    }

    /// Compute Tab 4: Upstream diff preview (ahead/behind vs tracking branch)
    ///
    /// Independent of `item.upstream` — `git rev-parse {branch}@{{u}}`
    /// probes existence (non-zero exit when `@{{u}}` is unresolvable) and
    /// also yields the upstream SHA for cache keying. The follow-up
    /// `rev-list --left-right --count` then runs against the resolved SHAs
    /// so the count and the cached diff agree on which upstream commit was
    /// in play.
    fn compute_upstream_diff_preview(repo: &Repository, item: &ListItem, width: usize) -> String {
        let branch = item.branch_name();
        let reset = Reset;

        let upstream_ref = format!("{branch}@{{u}}");
        let Ok(upstream_sha_raw) =
            repo.run_command(&["rev-parse", "--verify", "--end-of-options", &upstream_ref])
        else {
            return cformat!(
                "{INFO_SYMBOL}{reset} <bold>{branch}</>{reset} has no upstream tracking branch\n"
            );
        };
        let upstream_sha = upstream_sha_raw.trim();

        if let Some(cached) =
            preview_cache::read_upstream_diff(repo, item.head(), upstream_sha, width)
        {
            return cached;
        }

        let probe_range = format!("{}...{upstream_sha}", item.head());
        let Ok(counts) = repo.run_command(&[
            "rev-list",
            "--left-right",
            "--count",
            "--end-of-options",
            &probe_range,
        ]) else {
            return cformat!(
                "{INFO_SYMBOL}{reset} <bold>{branch}</>{reset} has no upstream tracking branch\n"
            );
        };
        let mut parts = counts.split_whitespace();
        let parsed = parts
            .next()
            .zip(parts.next())
            .and_then(|(a, b)| Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok()?)));
        let Some((ahead, behind)) = parsed else {
            // Unreachable if `rev-list --left-right --count` succeeded —
            // git guarantees two whitespace-separated integers. Fall
            // through to the safe no-upstream message rather than
            // fabricating zeros if git ever changes output format.
            return cformat!(
                "{INFO_SYMBOL}{reset} <bold>{branch}</>{reset} has no upstream tracking branch\n"
            );
        };

        let result = if ahead == 0 && behind == 0 {
            cformat!("{INFO_SYMBOL}{reset} <bold>{branch}</>{reset} is up to date with upstream\n")
        } else if ahead > 0 && behind > 0 {
            let range = format!("{upstream_sha}...{}", item.head());
            compute_diff_preview(
                repo,
                &["diff", &range],
                &cformat!(
                    "{INFO_SYMBOL}{reset} <bold>{branch}</>{reset} has diverged (⇡{ahead} ⇣{behind}) but no unique file changes"
                ),
                width,
            )
        } else if ahead > 0 {
            let range = format!("{upstream_sha}...{}", item.head());
            compute_diff_preview(
                repo,
                &["diff", &range],
                &cformat!(
                    "{INFO_SYMBOL}{reset} <bold>{branch}</>{reset} has no unpushed file changes"
                ),
                width,
            )
        } else {
            let range = format!("{}...{upstream_sha}", item.head());
            compute_diff_preview(
                repo,
                &["diff", &range],
                &cformat!(
                    "{INFO_SYMBOL}{reset} <bold>{branch}</>{reset} is behind upstream (⇣{behind}) but no file changes"
                ),
                width,
            )
        };

        preview_cache::write_upstream_diff(repo, item.head(), upstream_sha, width, &result);
        result
    }

    /// Compute log preview for a worktree item.
    ///
    /// Splits work into a SHA-deterministic part that's safe to disk-cache
    /// (raw `git log --graph` output and the per-commit insertions/deletions
    /// map from `batch_fetch_stats`) and a path that has to recompute on
    /// every call (merge-base + rev-list for the dim/bright split, plus
    /// `format_log_output` for relative timestamps). This keeps the cache
    /// key out of `main`'s SHA — a `git fetch` advancing `origin/main`
    /// doesn't invalidate any entry — while preserving correctness as
    /// `main` and wall-clock advance.
    ///
    /// Returns the rendered preview and a flag for whether the disk cache
    /// was hit. The orchestrator uses the flag to schedule a background
    /// refresh — see [`Self::refresh_log_preview`] and the `LogCacheEntry`
    /// docstring for why decorations baked into `raw_log` need refreshing
    /// even though the cache key is correct.
    pub(super) fn compute_log_preview(
        repo: &Repository,
        item: &ListItem,
        width: usize,
        height: usize,
    ) -> (String, bool) {
        Self::compute_log_preview_inner(repo, item, width, height, false)
    }

    /// Force-recompute the Log preview, bypassing the disk-cache read but
    /// writing through. Returns the freshly rendered string. Called by the
    /// orchestrator's refresh worker after a cache hit so the next visit
    /// sees decorations that match current ref topology.
    pub(super) fn refresh_log_preview(
        repo: &Repository,
        item: &ListItem,
        width: usize,
        height: usize,
    ) -> String {
        Self::compute_log_preview_inner(repo, item, width, height, true).0
    }

    fn compute_log_preview_inner(
        repo: &Repository,
        item: &ListItem,
        width: usize,
        height: usize,
        force_recompute: bool,
    ) -> (String, bool) {
        // Minimum preview width to show timestamps (adds ~7 chars: space + 4-char time + space)
        // Note: preview is typically 50% of terminal width, so 50 = 100-col terminal
        const TIMESTAMP_WIDTH_THRESHOLD: usize = 50;
        // Tab header takes 3 lines (tabs + controls + blank)
        const HEADER_LINES: usize = 3;

        let show_timestamps = width >= TIMESTAMP_WIDTH_THRESHOLD;
        // Calculate how many log lines fit in preview (height minus header)
        let log_limit = height.saturating_sub(HEADER_LINES).max(1);
        let head = item.head();
        let branch = item.branch_name();
        let reset = Reset;
        let Some(default_branch) = repo.default_branch() else {
            return (
                cformat!("{INFO_SYMBOL}{reset} <bold>{branch}</>{reset} has no commits\n"),
                false,
            );
        };

        // merge-base / rev-list run on every call — they're how the
        // dim/bright split tracks main's current position. See the cache
        // entry docstring for why we keep this off the SHA-keyed disk cache.
        //
        // Don't pre-resolve `default_branch` to a SHA via
        // `Repository::default_branch_sha` here. That accessor is a
        // snapshot of the local-branch inventory at first scan (see its
        // docstring) — feeding the snapshot into a merge-base call would
        // freeze the dim/bright styling at the SHA main pointed at when
        // the picker started, instead of the current SHA. The
        // `log_cache_dim_split_tracks_main_advance` test pins this
        // contract.
        //
        // (`Repository::merge_base` is correctness-safe — it re-resolves
        // ref names through an uncached `git rev-parse` before hitting
        // its SHA-keyed cache — but it'd cost an extra subprocess per
        // render on cache miss, with no win since each item's head is
        // unique.)
        //
        // Error handling note: this code runs in an interactive preview
        // pane. Silent fallbacks beat disruptive errors during navigation;
        // the preview is supplementary, users can still select worktrees
        // even if a probe fails.
        let Ok(merge_base_output) =
            repo.run_command(&["merge-base", "--end-of-options", &default_branch, head])
        else {
            return (
                cformat!("{INFO_SYMBOL}{reset} <bold>{branch}</>{reset} has no commits\n"),
                false,
            );
        };
        let merge_base = merge_base_output.trim();
        let is_default_branch = branch == default_branch;
        let log_limit_str = log_limit.to_string();

        // Get commits after merge-base (for dimming logic)
        // These are commits reachable from HEAD but not from merge-base, shown bright.
        // Commits before merge-base (shared with default branch) are shown dimmed.
        // Bounded to log_limit since we only need to check displayed commits.
        let unique_commits: Option<HashSet<String>> = if is_default_branch {
            // On default branch: no dimming (None means show everything bright)
            None
        } else {
            // On feature branch: get commits unique to this branch
            // rev-list A...B --right-only gives commits reachable from B but not A
            let range = format!("{}...{}", merge_base, head);
            let commits = repo
                .run_command(&["rev-list", &range, "--right-only", "-n", &log_limit_str])
                .map(|out| out.lines().map(String::from).collect())
                .unwrap_or_default();
            Some(commits) // Some(empty) means dim everything
        };

        // Cacheable: the raw `git log --graph` output plus per-commit
        // stats. Both are pure functions of (head, width, height); on a
        // disk-cache hit we skip the `git log` and `git diff-tree` calls
        // entirely. On miss we compute and write through. `force_recompute`
        // bypasses the read (the refresh path) but always writes.
        let cached = if force_recompute {
            None
        } else {
            preview_cache::read_log(repo, head, width, height)
        };
        let was_disk_hit = cached.is_some();
        // On `git log` failure (effectively unreachable — merge-base
        // already validated `head`), `unwrap_or_default()` yields an
        // empty entry which `process_log_with_dimming` + `format_log_output`
        // render as empty output below. We deliberately skip the disk
        // write in that case: persisting an empty `LogCacheEntry` would
        // poison the SHA-keyed cache so a single transient failure
        // suppresses the preview indefinitely.
        let entry = cached.unwrap_or_else(|| {
            let fresh = Self::compute_log_raw_and_stats(repo, head, log_limit, show_timestamps);
            if let Some(ref f) = fresh {
                preview_cache::write_log(repo, head, width, height, f);
            }
            fresh.unwrap_or_default()
        });

        let (processed, _hashes) =
            process_log_with_dimming(&entry.raw_log, unique_commits.as_ref());
        let rendered = if show_timestamps {
            // `format_log_output` reads `epoch_now()` so relative-time
            // strings ("5m" / "2h" / "3d") track wall-clock on every call,
            // even when serving from cache.
            format_log_output(&processed, &entry.stats)
        } else {
            // Strip hash markers (SOH...NUL) since we're not using format_log_output
            strip_hash_markers(&processed)
        };
        (rendered, was_disk_hit)
    }

    /// Run `git log --graph` and (when timestamps are shown) `batch_fetch_stats`,
    /// returning the SHA-deterministic payload to store in the disk cache.
    /// Returns `None` only when `git log` itself fails — caller renders an
    /// empty preview in that case.
    fn compute_log_raw_and_stats(
        repo: &Repository,
        head: &str,
        log_limit: usize,
        show_timestamps: bool,
    ) -> Option<preview_cache::LogCacheEntry> {
        // Format strings for git log
        // Without timestamps: hash (colored/dimmed), then message
        // Format includes full hash (for matching) between SOH and NUL delimiters.
        // Display content uses \x1f to separate fields for timestamp parsing.
        // Format: SOH full_hash NUL short_hash \x1f timestamp \x1f decorations+message
        // Using delimiters allows parsing without assuming fixed hash length (SHA-256 safe)
        // Note: Use %x01/%x00 (git's hex escapes) to avoid embedding control chars in argv
        let timestamp_format = format!(
            "--format=%x01%H%x00%C(auto)%h{}%ct{}%C(auto)%d%C(reset) %s",
            FIELD_DELIM, FIELD_DELIM
        );
        let no_timestamp_format = "--format=%x01%H%x00%C(auto)%h%C(auto)%d%C(reset) %s";
        let format: &str = if show_timestamps {
            &timestamp_format
        } else {
            no_timestamp_format
        };
        let log_limit_str = log_limit.to_string();
        let args = vec![
            "log",
            "--graph",
            "--no-show-signature",
            format,
            "--color=always",
            "-n",
            &log_limit_str,
            head,
        ];

        let raw_log = repo.run_command(&args).ok()?;

        let stats = if show_timestamps {
            // Pull hashes from the raw log via `process_log_with_dimming`
            // with `unique_commits = None` — that path doesn't apply any
            // dim styling, so we get a clean hash list for the stats fetch
            // without baking dimming into the cached value.
            let (_processed, hashes) = process_log_with_dimming(&raw_log, None);
            batch_fetch_stats(repo, &hashes)
        } else {
            std::collections::HashMap::new()
        };

        Some(preview_cache::LogCacheEntry { raw_log, stats })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;

    #[test]
    fn test_render_preview_tabs() {
        // Each mode active, on a worktree row with upstream + summaries
        // enabled (tabs 1-5 present; tab 6 empty — PR previews live on --prs
        // rows). Verifies labels, active styling, and structure.
        let wt = TabAvailability::worktree(true, true);
        for (name, mode) in [
            ("working_tree", PreviewMode::WorkingTree),
            ("log", PreviewMode::Log),
            ("branch_diff", PreviewMode::BranchDiff),
            ("upstream_diff", PreviewMode::UpstreamDiff),
            ("summary", PreviewMode::Summary),
            ("pr", PreviewMode::Pr),
        ] {
            assert_snapshot!(name, render_preview_tabs(mode, wt));
        }

        // Empty states: a worktree row with no upstream and summaries disabled
        // dims tabs 4 and 5 (number kept); a --prs row dims tabs 1-5.
        assert_snapshot!(
            "empty_upstream_and_summary",
            render_preview_tabs(
                PreviewMode::WorkingTree,
                TabAvailability::worktree(false, false)
            )
        );
        assert_snapshot!(
            "pr_row",
            render_preview_tabs(PreviewMode::Pr, TabAvailability::pull_request())
        );
    }

    #[test]
    fn test_loading_placeholder_all_modes() {
        // Verifies wording and refresh-key hint per mode.
        for (name, mode) in [
            ("working_tree", PreviewMode::WorkingTree),
            ("log", PreviewMode::Log),
            ("branch_diff", PreviewMode::BranchDiff),
            ("upstream_diff", PreviewMode::UpstreamDiff),
            ("summary", PreviewMode::Summary),
            // On a worktree row the `pr` tab has no content, so this returns
            // the "appears on --prs rows" placeholder (`pr_unavailable_placeholder`).
            ("pr", PreviewMode::Pr),
        ] {
            assert_snapshot!(
                format!("loading_placeholder_{name}"),
                WorktreeSkimItem::loading_placeholder(mode)
            );
        }
    }

    #[test]
    fn test_preview_for_mode_summary_cache() {
        // Cache hit returns cached content; cache miss computes the placeholder
        let item = Arc::new(ListItem::new_branch(
            "abc123".to_string(),
            "feature".to_string(),
        ));

        let cache_hit = {
            let preview_cache: PreviewCache = Arc::new(DashMap::new());
            preview_cache.insert(
                ("feature".to_string(), PreviewMode::Summary),
                "Add auth module\n\nImplements JWT-based authentication.".to_string(),
            );
            WorktreeSkimItem {
                search_text: String::new(),
                rendered: Arc::new(Mutex::new(String::new())),
                branch_name: "feature".to_string(),
                item: Arc::clone(&item),
                preview_cache,
                has_upstream: false,
                summaries_enabled: false,
            }
        };

        let cache_miss = {
            let preview_cache: PreviewCache = Arc::new(DashMap::new());
            WorktreeSkimItem {
                search_text: String::new(),
                rendered: Arc::new(Mutex::new(String::new())),
                branch_name: "feature".to_string(),
                item: Arc::clone(&item),
                preview_cache,
                has_upstream: false,
                summaries_enabled: false,
            }
        };

        assert_snapshot!(
            "cache_hit",
            cache_hit.preview_for_mode(PreviewMode::Summary, 80, 40)
        );
        assert_snapshot!(
            "cache_miss",
            cache_miss.preview_for_mode(PreviewMode::Summary, 80, 40)
        );
    }

    /// Helper: build a test repo with `main` at the initial commit, then a
    /// second commit so branches can diverge from it.
    fn repo_with_main() -> (worktrunk::testing::TestRepo, Repository) {
        let t = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = Repository::at(t.path()).unwrap();
        // Add a second commit on main so later branches have a merge base
        // with a real parent (otherwise `rev-list main...HEAD` walks back
        // to the initial commit unconditionally).
        std::fs::write(t.path().join("main2.txt"), "main2").unwrap();
        repo.run_command(&["add", "main2.txt"]).unwrap();
        repo.run_command(&["commit", "-m", "main2"]).unwrap();
        (t, repo)
    }

    fn item_at(repo: &Repository, branch: &str) -> ListItem {
        let head = repo
            .run_command(&["rev-parse", branch])
            .unwrap()
            .trim()
            .to_string();
        ListItem::new_branch(head, branch.to_string())
    }

    #[test]
    fn branch_diff_empty_when_no_commits_ahead() {
        // A branch at the same commit as main has no commits ahead — the
        // empty-diff fallback message should fire.
        let (_t, repo) = repo_with_main();
        repo.run_command(&["branch", "parity"]).unwrap();
        let item = item_at(&repo, "parity");
        let output = WorktreeSkimItem::compute_branch_diff_preview(&repo, &item, 80);
        assert!(
            output.contains("has no file changes vs"),
            "expected empty-diff fallback, got: {output:?}"
        );
    }

    #[test]
    fn branch_diff_shows_diff_when_commits_ahead() {
        // A branch with a unique commit should produce a non-empty diff.
        let (t, repo) = repo_with_main();
        repo.run_command(&["checkout", "-b", "feature"]).unwrap();
        std::fs::write(t.path().join("feat.txt"), "feature\n").unwrap();
        repo.run_command(&["add", "feat.txt"]).unwrap();
        repo.run_command(&["commit", "-m", "feat"]).unwrap();
        let item = item_at(&repo, "feature");
        let output = WorktreeSkimItem::compute_branch_diff_preview(&repo, &item, 80);
        assert!(
            output.contains("feat.txt"),
            "expected diff to mention feat.txt, got: {output:?}"
        );
    }

    #[test]
    fn branch_diff_cache_short_circuits_recompute() {
        // Pre-populate the disk cache with a sentinel value, then call
        // compute — a hit must return the sentinel verbatim instead of
        // running git diff. Proves the SHA + width key is the lookup path
        // and that a hit short-circuits before `compute_diff_preview`.
        let (t, repo) = repo_with_main();
        repo.run_command(&["checkout", "-b", "feature"]).unwrap();
        std::fs::write(t.path().join("real.txt"), "real\n").unwrap();
        repo.run_command(&["add", "real.txt"]).unwrap();
        repo.run_command(&["commit", "-m", "real"]).unwrap();
        let item = item_at(&repo, "feature");

        let base_sha = repo.default_branch_sha().unwrap();
        let sentinel = "SENTINEL_FROM_CACHE";
        super::preview_cache::write_branch_diff(&repo, &base_sha, item.head(), 80, sentinel);

        let output = WorktreeSkimItem::compute_branch_diff_preview(&repo, &item, 80);
        assert_eq!(output, sentinel, "cache hit must return cached value");
    }

    #[test]
    fn branch_diff_cache_writeback_on_miss() {
        // After a miss, the next call's cache key must be populated. Width
        // is part of the key, so a different width still misses.
        let (t, repo) = repo_with_main();
        repo.run_command(&["checkout", "-b", "feature"]).unwrap();
        std::fs::write(t.path().join("wb.txt"), "wb\n").unwrap();
        repo.run_command(&["add", "wb.txt"]).unwrap();
        repo.run_command(&["commit", "-m", "wb"]).unwrap();
        let item = item_at(&repo, "feature");

        let base_sha = repo.default_branch_sha().unwrap();

        assert!(
            super::preview_cache::read_branch_diff(&repo, &base_sha, item.head(), 80).is_none()
        );
        let _ = WorktreeSkimItem::compute_branch_diff_preview(&repo, &item, 80);
        assert!(
            super::preview_cache::read_branch_diff(&repo, &base_sha, item.head(), 80).is_some()
        );
        // Different width: miss.
        assert!(
            super::preview_cache::read_branch_diff(&repo, &base_sha, item.head(), 100).is_none()
        );
    }

    #[test]
    fn log_cache_writeback_on_miss() {
        // First call populates the cache; the entry must exist after.
        // Width is part of the key, so a different width still misses.
        let (t, repo) = repo_with_main();
        repo.run_command(&["checkout", "-b", "feature"]).unwrap();
        std::fs::write(t.path().join("log.txt"), "x\n").unwrap();
        repo.run_command(&["add", "log.txt"]).unwrap();
        repo.run_command(&["commit", "-m", "log"]).unwrap();
        let item = item_at(&repo, "feature");

        assert!(super::preview_cache::read_log(&repo, item.head(), 80, 24).is_none());
        let _ = WorktreeSkimItem::compute_log_preview(&repo, &item, 80, 24).0;
        let entry = super::preview_cache::read_log(&repo, item.head(), 80, 24)
            .expect("cache populated after first compute");
        assert!(
            !entry.raw_log.is_empty(),
            "cached raw log should be non-empty"
        );
        assert!(
            super::preview_cache::read_log(&repo, item.head(), 100, 24).is_none(),
            "different width still misses"
        );
    }

    #[test]
    fn log_cache_dim_split_tracks_main_advance() {
        // Regression for worktrunk-bot's review on PR #2628: the cache key
        // is only `(branch_head_sha, w, h)` — main's SHA isn't included —
        // so a `git fetch` advancing the default branch must NOT serve
        // stale dim/bright styling. The dim split runs on every call from
        // a fresh `merge-base` + `rev-list`, even on cache hit.
        //
        // Setup: feature branches off main, gets a unique commit, then
        // main advances to include that commit. Before main advances,
        // feature's commit is "unique" (bright). After main advances and
        // contains the commit, it's no longer unique (dim).
        let (t, repo) = repo_with_main();
        repo.run_command(&["checkout", "-b", "feature"]).unwrap();
        std::fs::write(t.path().join("f.txt"), "feat\n").unwrap();
        repo.run_command(&["add", "f.txt"]).unwrap();
        repo.run_command(&["commit", "-m", "feature commit"])
            .unwrap();
        let feature_head = repo
            .run_command(&["rev-parse", "feature"])
            .unwrap()
            .trim()
            .to_string();
        let item = ListItem::new_branch(feature_head.clone(), "feature".to_string());

        // The dim/bright signal we check is the bold-green branch
        // decoration `\x1b[1;32m` — `git log --format=%C(auto)%d` colors
        // the branch name (e.g. `feature`) bold-green when bright, and
        // `process_log_with_dimming`'s dim path runs `display.ansi_strip()`
        // which removes that escape. The dim SGR `\x1b[2m` is unsuitable
        // because `format_log_output` already wraps every relative-time
        // column in dim, so it appears even in bright lines.
        let before = WorktreeSkimItem::compute_log_preview(&repo, &item, 80, 24).0;
        let before_subject_line = before
            .lines()
            .find(|l| l.contains("feature commit"))
            .expect("subject line present before advance");
        assert!(
            before_subject_line.contains("\x1b[1;32m"),
            "before main advance, unique commit should be bright (bold-green branch decoration present), got: {before_subject_line:?}"
        );

        // Advance main to include feature's commit. Same `feature_head`,
        // same cache key — but the dim split now changes because rev-list
        // returns no unique commits.
        repo.run_command(&["checkout", "main"]).unwrap();
        repo.run_command(&["merge", "--ff-only", "feature"])
            .unwrap();
        repo.run_command(&["checkout", "feature"]).unwrap();

        let after = WorktreeSkimItem::compute_log_preview(&repo, &item, 80, 24).0;
        let after_subject_line = after
            .lines()
            .find(|l| l.contains("feature commit"))
            .expect("subject line present after advance");
        assert!(
            !after_subject_line.contains("\x1b[1;32m"),
            "after main advance, commit should be dimmed (bold-green stripped by dim path), got: {after_subject_line:?}"
        );
    }

    #[test]
    fn upstream_diff_cache_short_circuits_recompute() {
        let (_t, repo) = repo_with_tracked_pair();
        let item = item_at(&repo, "feature");
        let upstream_sha = repo
            .run_command(&["rev-parse", "upstream-base"])
            .unwrap()
            .trim()
            .to_string();
        let sentinel = "SENTINEL_UPSTREAM_VALUE";
        super::preview_cache::write_upstream_diff(&repo, item.head(), &upstream_sha, 80, sentinel);

        let output = WorktreeSkimItem::compute_upstream_diff_preview(&repo, &item, 80);
        assert_eq!(output, sentinel);
    }

    #[test]
    fn upstream_diff_no_tracking_branch() {
        // Branch with no configured upstream should hit the no-upstream path
        // via non-zero exit from `git rev-list --left-right --count HEAD...@{u}`.
        let (_t, repo) = repo_with_main();
        repo.run_command(&["branch", "orphan"]).unwrap();
        let item = item_at(&repo, "orphan");
        let output = WorktreeSkimItem::compute_upstream_diff_preview(&repo, &item, 80);
        assert!(
            output.contains("has no upstream tracking branch"),
            "expected no-upstream message, got: {output:?}"
        );
    }

    /// Sets up a branch that tracks another local branch, so `@{u}` resolves
    /// without needing a remote. This covers all four ahead/behind shapes.
    fn repo_with_tracked_pair() -> (worktrunk::testing::TestRepo, Repository) {
        let (t, repo) = repo_with_main();
        repo.run_command(&["branch", "upstream-base"]).unwrap();
        repo.run_command(&["checkout", "-b", "feature"]).unwrap();
        repo.run_command(&["branch", "--set-upstream-to=upstream-base"])
            .unwrap();
        (t, repo)
    }

    #[test]
    fn upstream_diff_up_to_date() {
        let (_t, repo) = repo_with_tracked_pair();
        let item = item_at(&repo, "feature");
        let output = WorktreeSkimItem::compute_upstream_diff_preview(&repo, &item, 80);
        assert!(
            output.contains("is up to date with upstream"),
            "expected up-to-date message, got: {output:?}"
        );
    }

    #[test]
    fn upstream_diff_ahead_only() {
        let (t, repo) = repo_with_tracked_pair();
        std::fs::write(t.path().join("ahead.txt"), "ahead\n").unwrap();
        repo.run_command(&["add", "ahead.txt"]).unwrap();
        repo.run_command(&["commit", "-m", "ahead"]).unwrap();
        let item = item_at(&repo, "feature");
        let output = WorktreeSkimItem::compute_upstream_diff_preview(&repo, &item, 80);
        assert!(
            output.contains("ahead.txt"),
            "expected diff to mention ahead.txt, got: {output:?}"
        );
    }

    #[test]
    fn upstream_diff_behind_only() {
        let (t, repo) = repo_with_tracked_pair();
        // Advance the upstream (upstream-base) past feature
        repo.run_command(&["checkout", "upstream-base"]).unwrap();
        std::fs::write(t.path().join("behind.txt"), "behind\n").unwrap();
        repo.run_command(&["add", "behind.txt"]).unwrap();
        repo.run_command(&["commit", "-m", "behind"]).unwrap();
        repo.run_command(&["checkout", "feature"]).unwrap();
        let item = item_at(&repo, "feature");
        let output = WorktreeSkimItem::compute_upstream_diff_preview(&repo, &item, 80);
        assert!(
            output.contains("behind.txt"),
            "expected diff to mention behind.txt, got: {output:?}"
        );
    }

    #[test]
    fn upstream_diff_diverged() {
        let (t, repo) = repo_with_tracked_pair();
        // feature has a unique commit
        std::fs::write(t.path().join("feat.txt"), "feat\n").unwrap();
        repo.run_command(&["add", "feat.txt"]).unwrap();
        repo.run_command(&["commit", "-m", "feat"]).unwrap();
        // upstream-base has a unique commit
        repo.run_command(&["checkout", "upstream-base"]).unwrap();
        std::fs::write(t.path().join("upstream.txt"), "upstream\n").unwrap();
        repo.run_command(&["add", "upstream.txt"]).unwrap();
        repo.run_command(&["commit", "-m", "upstream"]).unwrap();
        repo.run_command(&["checkout", "feature"]).unwrap();
        let item = item_at(&repo, "feature");
        let output = WorktreeSkimItem::compute_upstream_diff_preview(&repo, &item, 80);
        // Diverged path runs the diff; symmetric difference includes both files.
        assert!(
            output.contains("feat.txt") || output.contains("upstream.txt"),
            "expected diverged diff, got: {output:?}"
        );
    }

    #[test]
    fn test_render_preview_tabs_ansi_codes() {
        // Test that ANSI escape sequences properly reset to prevent style bleeding.
        // The per-tab `{reset}` is appended in the outer format regardless of a
        // tab's internal styling, so the reset/divider counts hold whether a tab
        // is bold (active), plain (inactive-with-content, no internal SGR), or
        // dim (empty — here tab 6, pr).
        let output = render_preview_tabs(
            PreviewMode::WorkingTree,
            TabAvailability::worktree(true, true),
        );

        let first_line = output.lines().next().unwrap();
        let second_line = output.lines().nth(1).unwrap();

        // Each styled tab should end with a full reset (\x1b[0m) before the divider
        // This prevents bold/dim from bleeding into the " | " dividers
        let full_reset = "\x1b[0m";

        // Count resets - should have one after each of the 6 tabs
        assert_eq!(first_line.matches(full_reset).count(), 6);

        // The sequence should be: style + text + [22m + [0m + divider
        // Check that dividers come after full resets
        let parts: Vec<&str> = first_line.split(" | ").collect();
        assert_eq!(parts.len(), 6);
        assert!(parts.iter().all(|part| part.ends_with(full_reset)));

        // Controls line should end with full reset to ensure clean state for preview content
        assert!(second_line.ends_with(full_reset));
    }
}
