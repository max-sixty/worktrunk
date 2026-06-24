//! Skim item implementations.
//!
//! Wrappers for ListItem and header row that implement SkimItem for the interactive selector.

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use ansi_to_tui::IntoText;
use anstyle::Reset;
use color_print::cformat;
use dashmap::DashMap;
use ratatui::text::{Line, Span};
use skim::prelude::*;
use worktrunk::git::Repository;
use worktrunk::styling::{INFO_SYMBOL, visual_width};

use super::super::list::ci_status::{PrRef, PrStatus};
use super::super::list::model::ListItem;
use super::log_formatter::{
    FIELD_DELIM, batch_fetch_stats, format_log_output, process_log_with_dimming, strip_hash_markers,
};
use super::pager::{diff_pager, pipe_through_pager};
use super::pr_pane;
use super::preview::{PreviewMode, PreviewStateData};
use super::preview_cache;

/// Parse a pre-rendered ANSI string into a single ratatui `Line` for skim's
/// item list. skim's `DisplayContext::to_line` only applies match-highlight
/// styling to plain text, so the picker — whose rows already carry SGR color
/// codes — converts them itself. Rows are single lines; spans from any parsed
/// lines are flattened into one. A parse failure falls back to the raw text.
///
/// Each span's background is cleared to `None`. The rows are foreground-only,
/// but an `\x1b[0m` reset in the source ANSI parses to an explicit
/// `bg = Reset`, which would override skim's `highlight_line` fill and leave the
/// selected row's gray highlight (`current_bg`) full of holes. Leaving `bg`
/// unset lets the line-level current-row background show through uniformly.
pub(super) fn ansi_to_line(s: &str) -> Line<'static> {
    match s.into_text() {
        Ok(text) => Line::from(
            text.lines
                .into_iter()
                .flat_map(|line| line.spans)
                .map(|mut span| {
                    span.style.bg = None;
                    span
                })
                .collect::<Vec<Span<'static>>>(),
        ),
        Err(_) => Line::from(s.to_string()),
    }
}

/// Cache key for pre-computed previews: (branch_name, mode).
pub(super) type PreviewCacheKey = (String, PreviewMode);

/// Cache for pre-computed previews, keyed by (branch_name, mode).
/// Shared across all WorktreeSkimItems for background pre-computation.
pub(super) type PreviewCache = Arc<DashMap<PreviewCacheKey, String>>;

/// Per-row live `pr_status` for the `pr` tab, shared with the collect handler.
/// Primed from the CI cache at skeleton time, then overwritten as the live
/// `CiStatus` task reports (`progressive_handler::PickerHandler::on_update`).
/// `None` = the fetch hasn't reported yet (Loading); `Some(None)` = no PR;
/// `Some(Some(status))` = a PR/MR with status.
pub(super) type PrStatusSlot = Arc<Mutex<Option<Option<PrStatus>>>>;

/// Prefix on a worktree-backed item's `output()` token. Detached worktrees
/// all share the `(detached)` branch label, so `output()` returns the
/// worktree path (which is unique) behind this prefix instead.
pub(super) const WORKTREE_OUTPUT_PREFIX: &str = "worktree-path:";

/// A `--prs` picker's header shows a dim "loading…" line while the forge call
/// is still in flight, since its rows arrive (~1s) after the local rows. The
/// `--prs` thread clears `pending` and repaints once the fetch resolves (rows
/// sent, no PRs, or error). Absent (`None`) on non-`--prs` pickers, where every
/// row is present at skeleton.
pub(super) struct HeaderLoading {
    pub pending: Arc<AtomicBool>,
    /// Pre-rendered dim "loading…" line (ANSI). Shown *in place of* the column
    /// labels, not appended — a full-width header would clip the suffix.
    pub marker_ansi: String,
}

/// Header item for column names (non-selectable)
pub(super) struct HeaderSkimItem {
    pub display_text: String,
    pub display_text_with_ansi: String,
    pub loading: Option<HeaderLoading>,
}

impl SkimItem for HeaderSkimItem {
    fn text(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.display_text)
    }

    fn display(&self, _context: DisplayContext) -> Line<'_> {
        // While the --prs fetch is in flight, show the loading line in place of
        // the column labels — appending would clip off the right edge of a
        // full-width header. The labels return when the rows land.
        if let Some(loading) = &self.loading
            && loading.pending.load(Ordering::Relaxed)
        {
            return ansi_to_line(&loading.marker_ansi);
        }
        ansi_to_line(&self.display_text_with_ansi)
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
/// the ANSI-colored display string in place as task results arrive. skim 4.x
/// renders on demand, so the handler pokes an `Event::Render` after each
/// mutation (see `progressive_handler`); skim then re-reads `display()` and the
/// new value surfaces — no re-send through the item channel.
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
    /// Live CI status for the `pr` tab, shared with the collect handler. Unlike
    /// the frozen `item` snapshot (whose `pr_status` never updates after
    /// skeleton), this slot is primed from the cache and then overwritten as the
    /// `CiStatus` task streams in — so the `pr` tab reflects the live fetch.
    pub pr_status: PrStatusSlot,
}

impl SkimItem for WorktreeSkimItem {
    fn text(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.search_text)
    }

    fn display(&self, _context: DisplayContext) -> Line<'_> {
        // Clone-under-lock so the parser's input outlives the guard;
        // `ansi_to_line` returns an owned `Line<'static>`.
        let snapshot = self.rendered.lock().unwrap().clone();
        ansi_to_line(&snapshot)
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
        // The mode is the only render input that comes from outside the item
        // (it's per-process picker state); everything else is derived in
        // `render_preview`, which takes the mode explicitly so it's testable
        // without touching that global state.
        let mode = PreviewStateData::read_mode();
        ItemPreview::AnsiText(self.render_preview(mode, context.width, context.height))
    }
}

/// The `pr` tab's state for a worktree row, derived from the row's live CI
/// status slot ([`WorktreeSkimItem::pr_status`]).
enum PrPreview {
    /// The live CI fetch hasn't reported for this branch yet, and no cache
    /// primed it — the tab shows a "fetching" hint rather than claiming the
    /// branch has no PR.
    Loading,
    /// The fetch reported no PR (no CI, or a branch workflow with no PR/MR
    /// number) — the tab dims and shows "… has no PR".
    NoPr,
    /// The branch has a PR/MR. The `PrRef` rides alongside the status so the
    /// reference is structurally guaranteed (a status with no `number` is
    /// `NoPr`, not `HasPr`) — `render_worktree_pr` needs no fallback.
    HasPr(PrRef, PrStatus),
}

/// Which preview tabs have renderable content for the selected row.
///
/// Empty tabs are de-emphasized in the bar (number dimmed). Skim computes a
/// preview once per selection and cannot re-query it mid-selection (see
/// `loading_placeholder`). `upstream` and `summary` are synchronous
/// skeleton-time facts, read to avoid locking the bar into a stale state:
/// `upstream` reads `Repository::local_branches()` (the pre-skeleton
/// `for-each-ref` scan), NOT the async `item.upstream`; `summary` reads the
/// process-wide `[commit.generation]` flag, NOT the async `item.summary`. `pr`
/// reads the row's live status slot (primed from the CI cache, then refreshed
/// by the `CiStatus` task — see [`WorktreeSkimItem::pr_status`]), so it can
/// change between selections: it dims once the fetch reports no PR, and stays
/// available while loading or with a PR. `--prs` rows always render their PR.
#[derive(Debug, Clone, Copy)]
pub(super) struct TabAvailability {
    working_tree: bool,
    log: bool,
    branch_diff: bool,
    upstream: bool,
    summary: bool,
    pr: bool,
    comments: bool,
}

impl TabAvailability {
    /// A worktree-backed row: working-tree/log/branch-diff always render; the
    /// upstream and summary tabs depend on synchronous skeleton-time facts; the
    /// `pr` tab is available unless the live fetch has reported no PR for this
    /// branch (see [`WorktreeSkimItem::pr_preview`]). The `comments` tab is
    /// always empty here — comments are fetched only for `--prs` rows.
    pub(super) fn worktree(has_upstream: bool, summaries_enabled: bool, pr: bool) -> Self {
        Self {
            working_tree: true,
            log: true,
            branch_diff: true,
            upstream: has_upstream,
            summary: summaries_enabled,
            pr,
            comments: false,
        }
    }

    /// A `--prs` row: it carries no local worktree, so the working-tree,
    /// branch-diff, upstream, and summary tabs are empty. The `pr` tab is built
    /// at construction; the `log` and `comments` tabs load in the background
    /// (see `prs::compute_pr_log` / `prs::compute_pr_comments`), so all three
    /// are available.
    pub(super) fn pull_request() -> Self {
        Self {
            working_tree: false,
            log: true,
            branch_diff: false,
            upstream: false,
            summary: false,
            pr: true,
            comments: true,
        }
    }
}

/// One preview tab's identity: its `alt-N` digit, label, and whether it's the
/// active mode / has content for the current row. Built once, then rendered
/// full or compact depending on the pane width.
struct Tab {
    number: u8,
    label: &'static str,
    is_active: bool,
    has_content: bool,
}

/// Render the preview tab bar, shared by worktree rows and `--prs` rows.
///
/// Every full-form tab keeps its `N: label` text — only the formatting varies,
/// so the accelerators stay discoverable. Two **orthogonal** signals carry
/// through the styling: the **number's** brightness says whether the tab is
/// selectable (normal = has content, dim = empty for this row — see
/// `TabAvailability`), and the **label's** weight says whether it's the active
/// mode (bold = active, dim = inactive). The two compose independently: an empty
/// tab dims its number whether or not it's selected, but the active tab's label
/// still bolds — so an active-but-empty tab (dim number, bold label) stays
/// distinct from an inactive-empty one (dim number, dim label), and the selected
/// tab is always identifiable even when it has nothing to show (its pane, e.g.
/// "… has no PR", says the rest).
///
/// **Width adaptation.** skim renders previews with wrapping off (its default),
/// so a tab bar wider than `width` would truncate on the right — and the `pr` /
/// `comments` tabs, exactly the ones with content on a `--prs` row, sit at that
/// end. When the seven full-form tabs don't fit, the bar falls back to a compact
/// form (`1 2: log 3 …`): every accelerator digit stays visible, but only the
/// active tab keeps its label. The two style signals survive — empty digits dim,
/// the active digit+label bolds — so navigation works at any width. `width` is
/// the preview pane width skim reports.
pub(super) fn render_preview_tabs(
    mode: PreviewMode,
    avail: TabAvailability,
    width: usize,
) -> String {
    let reset = Reset;

    let tabs = [
        Tab {
            number: 1,
            label: "HEAD±",
            is_active: mode == PreviewMode::WorkingTree,
            has_content: avail.working_tree,
        },
        Tab {
            number: 2,
            label: "log",
            is_active: mode == PreviewMode::Log,
            has_content: avail.log,
        },
        Tab {
            number: 3,
            label: "main…±",
            is_active: mode == PreviewMode::BranchDiff,
            has_content: avail.branch_diff,
        },
        Tab {
            number: 4,
            label: "remote⇅",
            is_active: mode == PreviewMode::UpstreamDiff,
            has_content: avail.upstream,
        },
        Tab {
            number: 5,
            label: "summary",
            is_active: mode == PreviewMode::Summary,
            has_content: avail.summary,
        },
        Tab {
            number: 6,
            label: "pr",
            is_active: mode == PreviewMode::Pr,
            has_content: avail.pr,
        },
        Tab {
            number: 7,
            label: "comments",
            is_active: mode == PreviewMode::Comments,
            has_content: avail.comments,
        },
    ];

    // Prefer the full bar; fall back to compact only when it would overflow the
    // pane (`visual_width` measures the styled string with ANSI stripped).
    let full = render_tab_row_full(&tabs, reset);
    let bar = if visual_width(&full) <= width {
        full
    } else {
        render_tab_row_compact(&tabs, reset)
    };

    // Controls use dim yellow to distinguish from dimmed (white) tabs.
    // The tab numbers above are the alt-N accelerators (bare digits type
    // into the query); Tab/shift-tab cycle the same tabs.
    //
    // The controls line is intentionally NOT width-managed: skim clips it on the
    // right on a narrow pane, but it's only a reminder — the accelerators it
    // names live in the tab bar above, which IS width-managed, so nothing
    // navigable is lost when the tail clips.
    let controls = cformat!(
        "<dim,yellow>Enter: switch | Tab/alt-1…7: preview | alt-c: create | Esc: cancel | ctrl-u/d: scroll | alt-p: toggle</>"
    );

    // Each tab/segment already ends with a full reset (so styling never bleeds
    // into the dividers or preview content); `{reset}` here only closes the
    // controls line's dim-yellow span.
    format!("{bar}\n{controls}{reset}\n\n")
}

/// Full tab bar: `N: label` per tab, ` | `-separated. The number dims when the
/// tab is empty (not selectable), and the label bolds on the active tab — two
/// orthogonal signals that compose (see [`render_preview_tabs`]).
fn render_tab_row_full(tabs: &[Tab], reset: Reset) -> String {
    tabs.iter()
        .map(|t| {
            let number = if t.has_content {
                format!("{}:", t.number)
            } else {
                cformat!("<dim>{}:</>", t.number)
            };
            let label = if t.is_active {
                cformat!("<bold>{}</>", t.label)
            } else {
                cformat!("<dim>{}</>", t.label)
            };
            format!("{number} {label}{reset}")
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

/// Compact tab bar for narrow panes: just the digits, space-separated, with the
/// active tab keeping its label (`1 2: log 3 …`). The number still dims when
/// empty and the active digit+label bolds, so both style signals survive and
/// every accelerator stays visible — only the inactive labels drop.
fn render_tab_row_compact(tabs: &[Tab], reset: Reset) -> String {
    tabs.iter()
        .map(|t| {
            if t.is_active {
                // Active: `N: label`, with the number bold (and dim too when empty).
                let number = if t.has_content {
                    cformat!("<bold>{}:</>", t.number)
                } else {
                    cformat!("<dim,bold>{}:</>", t.number)
                };
                format!("{number} {}{reset}", cformat!("<bold>{}</>", t.label))
            } else {
                // Inactive: just the digit, dim when empty.
                let number = if t.has_content {
                    t.number.to_string()
                } else {
                    cformat!("<dim>{}</>", t.number)
                };
                format!("{number}{reset}")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The `pr` pane for a worktree row whose branch has a PR/MR. Renders the same
/// shape as the `--prs` rows' pane (`PrSkimItem::pr_pane`) — reference + title
/// header, dim-labeled metadata, and the description as markdown — via the
/// shared [`pr_pane`] helpers, so the two read alike. The title, body, and
/// comment count ride the same CI fetch the column already makes (see
/// [`PrStatus`]); a status without them (an older cache entry, a forge that
/// doesn't expose them) falls back to a reference-only header and skips the
/// missing lines.
///
/// The `comments` line shows only the count — the full thread is a `--prs`-only
/// fetch (see [`Self::comments_unavailable_pane`]). A PR with no comments
/// carries `None` (zero is flattened at the mapping boundary), so the line is
/// skipped.
///
/// `width` is the live preview-pane width, used to wrap the markdown body.
fn render_worktree_pr(branch: &str, pr_ref: PrRef, status: &PrStatus, width: usize) -> String {
    let title = status.title.as_deref().filter(|t| !t.is_empty());
    let mut out = pr_pane::header(pr_ref, title);
    out.push_str(&pr_pane::metadata_line("branch", branch));
    if let Some(url) = &status.url {
        out.push_str(&pr_pane::metadata_line("url", url));
    }
    if let Some(count) = status.comment_count {
        out.push_str(&pr_pane::metadata_line("comments", &count.to_string()));
    }
    if let Some(body) = status.body.as_deref() {
        out.push_str(&pr_pane::description(body, width));
    }
    out
}

impl WorktreeSkimItem {
    /// Render the full preview pane (tab bar + mode content) for an explicit
    /// `mode`. Split out of [`SkimItem::preview`] so the dispatch — including
    /// the `pr` tab's `render_pr_pane` call — is testable with a given mode
    /// rather than the per-process picker-state file.
    fn render_preview(&self, mode: PreviewMode, width: usize, height: usize) -> String {
        // Build preview: tabs header + content. `has_upstream` and
        // `summaries_enabled` are synchronous skeleton-time facts (see
        // `TabAvailability`); `pr` reads the row's live status slot, refreshed as
        // the `CiStatus` task streams in, so it can change between selections.
        // The `pr` tab dims only once the fetch reports no PR — while loading or
        // with a PR it stays available. `pr_tab_available` is a cheap
        // discriminant read (no body clone); the pane itself is rendered once and
        // memoized in `render_pr_pane_cached`.
        let avail = TabAvailability::worktree(
            self.has_upstream,
            self.summaries_enabled,
            self.pr_tab_available(),
        );
        let mut result = render_preview_tabs(mode, avail, width);
        result.push_str(&match mode {
            PreviewMode::Pr => self.render_pr_pane_cached(width),
            // The `comments` tab (7) is `--prs`-only — worktree rows don't fetch
            // PR discussion — so it shows a pointer rather than the cache path.
            PreviewMode::Comments => Self::comments_unavailable_pane(),
            _ => self.preview_for_mode(mode, width, height),
        });
        result
    }

    /// Derive the `pr` tab's state from the row's live `pr_status` slot. The
    /// slot is primed from the CI cache at skeleton time and overwritten as the
    /// `CiStatus` task reports: `None` = no result yet (Loading); `Some(None)`,
    /// or a status carrying no PR/MR `number` (a bare branch workflow), = no PR;
    /// a status with a `number` = a real PR.
    fn pr_preview(&self) -> PrPreview {
        match &*self.pr_status.lock().unwrap() {
            None => PrPreview::Loading,
            Some(None) => PrPreview::NoPr,
            Some(Some(status)) => match status.number {
                Some(pr_ref) => PrPreview::HasPr(pr_ref, status.clone()),
                None => PrPreview::NoPr,
            },
        }
    }

    /// Whether the `pr` tab has content for this row — it dims only once the
    /// live fetch reports no PR. A cheap discriminant read of the `pr_status`
    /// slot (no `PrStatus` clone, unlike [`Self::pr_preview`]), so the tab bar
    /// can be drawn on every `preview()` call without re-cloning the
    /// possibly-large body that [`Self::render_pr_pane_cached`] already memoizes.
    fn pr_tab_available(&self) -> bool {
        match &*self.pr_status.lock().unwrap() {
            None => true,                                  // still fetching
            Some(None) => false,                           // fetch reported no PR
            Some(Some(status)) => status.number.is_some(), // a bare branch workflow is "no PR"
        }
    }

    /// Render the `pr` pane, memoized in the shared `preview_cache` so repeated
    /// `preview()` calls (every keystroke while the tab is active) don't re-run
    /// the markdown render of the body. The other modes are pre-computed into
    /// this same cache by the orchestrator; the `pr` pane is filled lazily here
    /// instead because its inputs aren't known at skeleton time — they stream in
    /// on the `CiStatus` task. The collect handler's `on_update` removes this
    /// entry whenever the live `pr_status` slot changes (fetch lands, manual
    /// refresh), so a cache hit is always consistent with the current slot.
    fn render_pr_pane_cached(&self, width: usize) -> String {
        let key = (self.branch_name.clone(), PreviewMode::Pr);
        if let Some(cached) = self.preview_cache.get(&key) {
            return cached.value().clone();
        }
        let rendered = self.render_pr_pane(self.pr_preview(), width);
        self.preview_cache.insert(key, rendered.clone());
        rendered
    }

    /// Render the `pr` tab's pane from the derived [`PrPreview`] state. `width`
    /// is the live preview-pane width, threaded to wrap the description markdown.
    fn render_pr_pane(&self, pr: PrPreview, width: usize) -> String {
        let reset = Reset;
        let branch = self.item.branch_name();
        match pr {
            PrPreview::Loading => cformat!(
                "○ Fetching PR status for <bold>{branch}</>{reset}… press <bold>alt-6</>{reset} to refresh\n"
            ),
            PrPreview::NoPr => {
                cformat!("{INFO_SYMBOL}{reset} <bold>{branch}</>{reset} has no PR\n")
            }
            PrPreview::HasPr(pr_ref, status) => render_worktree_pr(branch, pr_ref, &status, width),
        }
    }

    /// The `comments` tab's pane on a worktree row. The discussion thread is
    /// fetched only for `--prs` rows (a worktree row has no PR number until its
    /// `pr` tab resolves one, and no background comments fetch wired to it), so
    /// the tab dims and points at where comments live.
    fn comments_unavailable_pane() -> String {
        let reset = Reset;
        cformat!("{INFO_SYMBOL}{reset} Comments show on <bold>wt switch --prs</>{reset} rows\n")
    }

    /// Render preview for the given mode with specified dimensions.
    ///
    /// Pure cache read: skim invokes `preview()` synchronously while drawing
    /// the preview pane, so any blocking here gates the render. Background
    /// tasks populate the cache out-of-band; a miss returns a placeholder, and
    /// skim re-queries on the next selection/query change.
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
    /// preview for this mode. Skim has no API to re-query the preview from
    /// outside user interaction, so the hint tells the user to press the mode
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
            // The `pr` and `comments` tabs have no worktree-row background task —
            // `preview()` renders `pr` from the cached `pr_status` via
            // `render_pr_pane` and routes `comments` to `comments_unavailable_pane`,
            // never here.
            PreviewMode::Pr => unreachable!("pr tab renders via render_pr_pane"),
            PreviewMode::Comments => {
                unreachable!("comments tab renders via comments_unavailable_pane")
            }
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
            // PR and comments previews never precompute on worktree rows (no
            // git/LLM work) — the orchestrator never spawns these modes, and
            // `preview()` renders them directly (cached `pr_status` / the
            // `--prs`-only pointer).
            PreviewMode::Pr => unreachable!("pr tab is never precomputed"),
            PreviewMode::Comments => unreachable!("comments tab is never precomputed"),
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
        Self::compute_log_preview_inner(repo, item.head(), item.branch_name(), width, height, false)
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
        Self::compute_log_preview_inner(repo, item.head(), item.branch_name(), width, height, true)
            .0
    }

    /// Render the rich local `git log` for an arbitrary `(head, branch)` pair —
    /// the same graph + dim/bright merge-base split + timestamps the worktree
    /// rows show. Used by the `--prs` `log` tab for a row whose head commit is
    /// already in the local object store (a same-repo PR off a fetched
    /// `origin`); see `prs::compute_pr_log`. Discards the disk-hit flag — a
    /// `--prs` row's preview renders once with no background-refresh loop, so
    /// there's nothing to reschedule.
    pub(super) fn compute_log_for_head(
        repo: &Repository,
        head: &str,
        branch: &str,
        width: usize,
        height: usize,
    ) -> String {
        Self::compute_log_preview_inner(repo, head, branch, width, height, false).0
    }

    fn compute_log_preview_inner(
        repo: &Repository,
        head: &str,
        branch: &str,
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
    use ansi_str::AnsiStr;
    use insta::assert_snapshot;

    /// Width at which all seven full-form tabs fit, so the snapshots capture the
    /// full bar (the compact fallback has its own test).
    const WIDE: usize = 200;

    #[test]
    fn test_render_preview_tabs() {
        // Each mode active, on a worktree row with upstream + summaries enabled
        // but no PR (tabs 1-5 available; tabs 6 pr and 7 comments dim). The
        // active mode's label is bold, inactive available labels dim, and on the
        // `pr` iteration tab 6 is active-but-empty — exercising the rule that
        // emptiness dims even the active tab. Verifies labels and structure.
        let wt = TabAvailability::worktree(true, true, false);
        for (name, mode) in [
            ("working_tree", PreviewMode::WorkingTree),
            ("log", PreviewMode::Log),
            ("branch_diff", PreviewMode::BranchDiff),
            ("upstream_diff", PreviewMode::UpstreamDiff),
            ("summary", PreviewMode::Summary),
            ("pr", PreviewMode::Pr),
        ] {
            assert_snapshot!(name, render_preview_tabs(mode, wt, WIDE));
        }

        // Empty states: a worktree row with no upstream and summaries disabled
        // dims tabs 4 and 5 (number dim too); a --prs row dims the
        // working-tree/branch-diff/upstream/summary tabs but keeps log/pr/comments.
        assert_snapshot!(
            "empty_upstream_and_summary",
            render_preview_tabs(
                PreviewMode::WorkingTree,
                TabAvailability::worktree(false, false, false),
                WIDE,
            )
        );
        assert_snapshot!(
            "pr_row",
            render_preview_tabs(PreviewMode::Pr, TabAvailability::pull_request(), WIDE)
        );
    }

    /// Narrow panes can't fit the full bar (skim renders previews with wrapping
    /// off, so it would truncate the `pr` / `comments` tabs — exactly the ones
    /// with content on a `--prs` row). The compact fallback keeps every
    /// accelerator digit visible and labels only the active tab.
    #[test]
    fn render_preview_tabs_compacts_when_narrow() {
        // A --prs row with the `pr` tab active, in a narrow pane.
        let compact = render_preview_tabs(PreviewMode::Pr, TabAvailability::pull_request(), 40);
        let plain = compact.lines().next().unwrap().ansi_strip().to_string();
        // Every digit 1-7 is present; only the active tab keeps its label.
        for n in 1..=7 {
            assert!(
                plain.contains(&n.to_string()),
                "digit {n} present: {plain:?}"
            );
        }
        assert!(plain.contains("6: pr"), "active tab labeled: {plain:?}");
        assert!(
            !plain.contains("comments"),
            "inactive label dropped: {plain:?}"
        );
        assert!(!plain.contains("HEAD"), "inactive label dropped: {plain:?}");
        // The compact bar fits a narrow pane where the full bar would not.
        assert!(visual_width(&plain) <= 40, "fits the pane: {plain:?}");

        // The same row in a wide pane uses the full bar (labels for all tabs).
        let full = render_preview_tabs(PreviewMode::Pr, TabAvailability::pull_request(), WIDE);
        assert!(
            full.contains("7: ") && full.contains("comments"),
            "wide pane keeps full labels"
        );

        // Boundary: the switch is `visual_width(full) <= width`. Measure the full
        // bar's own width, then check that exactly that width stays full while one
        // column narrower compacts (the `pr` tab is active, so only the full bar
        // carries the inactive `comments` label).
        let avail = TabAvailability::pull_request();
        let full_w = visual_width(full.lines().next().unwrap());
        let at_fit = render_preview_tabs(PreviewMode::Pr, avail, full_w);
        assert!(
            at_fit.contains("comments"),
            "full bar at exact-fit width: {at_fit:?}"
        );
        let one_under = render_preview_tabs(PreviewMode::Pr, avail, full_w - 1);
        assert!(
            !one_under.contains("comments"),
            "compacts one column under: {one_under:?}"
        );
    }

    #[test]
    fn header_loading_marker_shows_until_cleared() {
        // `--prs` mode: the header carries a dim "loading…" marker while the
        // forge call is in flight, dropped once the `--prs` thread clears the
        // shared flag.
        let pending = Arc::new(AtomicBool::new(true));
        let header = HeaderSkimItem {
            display_text: "Branch  CI".to_string(),
            display_text_with_ansi: "Branch  CI".to_string(),
            loading: Some(HeaderLoading {
                pending: Arc::clone(&pending),
                marker_ansi: "  loading open PRs…".to_string(),
            }),
        };
        let text = |h: &HeaderSkimItem| {
            h.display(DisplayContext::default())
                .spans
                .iter()
                .map(|s| s.content.as_ref().to_string())
                .collect::<String>()
        };

        assert!(
            text(&header).contains("loading open PRs"),
            "marker shows while pending"
        );

        pending.store(false, Ordering::Relaxed);
        let cleared = text(&header);
        assert!(
            !cleared.contains("loading"),
            "marker gone once cleared: {cleared:?}"
        );
        assert!(
            cleared.contains("Branch"),
            "column header remains: {cleared:?}"
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
            // `Pr` isn't backed by the preview cache — it renders from the
            // row's live `pr_status` slot via `render_pr_pane`, so the
            // `loading_placeholder` path never covers it.
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
                pr_status: Arc::new(Mutex::new(None)),
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
                pr_status: Arc::new(Mutex::new(None)),
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

    #[test]
    fn worktree_pr_tab_reflects_live_status() {
        use crate::commands::list::ci_status::{CiSource, CiStatus, PrRef, ReviewState};

        let status = |number: Option<PrRef>,
                      url: Option<&str>,
                      title: Option<&str>,
                      body: Option<&str>| PrStatus {
            ci_status: CiStatus::Passed,
            source: CiSource::PullRequest,
            is_stale: false,
            is_priming: false,
            url: url.map(String::from),
            number,
            review_state: Some(ReviewState::Approved),
            title: title.map(String::from),
            body: body.map(String::from),
            comment_count: None,
        };
        // Build a row whose live `pr_status` slot carries a given state — what
        // the picker primes from the cache and then overwrites as the fetch lands.
        let row = |pr_status: Option<Option<PrStatus>>| {
            let item = ListItem::new_branch("abc".into(), "feature".into());
            WorktreeSkimItem {
                search_text: String::new(),
                rendered: Arc::new(Mutex::new(String::new())),
                branch_name: "feature".into(),
                item: Arc::new(item),
                preview_cache: Arc::new(DashMap::new()),
                has_upstream: false,
                summaries_enabled: false,
                pr_status: Arc::new(Mutex::new(pr_status)),
            }
        };

        // No result yet (outer `None`) → Loading; the pane shows a fetching hint.
        let loading = row(None);
        assert!(matches!(loading.pr_preview(), PrPreview::Loading));
        assert!(
            loading
                .render_pr_pane(loading.pr_preview(), 80)
                .contains("Fetching PR status")
        );

        // Cache reports no PR (`Some(None)`) → NoPr.
        let no_pr = row(Some(None));
        assert!(matches!(no_pr.pr_preview(), PrPreview::NoPr));
        assert!(
            no_pr
                .render_pr_pane(no_pr.pr_preview(), 80)
                .contains("has no PR")
        );

        // A cached PR with no title/body (an older cache entry) → the pane still
        // shows the reference, branch, and URL, with no description block.
        let bare = row(Some(Some(status(
            Some(PrRef::pr(42)),
            Some("https://github.com/o/r/pull/42"),
            None,
            None,
        ))));
        assert!(matches!(bare.pr_preview(), PrPreview::HasPr(..)));
        let pane = bare.render_pr_pane(bare.pr_preview(), 80);
        assert!(pane.contains("#42"), "reference: {pane:?}");
        assert!(pane.contains("feature"), "branch: {pane:?}");
        assert!(
            pane.contains("https://github.com/o/r/pull/42"),
            "url: {pane:?}"
        );
        assert!(
            !pane.contains("\x1b[107m"),
            "no description gutter without a body: {pane:?}"
        );

        // A PR carrying title + body → the title rides the header and the body
        // renders as markdown in the house gutter, matching the `--prs` pane.
        let full = row(Some(Some(status(
            Some(PrRef::pr(7)),
            Some("https://github.com/o/r/pull/7"),
            Some("Fix the flaky retry"),
            Some("Adds a **bounded** retry."),
        ))));
        let pane = full.render_pr_pane(full.pr_preview(), 80);
        assert!(pane.contains("#7"), "reference: {pane:?}");
        assert!(pane.contains("Fix the flaky retry"), "title: {pane:?}");
        assert!(pane.contains("\x1b[107m"), "description gutter: {pane:?}");
        assert!(pane.contains("\x1b[1m"), "markdown bold rendered: {pane:?}");
        assert!(!pane.contains("**"), "markdown markers consumed: {pane:?}");

        // Cached branch CI with no PR `number` → NoPr.
        let branch_ci = row(Some(Some(status(None, None, None, None))));
        assert!(matches!(branch_ci.pr_preview(), PrPreview::NoPr));
    }

    #[test]
    fn worktree_pr_pane_shows_comment_count() {
        use crate::commands::list::ci_status::{CiSource, CiStatus, PrRef};

        let status = |comment_count: Option<u32>| PrStatus {
            ci_status: CiStatus::Passed,
            source: CiSource::PullRequest,
            is_stale: false,
            is_priming: false,
            url: Some("https://github.com/o/r/pull/7".into()),
            number: Some(PrRef::pr(7)),
            review_state: None,
            title: Some("Fix the flaky retry".into()),
            body: None,
            comment_count,
        };

        // A PR with comments adds a `comments` metadata line carrying the count.
        let with = render_worktree_pr("feature", PrRef::pr(7), &status(Some(3)), 80)
            .ansi_strip()
            .to_string();
        assert!(
            with.lines()
                .any(|l| l.contains("comments") && l.contains('3')),
            "comments line with count: {with:?}"
        );

        // No comments (zero is flattened to `None` at the mapping boundary, and an
        // older cache entry carries `None` too) → the line is skipped entirely.
        let without = render_worktree_pr("feature", PrRef::pr(7), &status(None), 80)
            .ansi_strip()
            .to_string();
        assert!(
            !without.contains("comments"),
            "no comments line when absent: {without:?}"
        );
    }

    #[test]
    fn preview_assembles_tab_bar_and_pr_pane() {
        use crate::commands::list::ci_status::{CiSource, CiStatus, PrRef, ReviewState};

        // A worktree row whose live status carries a PR with title + body.
        let item = ListItem::new_branch("abc".into(), "feature".into());
        let row = WorktreeSkimItem {
            search_text: String::new(),
            rendered: Arc::new(Mutex::new(String::new())),
            branch_name: "feature".into(),
            item: Arc::new(item),
            preview_cache: Arc::new(DashMap::new()),
            has_upstream: false,
            summaries_enabled: false,
            pr_status: Arc::new(Mutex::new(Some(Some(PrStatus {
                ci_status: CiStatus::Passed,
                source: CiSource::PullRequest,
                is_stale: false,
                is_priming: false,
                url: Some("https://github.com/o/r/pull/7".into()),
                number: Some(PrRef::pr(7)),
                review_state: Some(ReviewState::Approved),
                title: Some("Fix the flaky retry".into()),
                body: Some("Adds a **bounded** retry.".into()),
                comment_count: None,
            })))),
        };

        // In Pr mode, `render_preview` assembles the tab bar plus the worktree PR
        // pane — the dispatch arm `SkimItem::preview` reaches once the picker-state
        // file selects mode 6. The pane shows the title and the markdown body.
        let pr_pane = row.render_preview(PreviewMode::Pr, 80, 24);
        // Strip ANSI before checking the tab labels: the active `pr` tab is bold,
        // so `6: pr` is split by an SGR escape in the raw string (the bar's own
        // test, `test_render_preview_tabs`, snapshots the styled form).
        let bar = pr_pane.ansi_strip().to_string();
        assert!(bar.contains("6: pr"), "pr tab: {bar:?}");
        assert!(bar.contains("7: comments"), "comments tab: {bar:?}");
        assert!(
            pr_pane.contains("Fix the flaky retry"),
            "title: {pr_pane:?}"
        );
        assert!(
            pr_pane.contains("\x1b[107m"),
            "description gutter: {pr_pane:?}"
        );

        // `SkimItem::preview` reads the default mode (no per-process state file in
        // tests → WorkingTree) and delegates to `render_preview`, exercising the
        // wrapper and the non-pr dispatch arm (cache miss → loading placeholder).
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
        let ItemPreview::AnsiText(text) = row.preview(ctx) else {
            panic!("expected AnsiText preview");
        };
        assert!(text.contains("HEAD"), "tab bar present: {text:?}");
        assert!(
            text.contains("Loading working-tree diff"),
            "non-pr arm placeholder: {text:?}"
        );
    }

    #[test]
    fn pr_pane_is_memoized_until_the_slot_is_invalidated() {
        use crate::commands::list::ci_status::{CiSource, CiStatus, PrRef, ReviewState};

        let status = |title: &str| {
            Some(Some(PrStatus {
                ci_status: CiStatus::Passed,
                source: CiSource::PullRequest,
                is_stale: false,
                is_priming: false,
                url: Some("https://github.com/o/r/pull/7".into()),
                number: Some(PrRef::pr(7)),
                review_state: Some(ReviewState::Approved),
                title: Some(title.into()),
                body: Some("body".into()),
                comment_count: None,
            }))
        };
        // Share the cache and slot Arcs so the test can mutate the slot and
        // invalidate the entry the way the collect handler's `on_update` does.
        let cache: PreviewCache = Arc::new(DashMap::new());
        let slot: PrStatusSlot = Arc::new(Mutex::new(status("First title")));
        let key = ("feature".to_string(), PreviewMode::Pr);
        let row = WorktreeSkimItem {
            search_text: String::new(),
            rendered: Arc::new(Mutex::new(String::new())),
            branch_name: "feature".into(),
            item: Arc::new(ListItem::new_branch("abc".into(), "feature".into())),
            preview_cache: Arc::clone(&cache),
            has_upstream: false,
            summaries_enabled: false,
            pr_status: Arc::clone(&slot),
        };

        // First render populates the shared cache.
        assert!(
            row.render_preview(PreviewMode::Pr, 80, 24)
                .contains("First title")
        );
        assert!(cache.contains_key(&key), "render populates the cache");

        // The slot changes but the entry is NOT invalidated → the cached render
        // is reused, proving the markdown render didn't re-run.
        *slot.lock().unwrap() = status("Second title");
        let served = row.render_preview(PreviewMode::Pr, 80, 24);
        assert!(
            served.contains("First title"),
            "served from cache: {served:?}"
        );
        assert!(
            !served.contains("Second title"),
            "not re-rendered: {served:?}"
        );

        // Invalidating the entry (what `on_update` does on a slot change) makes
        // the next render reflect the new slot.
        cache.remove(&key);
        let refreshed = row.render_preview(PreviewMode::Pr, 80, 24);
        assert!(
            refreshed.contains("Second title"),
            "re-rendered after invalidation: {refreshed:?}"
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
        // The per-tab `{reset}` is appended in the full bar regardless of a
        // tab's internal styling, so the reset/divider counts hold whether a tab
        // is active (bold label), inactive-available (dim label), or empty (dim
        // number + label — here tabs 6 pr and 7 comments). WIDE forces the full
        // (not compact) bar.
        let output = render_preview_tabs(
            PreviewMode::WorkingTree,
            TabAvailability::worktree(true, true, false),
            WIDE,
        );

        let first_line = output.lines().next().unwrap();
        let second_line = output.lines().nth(1).unwrap();

        // Each styled tab should end with a full reset (\x1b[0m) before the divider
        // This prevents bold/dim from bleeding into the " | " dividers
        let full_reset = "\x1b[0m";

        // Count resets - should have one after each of the 7 tabs
        assert_eq!(first_line.matches(full_reset).count(), 7);

        // The sequence should be: style + text + [22m + [0m + divider
        // Check that dividers come after full resets
        let parts: Vec<&str> = first_line.split(" | ").collect();
        assert_eq!(parts.len(), 7);
        assert!(parts.iter().all(|part| part.ends_with(full_reset)));

        // Controls line should end with full reset to ensure clean state for preview content
        assert!(second_line.ends_with(full_reset));
    }
}
