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

/// Cache key for pre-computed previews: (branch_name, mode).
pub(super) type PreviewCacheKey = (String, PreviewMode);

/// Cache for pre-computed previews, keyed by (branch_name, mode).
/// Shared across all WorktreeSkimItems for background pre-computation.
pub(super) type PreviewCache = Arc<DashMap<PreviewCacheKey, String>>;

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
    /// Branch name — also what `output()` returns when this item is
    /// selected.
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
        Cow::Borrowed(&self.branch_name)
    }

    fn preview(&self, context: PreviewContext<'_>) -> ItemPreview {
        let mode = PreviewStateData::read_mode();

        // Build preview: tabs header + content
        let mut result = Self::render_preview_tabs(mode);
        result.push_str(&self.preview_for_mode(mode, context.width, context.height));

        ItemPreview::AnsiText(result)
    }
}

impl WorktreeSkimItem {
    /// Render the tab header for the preview window
    ///
    /// Shows all preview modes as tabs, with the current mode bolded
    /// and unselected modes dimmed. Controls shown below in normal text
    /// for visual distinction from inactive tabs.
    pub(super) fn render_preview_tabs(mode: PreviewMode) -> String {
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

        /// Format a tab label with bold (active) or dimmed (inactive) styling
        fn format_tab(label: &str, is_active: bool) -> String {
            if is_active {
                cformat!("<bold>{}</>", label)
            } else {
                cformat!("<dim>{}</>", label)
            }
        }

        let tab1 = format_tab("1: HEAD±", mode == PreviewMode::WorkingTree);
        let tab2 = format_tab("2: log", mode == PreviewMode::Log);
        let tab3 = format_tab("3: main…±", mode == PreviewMode::BranchDiff);
        let tab4 = format_tab("4: remote⇅", mode == PreviewMode::UpstreamDiff);
        let tab5 = format_tab("5: summary", mode == PreviewMode::Summary);

        // Controls use dim yellow to distinguish from dimmed (white) tabs
        let controls = cformat!(
            "<dim,yellow>Enter: switch | alt-c: create | Esc: cancel | ctrl-u/d: scroll | alt-p: toggle</>"
        );

        // End each tab and controls with full reset to prevent style bleeding
        // into dividers and preview content
        format!(
            "{tab1}{reset} | {tab2}{reset} | {tab3}{reset} | {tab4}{reset} | {tab5}{reset}\n{controls}{reset}\n\n"
        )
    }

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
    /// key again to refresh once the background fill lands.
    pub(super) fn loading_placeholder(mode: PreviewMode) -> String {
        let (verb, label) = match mode {
            PreviewMode::WorkingTree => ("Loading", "working-tree diff"),
            PreviewMode::Log => ("Loading", "log"),
            PreviewMode::BranchDiff => ("Loading", "branch diff"),
            PreviewMode::UpstreamDiff => ("Loading", "upstream diff"),
            PreviewMode::Summary => ("Generating", "summary"),
        };
        let key = mode as u8;
        format!("○ {verb} {label}. Press {key} again to refresh.\n")
    }

    /// Compute preview and apply pager for diff modes.
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
    ) -> String {
        let content = Self::compute_preview(repo, item, mode, width, height);
        match mode {
            PreviewMode::WorkingTree | PreviewMode::BranchDiff | PreviewMode::UpstreamDiff => {
                if let Some(pager_cmd) = diff_pager() {
                    pipe_through_pager(&content, pager_cmd, width)
                } else {
                    content
                }
            }
            _ => content,
        }
    }

    /// Compute raw preview for any mode.
    pub(super) fn compute_preview(
        repo: &Repository,
        item: &ListItem,
        mode: PreviewMode,
        width: usize,
        height: usize,
    ) -> String {
        match mode {
            PreviewMode::WorkingTree => Self::compute_working_tree_preview(repo, item, width),
            PreviewMode::Log => Self::compute_log_preview(repo, item, width, height),
            PreviewMode::BranchDiff => Self::compute_branch_diff_preview(repo, item, width),
            PreviewMode::UpstreamDiff => Self::compute_upstream_diff_preview(repo, item, width),
            PreviewMode::Summary => Self::loading_placeholder(PreviewMode::Summary),
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
    fn compute_branch_diff_preview(repo: &Repository, item: &ListItem, width: usize) -> String {
        let branch = item.branch_name();
        let reset = Reset;
        let Some(default_branch) = repo.default_branch() else {
            return cformat!(
                "{INFO_SYMBOL}{reset} <bold>{branch}</>{reset} has no commits ahead of main\n"
            );
        };

        let merge_base = format!("{}...{}", default_branch, item.head());
        compute_diff_preview(
            repo,
            &["diff", &merge_base],
            &cformat!(
                "{INFO_SYMBOL}{reset} <bold>{branch}</>{reset} has no file changes vs <bold>{default_branch}</>{reset}"
            ),
            width,
        )
    }

    /// Compute Tab 4: Upstream diff preview (ahead/behind vs tracking branch)
    ///
    /// Independent of `item.upstream` — a single
    /// `git rev-list --left-right --count HEAD...@{u}` probes both
    /// existence (non-zero exit when `@{u}` is unresolvable) and counts,
    /// so the preview is correct even before the list-row pipeline has
    /// populated upstream.
    fn compute_upstream_diff_preview(repo: &Repository, item: &ListItem, width: usize) -> String {
        let branch = item.branch_name();
        let reset = Reset;

        let upstream_ref = format!("{branch}@{{u}}");
        let probe_range = format!("{}...{upstream_ref}", item.head());
        let Ok(counts) = repo.run_command(&["rev-list", "--left-right", "--count", &probe_range])
        else {
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

        if ahead == 0 && behind == 0 {
            return cformat!(
                "{INFO_SYMBOL}{reset} <bold>{branch}</>{reset} is up to date with upstream\n"
            );
        }

        if ahead > 0 && behind > 0 {
            let range = format!("{}...{}", upstream_ref, item.head());
            compute_diff_preview(
                repo,
                &["diff", &range],
                &cformat!(
                    "{INFO_SYMBOL}{reset} <bold>{branch}</>{reset} has diverged (⇡{ahead} ⇣{behind}) but no unique file changes"
                ),
                width,
            )
        } else if ahead > 0 {
            let range = format!("{}...{}", upstream_ref, item.head());
            compute_diff_preview(
                repo,
                &["diff", &range],
                &cformat!(
                    "{INFO_SYMBOL}{reset} <bold>{branch}</>{reset} has no unpushed file changes"
                ),
                width,
            )
        } else {
            let range = format!("{}...{}", item.head(), upstream_ref);
            compute_diff_preview(
                repo,
                &["diff", &range],
                &cformat!(
                    "{INFO_SYMBOL}{reset} <bold>{branch}</>{reset} is behind upstream (⇣{behind}) but no file changes"
                ),
                width,
            )
        }
    }

    /// Compute log preview for a worktree item.
    /// This can be called from background threads for pre-computation.
    pub(super) fn compute_log_preview(
        repo: &Repository,
        item: &ListItem,
        width: usize,
        height: usize,
    ) -> String {
        // Minimum preview width to show timestamps (adds ~7 chars: space + 4-char time + space)
        // Note: preview is typically 50% of terminal width, so 50 = 100-col terminal
        const TIMESTAMP_WIDTH_THRESHOLD: usize = 50;
        // Tab header takes 3 lines (tabs + controls + blank)
        const HEADER_LINES: usize = 3;

        let mut output = String::new();
        let show_timestamps = width >= TIMESTAMP_WIDTH_THRESHOLD;
        // Calculate how many log lines fit in preview (height minus header)
        let log_limit = height.saturating_sub(HEADER_LINES).max(1);
        let head = item.head();
        let branch = item.branch_name();
        let reset = Reset;
        let Some(default_branch) = repo.default_branch() else {
            output.push_str(&cformat!(
                "{INFO_SYMBOL}{reset} <bold>{branch}</>{reset} has no commits\n"
            ));
            return output;
        };

        // Get merge-base with default branch
        //
        // Note on error handling: This code runs in an interactive preview pane that updates
        // on every keystroke. We intentionally use silent fallbacks rather than propagating
        // errors to avoid disruptive error messages during navigation. The preview is
        // supplementary - users can still select worktrees even if preview fails.
        //
        // Alternative: Check specific conditions (default branch exists, valid HEAD, etc.) before
        // running git commands. This would provide better diagnostics but adds latency to
        // every preview render. Trade-off: simplicity + speed vs. detailed error messages.
        let Ok(merge_base_output) = repo.run_command(&["merge-base", &default_branch, head]) else {
            output.push_str(&cformat!(
                "{INFO_SYMBOL}{reset} <bold>{branch}</>{reset} has no commits\n"
            ));
            return output;
        };

        let merge_base = merge_base_output.trim();
        let is_default_branch = branch == default_branch;

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

        // Get graph output (no --numstat to avoid blank continuation lines)
        let format: &str = if show_timestamps {
            &timestamp_format
        } else {
            no_timestamp_format
        };
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

        if let Ok(log_output) = repo.run_command(&args) {
            let (processed, hashes) =
                process_log_with_dimming(&log_output, unique_commits.as_ref());
            if show_timestamps {
                // Batch fetch stats for all commits
                let stats = batch_fetch_stats(repo, &hashes);
                output.push_str(&format_log_output(&processed, &stats));
            } else {
                // Strip hash markers (SOH...NUL) since we're not using format_log_output
                output.push_str(&strip_hash_markers(&processed));
            }
        }

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;

    #[test]
    fn test_render_preview_tabs() {
        // All PreviewMode variants: verify tab labels, active styling, and structure
        for (name, mode) in [
            ("working_tree", PreviewMode::WorkingTree),
            ("log", PreviewMode::Log),
            ("branch_diff", PreviewMode::BranchDiff),
            ("upstream_diff", PreviewMode::UpstreamDiff),
            ("summary", PreviewMode::Summary),
        ] {
            assert_snapshot!(name, WorktreeSkimItem::render_preview_tabs(mode));
        }
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
        // Test that ANSI escape sequences properly reset to prevent style bleeding
        let output = WorktreeSkimItem::render_preview_tabs(PreviewMode::WorkingTree);

        let first_line = output.lines().next().unwrap();
        let second_line = output.lines().nth(1).unwrap();

        // Each styled tab should end with a full reset (\x1b[0m) before the divider
        // This prevents bold/dim from bleeding into the " | " dividers
        let full_reset = "\x1b[0m";

        // Count resets - should have one after each of the 5 tabs
        assert_eq!(first_line.matches(full_reset).count(), 5);

        // The sequence should be: style + text + [22m + [0m + divider
        // Check that dividers come after full resets
        let parts: Vec<&str> = first_line.split(" | ").collect();
        assert_eq!(parts.len(), 5);
        assert!(parts.iter().all(|part| part.ends_with(full_reset)));

        // Controls line should end with full reset to ensure clean state for preview content
        assert!(second_line.ends_with(full_reset));
    }
}
