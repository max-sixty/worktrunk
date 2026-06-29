//! Skim item implementations.
//!
//! The unified [`PickerRow`] (a branch/worktree row or a listed `--prs` row,
//! distinguished by `local: Option<LocalCheckout>`) and the header row, both
//! implementing `SkimItem` for the interactive selector.

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use ansi_to_tui::IntoText;
use anstyle::Reset;
use color_print::cformat;
use dashmap::DashMap;
use ratatui::style::{Color, Modifier};
use ratatui::text::{Line, Span};
use skim::prelude::*;
use worktrunk::git::Repository;
use worktrunk::styling::{HINT_SYMBOL, INFO_SYMBOL, visual_width};

use super::super::list::ci_status::{PrRef, PrStatus, ReviewState};
use super::super::list::model::ListItem;
use super::log_formatter::{
    FIELD_DELIM, batch_fetch_stats, format_log_output, process_log_with_dimming, strip_hash_markers,
};
use super::pager::{diff_pager, pipe_through_pager};
use super::pr_pane;
use super::preview::{PreviewMode, PreviewStateData};
use super::preview_cache;
use super::preview_notify::PreviewNotifier;

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

/// Foreground for faint text on the *selected* row.
///
/// A removable worktree (integrated / same commit as the default branch) renders
/// its branch and path gray — the "safe to delete" signal — via the `DIM`
/// modifier (foreground unset; see `list::render`). That works everywhere except
/// under the picker's cursor: skim paints the current row with
/// `current:237,current_bg:251` as a *line-level* style, so a `DIM` span inherits
/// foreground 237 and the faint attribute washes out against the light selection
/// background — the gray vanishes exactly when the row is highlighted. A
/// span-level foreground overrides the line-level `current`, so re-anchoring the
/// faint text to an explicit muted gray makes the de-emphasis read through the
/// selection. Gray 245 ≈ the perceived faint-gray off-cursor, so the row's look
/// is continuous as the cursor lands on it. See [`anchor_faint_under_selection`].
const SELECTED_FAINT_FG: Color = Color::Indexed(245);

/// Re-anchor a row's faint (`DIM`) spans to an explicit gray when it is the
/// selected row, so the de-emphasis survives skim's selection highlight.
///
/// Scoped to the current row only — `wt list` and unselected picker rows keep
/// their `DIM` styling untouched. skim tells the row its selection state through
/// `DisplayContext::base_style`: the `current` style (a concrete foreground) on
/// the cursor row, `normal` (foreground `Reset`) on every other. A concrete
/// foreground is therefore the "this row is selected" discriminator. Only faint
/// spans with no explicit foreground are touched (an already-colored span, e.g.
/// a green diff count, overrides `current` on its own and must keep its color);
/// the `DIM` modifier is dropped so the gray renders solid rather than dimming a
/// second time into the light background.
///
/// The discriminator relies on the picker's color scheme leaving `normal`'s
/// foreground unset (`fg:-1` in the `.color(...)` string in `picker::mod`, which
/// parses to `Reset`). Giving `fg` a concrete index there would make *every*
/// row's `base_style.fg` concrete and so read as selected — keep `fg:-1`.
fn anchor_faint_under_selection(
    mut line: Line<'static>,
    context: &DisplayContext,
) -> Line<'static> {
    let is_selected = !matches!(context.base_style.fg, None | Some(Color::Reset));
    if is_selected {
        for span in &mut line.spans {
            if span.style.add_modifier.contains(Modifier::DIM)
                && matches!(span.style.fg, None | Some(Color::Reset))
            {
                span.style.fg = Some(SELECTED_FAINT_FG);
                span.style.add_modifier.remove(Modifier::DIM);
            }
        }
    }
    line
}

/// Cache key for pre-computed previews: `(row-key, mode)`, where the row-key is
/// the row's [`PickerRow::preview_key`] — a branch for a worktree row, the
/// `pr:N` / `mr:N` token for a listed `--prs` row.
pub(super) type PreviewCacheKey = (String, PreviewMode);

/// Cache for pre-computed previews, keyed by [`PreviewCacheKey`].
/// Shared across all PickerRows for background pre-computation. This is the
/// in-memory tier and the only one `preview()` reads; the disk tiers and the
/// invalidation rules are mapped in [`super::preview_orchestrator`].
pub(super) type PreviewCache = Arc<DashMap<PreviewCacheKey, String>>;

/// Per-row live `pr_status` for the `pr` tab, shared with the collect handler.
/// Primed from the CI cache at skeleton time, then overwritten as the live
/// `CiStatus` task reports (`progressive_handler::PickerHandler::on_update`).
/// `None` = the fetch hasn't reported yet (Loading); `Some(None)` = no PR;
/// `Some(Some(status))` = a PR/MR with status.
pub(super) type PrStatusSlot = Arc<Mutex<Option<Option<PrStatus>>>>;

/// Per-row live content signals for the local-checkout tabs, shared with the
/// collect handler. Like [`PrStatusSlot`], the frozen `item` snapshot can't
/// carry these (its `counts` / `upstream` / `working_tree_status` are `None` at
/// skeleton and never update), so the handler mirrors them here as the list
/// pipeline lands (`on_update`). Read live by [`PickerRow::render_preview`]
/// to dim a tab once its diff is known empty.
pub(super) type LocalContentSlot = Arc<Mutex<LocalContent>>;

/// Prefix on a worktree-backed item's `output()` token. Detached worktrees
/// all share the `(detached)` branch label, so `output()` returns the
/// worktree path (which is unique) behind this prefix instead.
pub(super) const WORKTREE_OUTPUT_PREFIX: &str = "worktree-path:";

/// Per-row data the picker needs at key-press time, looked up by the row's
/// `output()` token: the `alt-y` (copy branch) / `alt-o` (open PR/MR URL)
/// shortcuts, and the `alt-x` in-place branch morph (see [`MorphHandle`]).
///
/// The callbacks read the *selected* `Arc<dyn SkimItem>` straight off skim's
/// `App`, but skim's cross-thread `downcast_ref` is unreliable (see
/// `picker_item_identifier`), so the row's typed fields and shared slots aren't
/// reachable that way. This table carries them instead, keyed by the same
/// `output()` token both sides already share. It's filled where the rows are
/// built — `progressive_handler::on_skeleton` for worktree/branch rows,
/// `prs::stream_open_prs` for `--prs` rows — and read by the keybinding
/// callbacks in `picker::install_shortcut_keybindings` and the `alt-x` removal
/// in `picker::PickerCollector`.
pub(super) struct RowShortcutData {
    /// The row's branch name — what `alt-y` copies. `None` for a detached
    /// worktree (no branch), which makes `alt-y` a no-op rather than copying
    /// the `"(detached)"` label. For a `--prs` row this is the PR/MR's head
    /// branch.
    pub branch: Option<String>,
    /// Where `alt-o` finds the row's PR/MR URL.
    pub url: RowUrl,
    /// Handles for the `alt-x` in-place branch morph, when this row is a
    /// linked worktree whose branch could outlive it. `None` for `--prs` rows
    /// and the primary worktree (nothing to morph into). See [`MorphHandle`].
    pub morph: Option<MorphHandle>,
}

/// Shared handles that let `alt-x` morph a worktree row into a `/ branch` row
/// in place — no reload, no cursor move — when a removal keeps the (unmerged)
/// branch. The collector rewrites the row's display through these slots and
/// flips [`morphed`](Self::morphed); the same `Arc`s back the live
/// [`PickerRow`], so skim repaints the one row on the next `Event::Render`.
///
/// Reached by the collector through the shared [`RowShortcutData`] table (the
/// `downcast_ref` route is unreliable cross-thread), so every field is an `Arc`
/// the handler already built for the row in `on_skeleton`.
pub(super) struct MorphHandle {
    /// Worktree snapshot — the source the collector clones (flipping `kind` to
    /// `Branch`) to render the `/ branch` line on the live layout.
    pub item: Arc<ListItem>,
    /// skim's display source for this row. The morph swaps the worktree line
    /// for the rendered branch line here; the revert (failed removal) swaps it
    /// back.
    pub rendered: Arc<Mutex<String>>,
    /// Dimmed to `working_tree: Some(false)` on morph so the `working_tree`
    /// preview tab — there's no worktree left to diff — reads as empty.
    pub local_content: LocalContentSlot,
    /// Shared with [`PickerRow::output`]: once set, the row's selection
    /// token is the bare branch name (a branch-only row) instead of the
    /// worktree path.
    pub morphed: Arc<AtomicBool>,
}

/// Source of a row's PR/MR URL for the `alt-o` shortcut.
pub(super) enum RowUrl {
    /// Worktree/branch row: the URL (if any) lives in the live `pr_status`
    /// slot, populated only once the CI fetch reports — so `alt-o` is a no-op
    /// until the row's PR resolves.
    Live(PrStatusSlot),
    /// `--prs` row: the URL is known when the row is built.
    Static(Option<String>),
}

impl RowUrl {
    /// The row's URL, if one is known right now.
    pub(super) fn resolve(&self) -> Option<String> {
        match self {
            RowUrl::Live(slot) => match &*slot.lock().unwrap() {
                Some(Some(status)) => status.url.clone(),
                _ => None,
            },
            RowUrl::Static(url) => url.clone(),
        }
    }
}

/// The `alt-y` / `alt-o` lookup table, keyed by `output()` token. Shared between
/// the collect handler and `--prs` thread (which fill it) and the keybinding
/// callbacks (which read it). Rebuilt on every skeleton, so a refresh re-collect
/// repopulates it.
pub(super) type ShortcutTable = Arc<Mutex<std::collections::HashMap<String, RowShortcutData>>>;

/// The collect [`LayoutConfig`](crate::commands::list::layout::LayoutConfig),
/// handed from the collect thread to the picker so `alt-x` can render a
/// `/ branch` row on the same grid as the worktree rows (the in-place morph).
/// `LayoutConfig` is `!Sync`, so it rides behind a lock; `None` until the first
/// skeleton lands, overwritten on each refresh. Shared (like [`ShortcutTable`])
/// between the per-spawn handler that fills it and the collector that reads it.
pub(super) type LayoutSlot = Arc<Mutex<Option<crate::commands::list::layout::LayoutConfig>>>;

/// The `output()` token for a worktree/branch row: the unique worktree path
/// (behind [`WORKTREE_OUTPUT_PREFIX`]) for any worktree-backed row, else the
/// bare branch name. Shared by [`PickerRow::output`] and the shortcut
/// table fill in `progressive_handler::on_skeleton`, so the selection token and
/// the lookup key can't drift.
pub(super) fn worktree_output_token(item: &ListItem, branch_name: &str) -> String {
    match item.worktree_path() {
        Some(path) => format!("{WORKTREE_OUTPUT_PREFIX}{}", path.to_string_lossy()),
        None => branch_name.to_string(),
    }
}

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
/// Append the PR/MR fields a row filters on — reference (`#123`/`!7`), title,
/// author — to `text`, space-separated, in the order shared by worktree rows
/// and listed `--prs` rows. A PR thus filters identically however it's shown,
/// the rule the picker holds to. Empty title/author are skipped; the caller
/// owns the trailing gutter glyph (which must stay last). `text` is assumed
/// non-empty (it starts with the branch), so every token is space-prefixed.
pub(super) fn push_pr_search_tokens(
    text: &mut String,
    reference: PrRef,
    title: Option<&str>,
    author: Option<&str>,
) {
    for token in [
        Some(reference.to_string()),
        title
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_owned),
        author
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .map(str::to_owned),
    ]
    .into_iter()
    .flatten()
    {
        text.push(' ');
        text.push_str(&token);
    }
}

/// The matcher text (`text()`) is assembled live from `search_base` + the
/// `gutter` glyph + the current `pr_status` slot, so a PR's reference, title,
/// and author become filterable the moment the CI fetch lands them — the same
/// fields a listed `--prs` row filters on (see [`push_pr_search_tokens`]). skim
/// re-reads `text()` on each query keystroke, so the live PR data folds in
/// without a re-send; the row may re-rank as that data streams in during the
/// first frame.
pub(super) struct PickerRow {
    /// The stable, skeleton-time head of the matcher text: branch name plus —
    /// for a worktree row — the distinct worktree path. The PR tokens and
    /// trailing gutter glyph are appended live in `text()`.
    pub search_base: String,
    /// Trailing gutter glyph (`@`/`^`/`+`/`/`/`|`, or `#` for a listed `--prs`
    /// row), kept last in `text()` so a typed sigil filters by row kind. A
    /// skeleton-time fact from `ItemKind::gutter_glyph`.
    pub gutter: char,
    /// Current ANSI-colored display line. For a worktree row it starts as the
    /// skeleton render and is replaced in place as data arrives; for a `--prs`
    /// row it's built once and never mutated.
    pub rendered: Arc<Mutex<String>>,
    /// Branch name (the head branch for a `--prs` row). Used by switch
    /// selection, the `pr` pane's branch line, and — for a worktree row — the
    /// preview cache key (see [`Self::preview_key`]).
    pub branch_name: String,
    /// Selection result returned by `output()`, and the `shortcut_table` key.
    /// `worktree_output_token` (`worktree-path:<path>` or `<branch>`) for a
    /// worktree row; `pr:N` / `mr:N` for a `--prs` row. Distinct from the
    /// *preview* key (see [`Self::preview_key`]): for a worktree row the two
    /// differ (path-token vs branch).
    pub output_token: String,
    /// Shared cache for pre-computed previews (all modes)
    pub preview_cache: PreviewCache,
    /// PR/MR status for the `pr`/`comments` tabs and the folded matcher tokens.
    /// LIVE for a worktree row — primed from the CI cache and overwritten as the
    /// `CiStatus` task streams in (so the `pr` tab reflects the live fetch);
    /// PRE-FILLED and STATIC for a `--prs` row (from `PrEntry::display_status`,
    /// never mutated within a row's life; a rebuild re-renders the `(pr:N, Pr)`
    /// memo — see `prs::listed_pr_row`).
    pub pr_status: PrStatusSlot,
    /// Surfaces a background preview fill without a keystroke. `preview()`
    /// records the row's awaited `(preview_key, mode)` here on every render; the
    /// orchestrator pokes a repaint when that key's compute lands (see
    /// [`PreviewNotifier`]).
    pub notifier: Arc<PreviewNotifier>,
    /// Local-checkout data — `Some` for a worktree row, `None` for a listed
    /// `--prs` row (whose head branch isn't checked out, so it has no working
    /// tree, diffs, or local `ListItem`). Its presence is the single axis the
    /// preview/output paths branch on.
    pub local: Option<LocalCheckout>,
}

/// The worktree-backed half of a [`PickerRow`]: present only when the row's
/// branch is checked out locally. A listed `--prs` row carries `None` and
/// renders its local-checkout tabs (working-tree, branch-diff, upstream,
/// summary) as placeholders.
pub(super) struct LocalCheckout {
    /// Whether this branch has an upstream tracking ref, for the tab-4
    /// (remote⇅) empty state. A SYNCHRONOUS skeleton-time fact read from
    /// `Repository::local_branches()` at construction — never from the async
    /// `item.upstream`, which is `None` until the row pipeline lands and would
    /// lock the tab bar into a stale state (see `TabAvailability`).
    pub has_upstream: bool,
    /// Whether `[commit.generation]` summaries are configured, for the tab-5
    /// (summary) empty state. A process-wide static fact (`llm_command.is_some()`).
    pub summaries_enabled: bool,
    /// Live diff-content signals for the `working_tree` / `branch_diff` /
    /// `upstream` tabs, shared with the collect handler. The frozen `item`
    /// snapshot can't carry these (its `counts` / `upstream` /
    /// `working_tree_status` are `None` at skeleton and never update), so the
    /// handler mirrors them here as the pipeline lands — letting those tabs dim
    /// once their diff is known empty (see [`LocalContent`]).
    pub local_content: LocalContentSlot,
    /// Set when an `alt-x` removal kept this row's branch and morphed the row to
    /// `/ branch` in place: [`PickerRow::output`] then yields the bare branch
    /// name (a branch-only row) instead of the worktree path. Shared with the
    /// row's [`MorphHandle`], which the collector flips when the worktree is
    /// removed but its branch kept.
    pub morphed: Arc<AtomicBool>,
}

impl PickerRow {
    /// Key for every preview-cache read, the `pr`-pane memo, and the
    /// `PreviewNotifier` `awaiting` record: the branch name for a worktree row
    /// (its local previews are computed and cached by branch), the `pr:N` /
    /// `mr:N` token for a `--prs` row (its deferred `log`/`comments` fetches key
    /// by that token — see the `prs` module). Git forbids `:` in branch names,
    /// so the two keyspaces never collide.
    fn preview_key(&self) -> &str {
        if self.local.is_some() {
            &self.branch_name
        } else {
            &self.output_token
        }
    }
}

impl SkimItem for PickerRow {
    fn text(&self) -> Cow<'_, str> {
        // branch + path (stable), then the live PR/MR tokens (reference, title,
        // author) from the current `pr_status` slot, then the gutter glyph last.
        // Read live so a PR filters by number/title/author as soon as the CI
        // fetch lands them — the same fields a `--prs` row carries.
        let mut text = self.search_base.clone();
        if let Some(Some(status)) = &*self.pr_status.lock().unwrap()
            && let Some(reference) = status.number
        {
            push_pr_search_tokens(
                &mut text,
                reference,
                status.title.as_deref(),
                status.author.as_deref(),
            );
        }
        text.push(' ');
        text.push(self.gutter);
        Cow::Owned(text)
    }

    fn display(&self, context: DisplayContext) -> Line<'_> {
        // Clone-under-lock so the parser's input outlives the guard;
        // `ansi_to_line` returns an owned `Line<'static>`.
        let snapshot = self.rendered.lock().unwrap().clone();
        anchor_faint_under_selection(ansi_to_line(&snapshot), &context)
    }

    fn output(&self) -> Cow<'_, str> {
        // An `alt-x` morph turned this worktree row into a branch-only row: its
        // selection token is the bare branch name, like any branch row (so a
        // later switch/`alt-x` treats it as a branch, and `alt-y` finds it under
        // the re-keyed token). `--prs` rows have no `local`, so they never morph.
        if let Some(local) = &self.local
            && local.morphed.load(Ordering::Relaxed)
        {
            return Cow::Owned(self.branch_name.clone());
        }
        // Precomputed at construction (a worktree-path/branch token, or `pr:N`);
        // see `output_token`. Distinct from `preview_key`.
        Cow::Borrowed(&self.output_token)
    }

    fn preview(&self, context: PreviewContext<'_>) -> ItemPreview {
        // The mode is the only render input that comes from outside the item
        // (it's per-process picker state); everything else is derived in
        // `render_preview`, which takes the mode explicitly so it's testable
        // without touching that global state.
        let mode = PreviewStateData::read_mode();
        // Record what this (selected) row is showing *before* reading the cache,
        // so a background fill that lands right after a miss still finds the key
        // set and pokes a repaint (see `PreviewNotifier`). Keyed by `preview_key`
        // (branch for a worktree row, `pr:N` for a `--prs` row) so the awaited
        // key matches the one the background fill writes.
        self.notifier.note_awaiting(self.preview_key(), mode);
        ItemPreview::AnsiText(self.render_preview(mode, context.width, context.height))
    }
}

/// The `pr` tab's state for a worktree row, derived from the row's live CI
/// status slot ([`PickerRow::pr_status`]).
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
    /// `NoPr`, not `HasPr`) — `render_pr_pane_body` needs no fallback.
    HasPr(PrRef, PrStatus),
}

/// The three states the `pr` and `comments` tabs branch on, read from a live
/// `pr_status` slot value without cloning the status. The discriminant mirrors
/// [`PickerRow::pr_preview`] exactly. The `comments` pane is byte-identical
/// within `HasPr` (its thread is branch-keyed and surfaced by `notify_filled`),
/// so the collect handler re-renders that tab on a *presence* change, not on
/// every `pr_status` field — re-running it for an unchanged body would only
/// reset the user's scroll. See [`PreviewNotifier`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PrPresence {
    Loading,
    NoPr,
    HasPr,
}

/// The [`PrPresence`] a live `pr_status` slot value resolves to — the same
/// three-way split [`PickerRow::pr_preview`] renders.
pub(super) fn pr_presence(pr_status: &Option<Option<PrStatus>>) -> PrPresence {
    match pr_status {
        None => PrPresence::Loading,
        Some(None) => PrPresence::NoPr,
        Some(Some(status)) if status.number.is_some() => PrPresence::HasPr,
        Some(Some(_)) => PrPresence::NoPr,
    }
}

/// Whether two live `pr_status` slot values render the *same* `pr` pane. Equal
/// under the `PrStatus` derive means identical; the pane ignores two fields —
/// `is_priming` (a list-cell dim hint [`render_pr_pane_body`] never reads) and
/// `updated_at` (the comments-cache key, also never shown in the pane) — so a
/// cache-prime→live flip that only changes those is not a pane change and must
/// not re-run the preview (which would reset its scroll). Every other field
/// (CI badge, title, body, review, comment count, …) shows in the pane, so any
/// of them differing is a real change. The lone change-path clone keeps the
/// no-change case (the common repeated `on_update`) allocation-free.
pub(super) fn pr_status_pane_eq(
    a: &Option<Option<PrStatus>>,
    b: &Option<Option<PrStatus>>,
) -> bool {
    match (a, b) {
        (Some(Some(x)), Some(Some(y))) => {
            x == y
                || PrStatus {
                    is_priming: y.is_priming,
                    updated_at: y.updated_at.clone(),
                    ..x.clone()
                } == *y
        }
        _ => a == b,
    }
}

/// Live, content-aware availability for the local-checkout tabs whose pane is a
/// diff — `working_tree`, `branch_diff`, and `upstream`. Each tab dims once its
/// diff is *known* empty, the same way the `pr` tab dims once the fetch reports
/// no PR (see [`PickerRow::pr_tab_available`]).
///
/// The diff emptiness is async (the `counts` / `upstream` / `working_tree_status`
/// fields are `None` at skeleton and land via the list pipeline), so each signal
/// is `Option<bool>`: `None` while loading, `Some(has_content)` once known. A tab
/// stays available while its signal is `None` — we don't dim before we know it's
/// empty — and dims when the signal resolves to "no content". Each predicate
/// mirrors exactly what its pane renders, so a dimmed number never contradicts a
/// non-empty pane.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct LocalContent {
    /// `working_tree`: tracked uncommitted changes exist. Matches the pane's
    /// `git diff HEAD` — staged/modified/renamed/deleted, NOT untracked (which
    /// `git diff HEAD` doesn't show). `Some(false)` for a branch-only row (no
    /// working tree to diff).
    working_tree: Option<bool>,
    /// `branch_diff`: the branch has commits ahead of the mainline tip
    /// (`counts.ahead > 0`), or it is an orphan (no merge base — see
    /// [`Self::from_item`]). `counts` is `AheadBehindTask`'s answer, measured
    /// against the **upstream-aware** comparison base
    /// (`TaskContext::comparison_base` — the upstream ref when the local default
    /// lags it), so a stale fork default doesn't inflate it. The branch-diff and
    /// summary panes diff against the **same** base
    /// (`Repository::branch_diff_spec`), so the dimmed number and the pane agree
    /// even on a fork whose local default lags upstream.
    branch_diff: Option<bool>,
    /// `upstream`: the branch is ahead of or behind its tracking ref. Combined
    /// with the synchronous `has_upstream` floor (no tracking ref dims the tab
    /// immediately, with no loading window); this only narrows a *present*
    /// upstream from "available" to "dim" once it's known to be up to date.
    upstream_diverged: Option<bool>,
}

impl LocalContent {
    /// Read the content signals off a row's `ListItem`. Called from the collect
    /// handler each time a row updates, then stored in the row's live slot.
    pub(super) fn from_item(item: &ListItem) -> Self {
        let working_tree = match item.worktree_data() {
            // A real worktree: tracked changes per the porcelain status (matches
            // the pane's `git diff HEAD`, which excludes untracked files).
            Some(data) => data
                .working_tree_status
                .map(|s| s.staged || s.modified || s.renamed || s.deleted),
            // Branch-only row: no working tree, so nothing to diff.
            None => Some(false),
        };
        Self {
            working_tree,
            // Commits ahead of the upstream-aware comparison base (see the
            // `branch_diff` field doc). An orphan has no merge base with the
            // default, so its three-dot diff is ill-defined — keep the tab
            // available rather than dim a pane that may show content. `counts`
            // and `is_orphan` land together (one task), so when `counts` is
            // known `is_orphan` is too.
            branch_diff: item
                .counts
                .map(|c| c.ahead > 0 || item.is_orphan == Some(true)),
            upstream_diverged: item
                .upstream
                .as_ref()
                .map(|u| u.active().is_some_and(|a| a.ahead > 0 || a.behind > 0)),
        }
    }
}

/// The local-checkout-backed preview tabs — present for a row with a local
/// worktree, absent for a listed PR/MR row (`--prs`), which has no local copy.
/// Grouping them names *why* they're empty on a `--prs` row (no checkout), so
/// the difference reads as a data fact rather than a row-type policy.
#[derive(Debug, Clone, Copy, Default)]
struct LocalTabs {
    working_tree: bool,
    branch_diff: bool,
    upstream: bool,
    summary: bool,
}

impl LocalTabs {
    /// A worktree-backed row. The diff tabs (`working_tree`, `branch_diff`,
    /// `upstream`) follow the live [`LocalContent`] signals — available while
    /// loading, dim once their diff is known empty; `upstream` also requires the
    /// synchronous `has_upstream` floor (no tracking ref → dim with no loading
    /// window). `summary` requires the process-wide `[commit.generation]` flag
    /// *and* something to summarize: its pane is generated from the combined diff
    /// (`crate::summary::compute_combined_diff` = commits ahead of the comparison
    /// base + working-tree changes), so it reuses the `branch_diff` and
    /// `working_tree` signals that gate tabs 3 and 1 — dimming in concert with
    /// them once both are known empty, not only when summaries are disabled.
    fn worktree(content: LocalContent, has_upstream: bool, summaries_enabled: bool) -> Self {
        let working_tree = content.working_tree.unwrap_or(true);
        let branch_diff = content.branch_diff.unwrap_or(true);
        Self {
            working_tree,
            branch_diff,
            upstream: has_upstream && content.upstream_diverged.unwrap_or(true),
            summary: summaries_enabled && (working_tree || branch_diff),
        }
    }
}

/// Which preview tabs have renderable content for the selected row.
///
/// Empty tabs are de-emphasized in the bar (number dimmed). Skim computes a
/// preview once per selection and cannot re-query it mid-selection (see
/// `loading_placeholder`). Two genuine axes drive availability, and `--prs`
/// touches neither — it only decides whether a PR row is *listed* at all:
///
/// - The local-checkout tabs ([`LocalTabs`]): the three diff tabs
///   (`working_tree` / `branch_diff` / `upstream`) follow the row's live
///   [`LocalContent`] — available while their diff is still loading, dim once
///   it's known empty (a clean working tree, no commits ahead, up to date with
///   upstream), the same loading-then-dim shape as the `pr` tab. `upstream` also
///   carries a synchronous `has_upstream` floor (`Repository::local_branches()`),
///   so a branch with no tracking ref dims immediately with no loading window.
///   `summary` requires the process-wide `[commit.generation]` flag *and* a
///   non-empty combined diff — its pane's content source — so it reuses the
///   `branch_diff` / `working_tree` signals and dims alongside the "no changes
///   to summarize" pane, not only when summaries are disabled. A `--prs` row has
///   no local checkout, so all four are empty.
/// - The PR-backed tabs: `pr` and `comments` are available together, gated by
///   `has_pr`. On a worktree row that's the live status slot (primed from the CI
///   cache, then refreshed by the `CiStatus` task — see
///   [`PickerRow::pr_tab_available`]): it dims once the fetch reports no
///   PR, and stays available while loading or with a PR. A `--prs` row always
///   has a PR, so both are available.
///
/// `comments` tracks `has_pr` on *every* row, so the comments tab behaves the
/// same whether the row is a worktree or a listed PR — only its content differs
/// where the data legitimately does (no local checkout). `log` is always
/// present (local `git log` on a worktree row, a background forge fetch on a
/// `--prs` row).
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
    /// Build from the two genuine axes: which local-checkout tabs the row has,
    /// and whether it has a PR. Centralizing the mapping pins the invariants the
    /// task depends on — `log` is always present, and the PR-backed tabs (`pr`,
    /// `comments`) both follow `has_pr` — so they can't drift between a worktree
    /// row and a listed-PR row.
    fn from_axes(local: LocalTabs, has_pr: bool) -> Self {
        Self {
            working_tree: local.working_tree,
            log: true,
            branch_diff: local.branch_diff,
            upstream: local.upstream,
            summary: local.summary,
            pr: has_pr,
            comments: has_pr,
        }
    }

    /// A worktree-backed row: the local-checkout tabs follow [`LocalTabs`]
    /// (diff tabs gated by the live [`LocalContent`] + the `has_upstream` floor);
    /// the PR-backed tabs follow the live PR status (see
    /// [`PickerRow::pr_tab_available`]).
    pub(super) fn worktree(
        content: LocalContent,
        has_upstream: bool,
        summaries_enabled: bool,
        has_pr: bool,
    ) -> Self {
        Self::from_axes(
            LocalTabs::worktree(content, has_upstream, summaries_enabled),
            has_pr,
        )
    }

    /// A listed PR/MR row (`--prs`): no local checkout, so the local-checkout
    /// tabs are empty; it always has a PR, so the `pr` and `comments` tabs are
    /// available and the `log` tab loads in the background (see
    /// `prs::compute_pr_log`). The differences from a worktree row are only the
    /// genuine data ones — no local checkout, and a PR that's always present.
    pub(super) fn listed_pr() -> Self {
        Self::from_axes(LocalTabs::default(), true)
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

    // Controls use dim cyan to distinguish from the dimmed (white) tabs above.
    // The tab numbers above are the alt-N accelerators (bare digits type
    // into the query); Tab/shift-tab cycle the same tabs.
    //
    // Order: primary action (Enter), preview navigation (ctrl-u/d scroll, then
    // the Tab/alt-1…7 accelerators), then row actions, with Esc last.
    //
    // The controls line is intentionally NOT width-managed: skim clips it on the
    // right on a narrow pane, but it's only a reminder. Note the trade-off of
    // this order: the row actions (alt-c/x/y/o/r/p) now trail and clip first, and
    // unlike the preview accelerators they are not duplicated in the (width-
    // managed) tab bar above — so on a narrow pane the only on-screen reminder of
    // them can clip away.
    let controls = cformat!(
        "<dim,cyan>Enter: switch | ctrl-u/d: scroll | Tab/alt-1…7: preview | alt-c: create | alt-x: remove | alt-y: copy | alt-o: open | alt-r: refresh | alt-p: toggle | Esc: cancel</>"
    );

    // Each tab/segment already ends with a full reset (so styling never bleeds
    // into the dividers or preview content); `{reset}` here only closes the
    // controls line's dim-cyan span.
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

/// The sole `pr`-tab pane renderer, shared by worktree rows and listed `--prs`
/// rows (both feed it a [`PrStatus`] — live from the CI fetch for a worktree
/// row, pre-filled via `PrEntry::display_status` for a `--prs` row). Renders
/// the reference + title header, cyan all-caps labeled metadata (the branch
/// bold, the author, draft state, the url underlined), and the description as
/// markdown via the shared [`pr_pane`] helpers. Any field a status doesn't
/// carry (an older cache entry, a forge that doesn't expose it) is skipped, the
/// header falling back to reference-only.
///
/// The `comments` line shows only the count; the full thread is the `comments`
/// tab's own background fetch (see [`PickerRow::render_comments_pane`]).
/// A PR with no comments carries `None` (zero is flattened at the mapping
/// boundary), so the line is skipped.
///
/// `width` is the live preview-pane width, used to wrap the markdown body.
fn render_pr_pane_body(branch: &str, pr_ref: PrRef, status: &PrStatus, width: usize) -> String {
    let title = status.title.as_deref().filter(|t| !t.is_empty());
    let mut out = pr_pane::header(pr_ref, title);
    out.push_str(&pr_pane::branch_line(branch));
    if let Some(author) = status.author.as_deref().filter(|a| !a.is_empty()) {
        out.push_str(&pr_pane::metadata_line("author", &format!("@{author}")));
    }
    if status.review_state == Some(ReviewState::Draft) {
        let reset = Reset;
        out.push_str(&pr_pane::metadata_line(
            "state",
            &cformat!("<yellow>draft</>{reset}"),
        ));
    }
    if let Some(url) = &status.url {
        out.push_str(&pr_pane::url_line(url));
    }
    if let Some(count) = status.comment_count {
        out.push_str(&pr_pane::metadata_line("comments", &count.to_string()));
    }
    if let Some(body) = status.body.as_deref() {
        out.push_str(&pr_pane::description(body, width));
    }
    out
}

impl PickerRow {
    /// Render the full preview pane (tab bar + mode content) for an explicit
    /// `mode`. Split out of [`SkimItem::preview`] so the dispatch — including
    /// the `pr` tab's `render_pr_pane` call — is testable with a given mode
    /// rather than the process-wide picker mode.
    fn render_preview(&self, mode: PreviewMode, width: usize, height: usize) -> String {
        // Build preview: tabs header + content. `has_upstream` and
        // `summaries_enabled` are synchronous skeleton-time facts (see
        // `TabAvailability`); the diff tabs' live emptiness (`local_content`) and
        // `pr` (`pr_tab_available`) read the row's live slots, refreshed as the
        // list pipeline / `CiStatus` task stream in, so they can change between
        // selections — a tab dims once its diff (or PR) is known empty, and stays
        // available while still loading. The `summary` tab rides the same
        // `local_content` signal: its content source is the combined diff, so it
        // dims once both `branch_diff` and `working_tree` are known empty. Both
        // reads are cheap (no body clone); the panes themselves are rendered once
        // and memoized.
        let avail = match &self.local {
            Some(local) => TabAvailability::worktree(
                self.local_content(),
                local.has_upstream,
                local.summaries_enabled,
                self.pr_tab_available(),
            ),
            // A listed `--prs` row: the local-checkout tabs are empty (no
            // working tree to diff) and the `pr` tab always has content (the
            // static slot carries a `number`), matching `pr_tab_available`.
            None => TabAvailability::listed_pr(),
        };
        let mut result = render_preview_tabs(mode, avail, width);
        result.push_str(&match mode {
            // The PR-backed tabs read the same `pr_status` slot for both row
            // kinds: `pr` renders from it, `comments` reads the background-fetched
            // thread when the row has a PR (always, for a `--prs` row).
            PreviewMode::Pr => self.render_pr_pane_cached(width),
            PreviewMode::Comments => self.render_comments_pane(),
            // The local-checkout tabs (working-tree/log/branch-diff/upstream/
            // summary) compute locally for a worktree row; a `--prs` row has no
            // checkout, so its `log` loads from the forge and the rest point at
            // the `pr` tab (see `render_listed_pr_mode`).
            _ => match &self.local {
                Some(_) => self.preview_for_mode(mode, width, height),
                None => self.render_listed_pr_mode(mode),
            },
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

    /// Whether the PR-backed tabs (`pr` and `comments`) have content for this
    /// row — they dim together only once the live fetch reports no PR. A cheap
    /// discriminant read of the `pr_status` slot (no `PrStatus` clone, unlike
    /// [`Self::pr_preview`]), so the tab bar can be drawn on every `preview()`
    /// call without re-cloning the possibly-large body that
    /// [`Self::render_pr_pane_cached`] already memoizes.
    fn pr_tab_available(&self) -> bool {
        match &*self.pr_status.lock().unwrap() {
            None => true,                                  // still fetching
            Some(None) => false,                           // fetch reported no PR
            Some(Some(status)) => status.number.is_some(), // a bare branch workflow is "no PR"
        }
    }

    /// Snapshot the row's live diff-content signals for the `working_tree` /
    /// `branch_diff` / `upstream` tabs (see [`LocalContent`]). A cheap copy of
    /// three `Option<bool>`s; the collect handler overwrites the slot as the list
    /// pipeline lands. A listed `--prs` row has no checkout, so it reports the
    /// default (all-`None`) signals — its diff tabs render placeholders anyway.
    fn local_content(&self) -> LocalContent {
        self.local
            .as_ref()
            .map(|l| *l.local_content.lock().unwrap())
            .unwrap_or_default()
    }

    /// Render the `pr` pane, memoized in the shared `preview_cache` so repeated
    /// `preview()` calls (every keystroke while the tab is active) don't re-run
    /// the markdown render of the body. The other modes are pre-computed into
    /// this same cache by the orchestrator; the `pr` pane is filled lazily here
    /// instead because its inputs aren't known at skeleton time — they stream in
    /// on the `CiStatus` task. The entry is invalidated so a cache hit always
    /// matches the current PR data: for a worktree row, `on_update` removes it
    /// whenever the live `pr_status` slot changes (fetch lands, manual refresh);
    /// for a `--prs` row, whose slot is static, `prs::listed_pr_row` removes it
    /// when the row is (re)built on an `alt-r` reload.
    pub(super) fn render_pr_pane_cached(&self, width: usize) -> String {
        let key = (self.preview_key().to_string(), PreviewMode::Pr);
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
        let branch = self.branch_name.as_str();
        match pr {
            PrPreview::Loading => {
                cformat!("{HINT_SYMBOL} <dim>Fetching PR status for {branch}…</>\n")
            }
            PrPreview::NoPr => {
                cformat!("{INFO_SYMBOL}{reset} <bold>{branch}</>{reset} has no PR\n")
            }
            PrPreview::HasPr(pr_ref, status) => render_pr_pane_body(branch, pr_ref, &status, width),
        }
    }

    /// The `comments` tab's pane on a worktree row — identical in behavior to a
    /// `--prs` row's comments tab. When the branch has an open PR, the thread is
    /// fetched in the background (spawned from `progressive_handler`'s
    /// `on_update`/`on_skeleton` once the CI fetch surfaces the PR) and read here
    /// from the shared cache, keyed by branch name; a miss is the in-flight
    /// window and shows the same loading placeholder a `--prs` row's tab does. No
    /// PR — or a CI fetch that hasn't resolved yet — mirrors the `pr` tab's
    /// empty/loading states, so the two PR-backed tabs stay consistent. The
    /// background fetch renders at the preview width, so this reads it back
    /// without re-wrapping (via `cached_or_loading`).
    fn render_comments_pane(&self) -> String {
        let reset = Reset;
        let branch = self.branch_name.as_str();
        match self.pr_preview() {
            // The CI fetch hasn't reported yet — we don't know whether there's a
            // PR to fetch comments for. Mirror the `pr` tab's loading state.
            // (A `--prs` row's static slot is always `HasPr`, so it never lands
            // here.)
            PrPreview::Loading => {
                cformat!("{HINT_SYMBOL} <dim>Fetching PR status for {branch}…</>\n")
            }
            PrPreview::NoPr => {
                cformat!("{INFO_SYMBOL}{reset} <bold>{branch}</>{reset} has no PR\n")
            }
            PrPreview::HasPr(..) => self.cached_or_loading(PreviewMode::Comments),
        }
    }

    /// Read a deferred forge-fetch tab's pane from the shared cache (keyed by
    /// the row's [`Self::preview_key`]), or a loading placeholder on a miss. The
    /// background fetch (worktree rows' `comments`, `--prs` rows' `log`/
    /// `comments`) writes the same key, and the orchestrator pokes a repaint
    /// once it lands.
    pub(super) fn cached_or_loading(&self, mode: PreviewMode) -> String {
        self.preview_cache
            .get(&(self.preview_key().to_string(), mode))
            .map(|v| v.value().clone())
            .unwrap_or_else(|| super::prs::pr_deferred_loading(mode))
    }

    /// Non-PR-tab content for a listed `--prs` row (no local checkout): the
    /// `log` tab loads commits in the background and reads from the cache; the
    /// local-only tabs (working-tree, branch-diff, upstream, summary) have no
    /// PR equivalent, so they point the user at the `pr` tab.
    fn render_listed_pr_mode(&self, mode: PreviewMode) -> String {
        match mode {
            PreviewMode::Log => self.cached_or_loading(PreviewMode::Log),
            _ => super::prs::pr_row_empty_placeholder(),
        }
    }

    /// Render preview for the given mode with specified dimensions.
    ///
    /// Pure cache read: skim invokes `preview()` synchronously while drawing
    /// the preview pane, so any blocking here gates the render. Background
    /// tasks populate the cache out-of-band; a miss returns a placeholder, and
    /// the orchestrator pokes a repaint for the awaited key once the fill lands
    /// (see [`PreviewNotifier`]).
    fn preview_for_mode(&self, mode: PreviewMode, width: usize, _height: usize) -> String {
        // Reached only for a worktree row (the `Some(_)` arm in `render_preview`),
        // so `preview_key` is the branch — the key the orchestrator precomputes
        // local previews under.
        let cache_key = (self.preview_key().to_string(), mode);
        let content = self
            .preview_cache
            .get(&cache_key)
            .map(|v| v.value().clone())
            .unwrap_or_else(|| Self::loading_placeholder(mode));

        match mode {
            // Summary post-processing is cheap (string formatting, no subprocess).
            // Applied at display time because `generate_summary_for_item` produces
            // raw LLM output.
            PreviewMode::Summary => super::summary::render_summary(&content, width),
            _ => content,
        }
    }

    /// Placeholder shown while a background task is still computing the preview
    /// for this mode. The pane fills in on its own once the compute lands — the
    /// orchestrator pokes a repaint for the awaited key (see [`PreviewNotifier`])
    /// — so the placeholder just states what's loading.
    pub(super) fn loading_placeholder(mode: PreviewMode) -> String {
        let (verb, label) = match mode {
            PreviewMode::WorkingTree => ("Loading", "working-tree diff"),
            PreviewMode::Log => ("Loading", "log"),
            PreviewMode::BranchDiff => ("Loading", "branch diff"),
            PreviewMode::UpstreamDiff => ("Loading", "upstream diff"),
            PreviewMode::Summary => ("Generating", "summary"),
            // `preview()` routes the PR-backed tabs around this path: `pr` renders
            // from the cached `pr_status` via `render_pr_pane`, and `comments`
            // through `render_comments_pane` (which uses the `--prs` rows' shared
            // `pr_deferred_loading` for its own in-flight placeholder), so neither
            // reaches here.
            PreviewMode::Pr => unreachable!("pr tab renders via render_pr_pane"),
            PreviewMode::Comments => {
                unreachable!("comments tab renders via render_comments_pane")
            }
        };
        cformat!("{HINT_SYMBOL} <dim>{verb} {label}…</>\n")
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

    /// Compute Tab 3: Branch diff preview (line diffs vs the comparison base)
    ///
    /// Independent of `item.counts` — `compute_diff_preview`'s empty-diff
    /// fallback covers the ahead=0 case, so the preview is correct even
    /// before the list-row pipeline has populated counts.
    ///
    /// The base comes from [`Repository::branch_diff_spec`], which measures
    /// against the **upstream-aware comparison base** — the same ref the
    /// ahead/behind and branch-diff columns use — so a stale fork default
    /// can't show upstream commits as this branch's changes. Its resolved SHA
    /// keys the disk cache (stable across `git fetch`, which moves the *ref*
    /// but not the captured SHA), and orphan branches diff their full content
    /// against the empty tree.
    fn compute_branch_diff_preview(repo: &Repository, item: &ListItem, width: usize) -> String {
        let branch = item.branch_name();
        let reset = Reset;
        let Some(spec) = repo.branch_diff_spec(item.head()) else {
            return cformat!(
                "{INFO_SYMBOL}{reset} <bold>{branch}</>{reset} has no commits ahead of main\n"
            );
        };

        if let Some(cached) =
            preview_cache::read_branch_diff(repo, &spec.cache_sha, item.head(), width)
        {
            return cached;
        }

        let mut diff_args = vec!["diff"];
        diff_args.extend(spec.revs.iter().map(String::as_str));
        let base_name = &spec.base_name;
        let result = compute_diff_preview(
            repo,
            &diff_args,
            &cformat!(
                "{INFO_SYMBOL}{reset} <bold>{branch}</>{reset} has no file changes vs <bold>{base_name}</>{reset}"
            ),
            width,
        );

        preview_cache::write_branch_diff(repo, &spec.cache_sha, item.head(), width, &result);
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

    /// `anchor_faint_under_selection` recolors faint text to an explicit gray
    /// only on the selected row (and only foreground-less faint spans), so the
    /// "safe to delete" gray survives skim's selection highlight while
    /// off-cursor rows keep their `DIM` styling.
    #[test]
    fn anchor_faint_under_selection_targets_only_the_selected_row() {
        use ratatui::style::Style;

        // A removable row's branch (faint, no fg), a faint span whose fg parsed
        // to an explicit `Reset` (e.g. `\x1b[2m\x1b[39m`), a colored diff count
        // (faint but explicitly green), and a plain span.
        let make = || {
            Line::from(vec![
                Span::styled(
                    "safe-to-delete",
                    Style::default().add_modifier(Modifier::DIM),
                ),
                Span::styled(
                    "path",
                    Style::default()
                        .fg(Color::Reset)
                        .add_modifier(Modifier::DIM),
                ),
                Span::styled(
                    "+1",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::DIM),
                ),
                Span::raw("main"),
            ])
        };

        // Off-cursor rows (base_style fg unset / Reset) are left untouched.
        for fg in [None, Some(Color::Reset)] {
            let ctx = DisplayContext {
                base_style: Style {
                    fg,
                    ..Style::default()
                },
                ..Default::default()
            };
            let out = anchor_faint_under_selection(make(), &ctx);
            assert!(out.spans[0].style.add_modifier.contains(Modifier::DIM));
            assert_eq!(out.spans[0].style.fg, None);
            assert!(out.spans[1].style.add_modifier.contains(Modifier::DIM));
        }

        // The selected row (base_style carries a concrete `current` fg) re-anchors
        // both the fg-unset and the `Reset`-fg faint spans to gray and drops their
        // DIM; the green span keeps its color, and the plain span is unchanged.
        let selected = DisplayContext {
            base_style: Style::default().fg(Color::Indexed(237)),
            ..Default::default()
        };
        let out = anchor_faint_under_selection(make(), &selected);
        for i in [0, 1] {
            assert_eq!(out.spans[i].style.fg, Some(SELECTED_FAINT_FG));
            assert!(!out.spans[i].style.add_modifier.contains(Modifier::DIM));
        }
        assert_eq!(out.spans[2].style.fg, Some(Color::Green));
        assert!(out.spans[2].style.add_modifier.contains(Modifier::DIM));
        assert_eq!(out.spans[3].style.fg, None);
    }

    /// Build a worktree-backed [`PickerRow`] (`local: Some`) for tests, with the
    /// given branch, preview cache, and live `pr_status` slot value; the local
    /// signals default (no upstream, no summaries, unknown diff content).
    fn worktree_test_row(
        branch: &str,
        preview_cache: PreviewCache,
        pr_status: Option<Option<PrStatus>>,
    ) -> PickerRow {
        PickerRow {
            search_base: String::new(),
            gutter: '@',
            rendered: Arc::new(Mutex::new(String::new())),
            branch_name: branch.to_string(),
            output_token: branch.to_string(),
            preview_cache,
            pr_status: Arc::new(Mutex::new(pr_status)),
            notifier: PreviewNotifier::detached(),
            local: Some(LocalCheckout {
                has_upstream: false,
                summaries_enabled: false,
                local_content: Arc::new(Mutex::new(LocalContent::default())),
                morphed: Arc::new(AtomicBool::new(false)),
            }),
        }
    }

    /// Width at which all seven full-form tabs fit, so the snapshots capture the
    /// full bar (the compact fallback has its own test).
    const WIDE: usize = 200;

    /// A worktree row whose three diff tabs (working-tree, branch-diff, upstream)
    /// all have content — every diff signal known and non-empty.
    const CONTENT_FULL: LocalContent = LocalContent {
        working_tree: Some(true),
        branch_diff: Some(true),
        upstream_diverged: Some(true),
    };

    /// A worktree row whose three diff tabs are all *known* empty — a clean
    /// working tree, no commits ahead, and up to date with a present upstream.
    const CONTENT_EMPTY: LocalContent = LocalContent {
        working_tree: Some(false),
        branch_diff: Some(false),
        upstream_diverged: Some(false),
    };

    #[test]
    fn test_render_preview_tabs() {
        // Each mode active, on a worktree row whose diffs all have content
        // (uncommitted changes, commits ahead, diverged from upstream) with
        // summaries enabled but no PR (tabs 1-5 available; tabs 6 pr and 7
        // comments dim). The active mode's label is bold, inactive available
        // labels dim, and on the `pr` iteration tab 6 is active-but-empty —
        // exercising the rule that emptiness dims even the active tab. Verifies
        // labels and structure.
        let wt = TabAvailability::worktree(CONTENT_FULL, true, true, false);
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

        // Empty states. Three rows:
        // - `empty_upstream_and_summary`: diffs still loading (so tabs 1 and 3
        //   read as available), no upstream ref, summaries disabled, no PR — dims
        //   tabs 4, 5, 6, and 7.
        // - `empty_all_local_diffs`: every diff *known* empty (clean working tree,
        //   no commits ahead, up to date with a present upstream) — dims tabs 1,
        //   3, 4, plus 5/6/7, leaving only `log`. This is the behavior the diff
        //   tabs gained: a dimmed number once the diff is known empty.
        // - `pr_row`: a listed-PR row dims the working-tree/branch-diff/upstream/
        //   summary tabs but keeps log/pr/comments.
        assert_snapshot!(
            "empty_upstream_and_summary",
            render_preview_tabs(
                PreviewMode::WorkingTree,
                TabAvailability::worktree(LocalContent::default(), false, false, false),
                WIDE,
            )
        );
        assert_snapshot!(
            "empty_all_local_diffs",
            render_preview_tabs(
                PreviewMode::WorkingTree,
                TabAvailability::worktree(CONTENT_EMPTY, true, false, false),
                WIDE,
            )
        );
        assert_snapshot!(
            "pr_row",
            render_preview_tabs(PreviewMode::Pr, TabAvailability::listed_pr(), WIDE)
        );
    }

    /// `LocalContent::from_item` reads each diff tab's emptiness off the row's
    /// `ListItem`, matching exactly what the corresponding pane renders.
    #[test]
    fn local_content_mirrors_item_diff_state() {
        use crate::commands::list::model::{
            ItemKind, UpstreamStatus, WorkingTreeStatus, WorktreeData,
        };

        // A branch-only row has no working tree, so its working-tree tab is empty
        // immediately (no loading window). The other signals are still unknown.
        let branch = ListItem::new_branch("abc".into(), "feature".into());
        let c = LocalContent::from_item(&branch);
        assert_eq!(c.working_tree, Some(false), "branch-only: no working tree");
        assert_eq!(c.branch_diff, None, "branch_diff loads from item.counts");
        assert_eq!(
            c.upstream_diverged, None,
            "upstream loads from item.upstream"
        );

        // `branch_diff` follows the commits-ahead count vs the local default (the
        // pane's base), not `has_file_changes` (integration target).
        use crate::commands::list::model::AheadBehind;
        let mut ahead = ListItem::new_branch("abc".into(), "feature".into());
        ahead.counts = Some(AheadBehind {
            ahead: 2,
            behind: 0,
        });
        assert_eq!(LocalContent::from_item(&ahead).branch_diff, Some(true));
        ahead.counts = Some(AheadBehind {
            ahead: 0,
            behind: 3,
        });
        assert_eq!(
            LocalContent::from_item(&ahead).branch_diff,
            Some(false),
            "behind-only (no commits ahead) is empty"
        );
        // An orphan has an ill-defined three-dot diff — stays available even with
        // a zero ahead count.
        ahead.is_orphan = Some(true);
        assert_eq!(
            LocalContent::from_item(&ahead).branch_diff,
            Some(true),
            "orphan keeps branch_diff available"
        );

        // `upstream_diverged` is true only when a *present* upstream is ahead or
        // behind; a configured-but-even upstream, and no remote at all, are empty.
        let with_upstream = |remote: Option<&str>, a: usize, b: usize| {
            let mut item = ListItem::new_branch("abc".into(), "feature".into());
            item.upstream = Some(UpstreamStatus {
                remote: remote.map(String::from),
                ahead: a,
                behind: b,
            });
            LocalContent::from_item(&item).upstream_diverged
        };
        assert_eq!(with_upstream(Some("origin/feature"), 2, 0), Some(true));
        assert_eq!(with_upstream(Some("origin/feature"), 0, 3), Some(true));
        assert_eq!(
            with_upstream(Some("origin/feature"), 0, 0),
            Some(false),
            "up to date with a present upstream is empty"
        );
        assert_eq!(
            with_upstream(None, 0, 0),
            Some(false),
            "no remote configured is empty"
        );

        // `working_tree` matches `git diff HEAD`: tracked changes count, untracked
        // alone does not (the pane wouldn't show them).
        let worktree_with = |status: WorkingTreeStatus| {
            let mut item = ListItem::new_branch("abc".into(), "feature".into());
            item.kind = ItemKind::Worktree(Box::new(WorktreeData {
                working_tree_status: Some(status),
                ..Default::default()
            }));
            LocalContent::from_item(&item).working_tree
        };
        // staged, modified, renamed, deleted → tracked content present.
        assert_eq!(
            worktree_with(WorkingTreeStatus::new(false, true, false, false, false)),
            Some(true),
            "modified is tracked content"
        );
        // untracked only → `git diff HEAD` is empty.
        assert_eq!(
            worktree_with(WorkingTreeStatus::new(false, false, true, false, false)),
            Some(false),
            "untracked alone is not shown by `git diff HEAD`"
        );
        // A worktree whose status task hasn't landed yet stays loading.
        let mut pending = ListItem::new_branch("abc".into(), "feature".into());
        pending.kind = ItemKind::Worktree(Box::default());
        assert_eq!(LocalContent::from_item(&pending).working_tree, None);
    }

    /// The diff tabs stay available while their signal is still loading, dim once
    /// it's known empty, and `upstream` additionally requires the synchronous
    /// `has_upstream` floor — a branch with no tracking ref dims immediately, even
    /// before (or despite) the live ahead/behind signal.
    #[test]
    fn diff_tabs_dim_only_once_known_empty() {
        let has = |avail: TabAvailability| (avail.working_tree, avail.branch_diff, avail.upstream);

        // Loading (default): every diff tab available — we don't dim before we know.
        assert_eq!(
            has(TabAvailability::worktree(
                LocalContent::default(),
                true,
                false,
                false
            )),
            (true, true, true),
            "loading → available"
        );

        // Known empty + a present upstream: all three dim.
        assert_eq!(
            has(TabAvailability::worktree(CONTENT_EMPTY, true, false, false)),
            (false, false, false),
            "known empty → dim"
        );

        // Known non-empty: all three available.
        assert_eq!(
            has(TabAvailability::worktree(CONTENT_FULL, true, false, false)),
            (true, true, true),
            "known content → available"
        );

        // No tracking ref: the upstream tab dims regardless of the live signal —
        // the synchronous floor wins over a (stale or loading) divergence read.
        assert!(
            !has(TabAvailability::worktree(CONTENT_FULL, false, false, false)).2,
            "no upstream ref → dim despite a 'diverged' signal"
        );
    }

    /// The summary tab needs both the process-wide `[commit.generation]` flag and
    /// something to summarize — its content is the combined diff (`branch_diff` or
    /// `working_tree`), so it dims in lockstep with the "no changes to summarize"
    /// pane rather than only when summaries are disabled. Like the diff tabs, it
    /// stays available while either signal is still loading.
    #[test]
    fn summary_tab_tracks_combined_diff_content() {
        let summary = |content, summaries_enabled| {
            TabAvailability::worktree(content, true, summaries_enabled, false).summary
        };

        // Summaries disabled dims the tab regardless of content.
        assert!(!summary(CONTENT_FULL, false), "disabled → dim");

        // Enabled + content in either diff → available.
        assert!(summary(CONTENT_FULL, true), "enabled + content → available");

        // Enabled but both diffs *known* empty (clean tree, no commits ahead) →
        // dim, matching the "has no changes to summarize" pane. This is the bug
        // the gate fixes: previously the tab stayed solid here.
        assert!(
            !summary(CONTENT_EMPTY, true),
            "enabled + nothing to summarize → dim"
        );

        // Only one diff has content → still something to summarize → available.
        let working_only = LocalContent {
            working_tree: Some(true),
            branch_diff: Some(false),
            upstream_diverged: Some(false),
        };
        let branch_only = LocalContent {
            working_tree: Some(false),
            branch_diff: Some(true),
            upstream_diverged: Some(false),
        };
        assert!(
            summary(working_only, true),
            "uncommitted changes → available"
        );
        assert!(summary(branch_only, true), "commits ahead → available");

        // Still loading (both unknown) → available; don't dim before we know.
        assert!(
            summary(LocalContent::default(), true),
            "loading → available"
        );
    }

    /// Narrow panes can't fit the full bar (skim renders previews with wrapping
    /// off, so it would truncate the `pr` / `comments` tabs — exactly the ones
    /// with content on a `--prs` row). The compact fallback keeps every
    /// accelerator digit visible and labels only the active tab.
    #[test]
    fn render_preview_tabs_compacts_when_narrow() {
        // A --prs row with the `pr` tab active, in a narrow pane.
        let compact = render_preview_tabs(PreviewMode::Pr, TabAvailability::listed_pr(), 40);
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
        let full = render_preview_tabs(PreviewMode::Pr, TabAvailability::listed_pr(), WIDE);
        assert!(
            full.contains("7: ") && full.contains("comments"),
            "wide pane keeps full labels"
        );

        // Boundary: the switch is `visual_width(full) <= width`. Measure the full
        // bar's own width, then check that exactly that width stays full while one
        // column narrower compacts (the `pr` tab is active, so only the full bar
        // carries the inactive `comments` label).
        let avail = TabAvailability::listed_pr();
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
                PickerRow::loading_placeholder(mode)
            );
        }
    }

    #[test]
    fn test_preview_for_mode_summary_cache() {
        // Cache hit returns cached content; cache miss computes the placeholder
        let cache_hit = {
            let preview_cache: PreviewCache = Arc::new(DashMap::new());
            preview_cache.insert(
                ("feature".to_string(), PreviewMode::Summary),
                "Add auth module\n\nImplements JWT-based authentication.".to_string(),
            );
            worktree_test_row("feature", preview_cache, None)
        };

        let cache_miss = worktree_test_row("feature", Arc::new(DashMap::new()), None);

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
    fn row_url_resolve() {
        use crate::commands::list::ci_status::{CiSource, CiStatus, PrRef, ReviewState};

        // Static URL (a `--prs` row): resolves directly.
        assert_eq!(
            RowUrl::Static(Some("https://x/pull/1".into()))
                .resolve()
                .as_deref(),
            Some("https://x/pull/1")
        );
        assert_eq!(RowUrl::Static(None).resolve(), None);

        // Live slot (a worktree row): `None` (still fetching) and `Some(None)`
        // (no PR) both resolve to no URL — alt-o is a no-op until a PR lands.
        let loading: PrStatusSlot = Arc::new(Mutex::new(None));
        assert_eq!(RowUrl::Live(loading).resolve(), None);
        let no_pr: PrStatusSlot = Arc::new(Mutex::new(Some(None)));
        assert_eq!(RowUrl::Live(no_pr).resolve(), None);

        // A live status carrying a URL resolves to it.
        let status = PrStatus {
            ci_status: CiStatus::Passed,
            source: CiSource::PullRequest,
            is_stale: false,
            is_priming: false,
            url: Some("https://x/pull/2".into()),
            number: Some(PrRef::pr(2)),
            review_state: Some(ReviewState::Approved),
            title: None,
            body: None,
            author: None,
            comment_count: None,
            updated_at: None,
        };
        let with_url: PrStatusSlot = Arc::new(Mutex::new(Some(Some(status))));
        assert_eq!(
            RowUrl::Live(with_url).resolve().as_deref(),
            Some("https://x/pull/2")
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
            author: None,
            comment_count: None,
            updated_at: None,
        };
        // Build a row whose live `pr_status` slot carries a given state — what
        // the picker primes from the cache and then overwrites as the fetch lands.
        let row = |pr_status: Option<Option<PrStatus>>| {
            worktree_test_row("feature", Arc::new(DashMap::new()), pr_status)
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
        // No body → no description block (it prefixes a blank line + reset).
        assert!(
            !pane.contains("\n\n\x1b[0m"),
            "no description block without a body: {pane:?}"
        );

        // A PR carrying title + body → the title rides the header and the body
        // renders flush as markdown, matching the `--prs` pane.
        let full = row(Some(Some(status(
            Some(PrRef::pr(7)),
            Some("https://github.com/o/r/pull/7"),
            Some("Fix the flaky retry"),
            Some("Adds a **bounded** retry."),
        ))));
        let pane = full.render_pr_pane(full.pr_preview(), 80);
        assert!(pane.contains("#7"), "reference: {pane:?}");
        assert!(pane.contains("Fix the flaky retry"), "title: {pane:?}");
        assert!(pane.contains("\n\n\x1b[0m"), "description block: {pane:?}");
        assert!(
            pane.contains("DESCRIPTION"),
            "cyan `DESCRIPTION` label heads the block: {pane:?}"
        );
        assert!(
            !pane.contains("\x1b[107m"),
            "renders flush, no gutter: {pane:?}"
        );
        assert!(
            pane.contains("bounded"),
            "description body rendered: {pane:?}"
        );
        assert!(pane.contains("\x1b[1m"), "markdown bold rendered: {pane:?}");
        assert!(!pane.contains("**"), "markdown markers consumed: {pane:?}");

        // Cached branch CI with no PR `number` → NoPr.
        let branch_ci = row(Some(Some(status(None, None, None, None))));
        assert!(matches!(branch_ci.pr_preview(), PrPreview::NoPr));
    }

    #[test]
    fn comments_tab_mirrors_pr_status_and_reads_the_shared_cache() {
        use crate::commands::list::ci_status::{CiSource, CiStatus, PrRef};

        // A worktree row's `comments` tab behaves like a `--prs` row's: with a
        // PR it reads the background-fetched thread from the shared cache (keyed
        // by branch name), and with no PR — or while CI is still loading — it
        // mirrors the `pr` tab's empty/loading states.
        let pr_status = |number: Option<PrRef>| {
            Some(Some(PrStatus {
                ci_status: CiStatus::Passed,
                source: CiSource::PullRequest,
                is_stale: false,
                is_priming: false,
                url: None,
                number,
                review_state: None,
                title: None,
                body: None,
                author: None,
                comment_count: None,
                updated_at: None,
            }))
        };
        let row = |slot: Option<Option<PrStatus>>, cache: PreviewCache| {
            worktree_test_row("feature", cache, slot)
        };

        // CI hasn't reported (None) → the shared "Fetching PR status…" hint (it
        // auto-resolves once the live status lands; see `PreviewNotifier`).
        let loading = row(None, Arc::new(DashMap::new()));
        let pane = loading.render_comments_pane();
        assert!(pane.contains("Fetching PR status"), "loading: {pane:?}");

        // No PR (Some(None)) → "has no PR", matching the `pr` tab.
        let no_pr = row(Some(None), Arc::new(DashMap::new()));
        assert!(
            no_pr.render_comments_pane().contains("has no PR"),
            "no-pr state"
        );

        // A PR with the thread already cached → the cached thread is shown.
        let cache: PreviewCache = Arc::new(DashMap::new());
        cache.insert(
            ("feature".to_string(), PreviewMode::Comments),
            "@octocat\nLooks good\n".to_string(),
        );
        let with_thread = row(pr_status(Some(PrRef::pr(7))), Arc::clone(&cache));
        assert_eq!(
            with_thread.render_comments_pane(),
            "@octocat\nLooks good\n",
            "cached thread served verbatim"
        );

        // A PR whose fetch hasn't landed → the shared `--prs` loading placeholder,
        // so a worktree row and a `--prs` row show the identical in-flight pane.
        let pending = row(pr_status(Some(PrRef::pr(7))), Arc::new(DashMap::new()));
        assert_eq!(
            pending.render_comments_pane(),
            super::super::prs::pr_deferred_loading(PreviewMode::Comments),
            "cache miss falls back to the shared loading placeholder"
        );
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
            author: None,
            comment_count,
            updated_at: None,
        };

        // A PR with comments adds a cyan all-caps `COMMENTS` metadata line
        // carrying the count (same `field_label` styling as BRANCH/URL).
        let with = render_pr_pane_body("feature", PrRef::pr(7), &status(Some(3)), 80)
            .ansi_strip()
            .to_string();
        assert!(
            with.lines()
                .any(|l| l.contains("COMMENTS") && l.contains('3')),
            "comments line with count: {with:?}"
        );

        // No comments (zero is flattened to `None` at the mapping boundary, and an
        // older cache entry carries `None` too) → the line is skipped entirely.
        let without = render_pr_pane_body("feature", PrRef::pr(7), &status(None), 80)
            .ansi_strip()
            .to_string();
        assert!(
            !without.contains("COMMENTS"),
            "no comments line when absent: {without:?}"
        );
    }

    /// The worktree-row `pr` pane shows the PR author, the same as a `--prs`
    /// row's pane — `PrStatus` now carries it from the CI fetch. Absent author
    /// (older cache entry, forge without the field) skips the line.
    #[test]
    fn worktree_pr_pane_shows_author() {
        use crate::commands::list::ci_status::{CiSource, CiStatus, PrRef};

        let status = |author: Option<&str>| PrStatus {
            ci_status: CiStatus::Passed,
            source: CiSource::PullRequest,
            is_stale: false,
            is_priming: false,
            url: None,
            number: Some(PrRef::pr(7)),
            review_state: None,
            title: Some("Fix the flaky retry".into()),
            body: None,
            author: author.map(str::to_owned),
            comment_count: None,
            updated_at: None,
        };

        let with = render_pr_pane_body("feature", PrRef::pr(7), &status(Some("bob")), 80)
            .ansi_strip()
            .to_string();
        assert!(
            with.lines()
                .any(|l| l.contains("AUTHOR") && l.contains("@bob")),
            "author line: {with:?}"
        );

        let without = render_pr_pane_body("feature", PrRef::pr(7), &status(None), 80)
            .ansi_strip()
            .to_string();
        assert!(
            !without.contains("AUTHOR"),
            "no author line when absent: {without:?}"
        );
    }

    #[test]
    fn preview_assembles_tab_bar_and_pr_pane() {
        use crate::commands::list::ci_status::{CiSource, CiStatus, PrRef, ReviewState};

        // A worktree row whose live status carries a PR with title + body.
        let row = worktree_test_row(
            "feature",
            Arc::new(DashMap::new()),
            Some(Some(PrStatus {
                ci_status: CiStatus::Passed,
                source: CiSource::PullRequest,
                is_stale: false,
                is_priming: false,
                url: Some("https://github.com/o/r/pull/7".into()),
                number: Some(PrRef::pr(7)),
                review_state: Some(ReviewState::Approved),
                title: Some("Fix the flaky retry".into()),
                body: Some("Adds a **bounded** retry.".into()),
                author: None,
                comment_count: None,
                updated_at: None,
            })),
        );

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
            !pr_pane.contains("\x1b[107m"),
            "description renders flush, no gutter: {pr_pane:?}"
        );
        assert!(
            pr_pane.contains("bounded"),
            "description body rendered: {pr_pane:?}"
        );

        // `SkimItem::preview` reads the default in-memory mode (WorkingTree, since
        // no tab switch happens in this test) and delegates to `render_preview`,
        // exercising the wrapper and the non-pr dispatch arm (cache miss → loading
        // placeholder).
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
                author: None,
                comment_count: None,
                updated_at: None,
            }))
        };
        // Share the cache and slot Arcs so the test can mutate the slot and
        // invalidate the entry the way the collect handler's `on_update` does.
        let cache: PreviewCache = Arc::new(DashMap::new());
        let slot: PrStatusSlot = Arc::new(Mutex::new(status("First title")));
        let key = ("feature".to_string(), PreviewMode::Pr);
        let row = PickerRow {
            search_base: String::new(),
            gutter: '@',
            rendered: Arc::new(Mutex::new(String::new())),
            branch_name: "feature".into(),
            output_token: "feature".into(),
            preview_cache: Arc::clone(&cache),
            pr_status: Arc::clone(&slot),
            notifier: PreviewNotifier::detached(),
            local: Some(LocalCheckout {
                has_upstream: false,
                summaries_enabled: false,
                local_content: Arc::new(Mutex::new(LocalContent::default())),
                morphed: Arc::new(AtomicBool::new(false)),
            }),
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
        let output = PickerRow::compute_branch_diff_preview(&repo, &item, 80);
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
        let output = PickerRow::compute_branch_diff_preview(&repo, &item, 80);
        assert!(
            output.contains("feat.txt"),
            "expected diff to mention feat.txt, got: {output:?}"
        );
    }

    #[test]
    fn branch_diff_orphan_shows_full_content() {
        // An orphan branch shares no history with the comparison base, so its
        // three-dot diff has no merge base. The pane must fall back to the
        // orphan's full content (vs the empty tree) rather than "no file
        // changes" — matching the dim signal, which keeps tab 3 available for
        // orphans.
        let (t, repo) = repo_with_main();
        repo.run_command(&["checkout", "--orphan", "orphan-feature"])
            .unwrap();
        repo.run_command(&["rm", "-rf", "."]).unwrap();
        std::fs::write(t.path().join("orphan.txt"), "orphan\n").unwrap();
        repo.run_command(&["add", "orphan.txt"]).unwrap();
        repo.run_command(&["commit", "-m", "orphan root"]).unwrap();
        let item = item_at(&repo, "orphan-feature");
        let output = PickerRow::compute_branch_diff_preview(&repo, &item, 80);
        assert!(
            output.contains("orphan.txt"),
            "orphan pane should show full content, got: {output:?}"
        );
    }

    #[test]
    fn branch_diff_uses_upstream_aware_base() {
        // When the local default lags its upstream, the pane must diff against
        // the upstream tip (the comparison base), not the stale local default —
        // otherwise it shows upstream commits as this branch's own changes.
        let (t, repo) = repo_with_main();
        // origin-main: one commit ahead of main, with main's tracking ref
        // pointed at it so local main lags by that commit.
        repo.run_command(&["branch", "origin-main"]).unwrap();
        repo.run_command(&["checkout", "origin-main"]).unwrap();
        std::fs::write(t.path().join("upstream.txt"), "from upstream\n").unwrap();
        repo.run_command(&["add", "upstream.txt"]).unwrap();
        repo.run_command(&["commit", "-m", "upstream commit"])
            .unwrap();
        // feature branches off the upstream tip and adds its own commit.
        repo.run_command(&["checkout", "-b", "feature"]).unwrap();
        std::fs::write(t.path().join("feature.txt"), "feature work\n").unwrap();
        repo.run_command(&["add", "feature.txt"]).unwrap();
        repo.run_command(&["commit", "-m", "feature commit"])
            .unwrap();
        repo.run_command(&["branch", "--set-upstream-to=origin-main", "main"])
            .unwrap();
        let item = item_at(&repo, "feature");
        let output = PickerRow::compute_branch_diff_preview(&repo, &item, 80);
        assert!(
            output.contains("feature.txt"),
            "expected feature.txt, got: {output:?}"
        );
        assert!(
            !output.contains("upstream.txt"),
            "must diff vs the upstream-aware base, not the stale local default; got: {output:?}"
        );
    }

    #[test]
    fn prime_comparison_base_feeds_branch_diff_spec() {
        // Priming the comparison base from the collector's already-captured
        // snapshot must seed the same upstream-aware base the lazy path would
        // resolve, so the preview pane reuses that scan instead of capturing
        // its own. Same upstream-aware setup as the test above: local main
        // lags origin-main, so the base is origin-main, not the stale local.
        let (t, repo) = repo_with_main();
        repo.run_command(&["branch", "origin-main"]).unwrap();
        repo.run_command(&["checkout", "origin-main"]).unwrap();
        std::fs::write(t.path().join("upstream.txt"), "from upstream\n").unwrap();
        repo.run_command(&["add", "upstream.txt"]).unwrap();
        repo.run_command(&["commit", "-m", "upstream commit"])
            .unwrap();
        repo.run_command(&["checkout", "-b", "feature"]).unwrap();
        std::fs::write(t.path().join("feature.txt"), "feature work\n").unwrap();
        repo.run_command(&["add", "feature.txt"]).unwrap();
        repo.run_command(&["commit", "-m", "feature commit"])
            .unwrap();
        repo.run_command(&["branch", "--set-upstream-to=origin-main", "main"])
            .unwrap();

        let head = repo
            .run_command(&["rev-parse", "refs/heads/feature"])
            .unwrap();
        let snapshot = repo.capture_refs().unwrap();
        repo.prime_comparison_base(&snapshot);

        let spec = repo
            .branch_diff_spec(head.trim())
            .expect("primed base resolves a spec");
        assert_eq!(
            spec.base_name, "origin-main",
            "primed base must be the upstream-aware comparison base, got: {:?}",
            spec.base_name
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

        let output = PickerRow::compute_branch_diff_preview(&repo, &item, 80);
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
        let _ = PickerRow::compute_branch_diff_preview(&repo, &item, 80);
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
        let _ = PickerRow::compute_log_preview(&repo, &item, 80, 24).0;
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
        let before = PickerRow::compute_log_preview(&repo, &item, 80, 24).0;
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

        let after = PickerRow::compute_log_preview(&repo, &item, 80, 24).0;
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

        let output = PickerRow::compute_upstream_diff_preview(&repo, &item, 80);
        assert_eq!(output, sentinel);
    }

    #[test]
    fn upstream_diff_no_tracking_branch() {
        // Branch with no configured upstream should hit the no-upstream path
        // via non-zero exit from `git rev-list --left-right --count HEAD...@{u}`.
        let (_t, repo) = repo_with_main();
        repo.run_command(&["branch", "orphan"]).unwrap();
        let item = item_at(&repo, "orphan");
        let output = PickerRow::compute_upstream_diff_preview(&repo, &item, 80);
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
        let output = PickerRow::compute_upstream_diff_preview(&repo, &item, 80);
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
        let output = PickerRow::compute_upstream_diff_preview(&repo, &item, 80);
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
        let output = PickerRow::compute_upstream_diff_preview(&repo, &item, 80);
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
        let output = PickerRow::compute_upstream_diff_preview(&repo, &item, 80);
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
            TabAvailability::worktree(CONTENT_FULL, true, true, false),
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
