//! Open PR/MR picker source (`wt switch --prs`).
//!
//! Widens the interactive picker with the repository's open pull requests
//! (GitHub) or merge requests (GitLab). Each row's `output()` is the
//! `pr:{N}` / `mr:{N}` shortcut, so selection routes through the exact same
//! [`SwitchPipeline`](super::super::worktree::SwitchPipeline) as
//! `wt switch pr:{N}` — fetch the ref, switch to its branch. No new switch
//! logic: the shortcut parsing in `commands::worktree::switch` already
//! resolves both same-repo and fork PRs/MRs.
//!
//! # Streaming
//!
//! The list is a single forge call (`gh pr list` / `glab mr list`) run on a
//! dedicated thread that holds a clone of skim's item channel. The picker
//! frame paints instantly from local worktree data; PR rows appear when the
//! call returns (~1s). The thread's sender drop is part of the picker's
//! EOF contract — skim's reader sees end-of-stream only once every sender
//! drops — see [`super::handle_picker`].
//!
//! # Alignment
//!
//! PR rows render on the same column grid as the worktree rows: a dim `#`
//! gutter sigil (see [`PR_GUTTER_SIGIL`]), the head branch in the Branch
//! column, and the number — in the CI column when the layout has one, else
//! just after the branch so it never hides. The PR title and author have no
//! worktree-column equivalent, so they stay off the row (in the `pr` preview
//! tab) rather than misaligning the grid — see [`render_grid_row`]. The geometry
//! ([`ColumnGrid`]) is computed by the collect thread at skeleton time and
//! handed over through a [`GridSlot`]; the skeleton (~50ms) beats the forge
//! call (~1s), so the wait is nominal. Without a grid (handoff timed out, or
//! collect never produced a skeleton) rows fall back to a freeform line.
//!
//! # Scope
//!
//! GitHub and GitLab only. Gitea and Azure DevOps support `pr:{N}` for a
//! single known number but have no listing path here yet.

use std::borrow::Cow;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use anstyle::{Reset, Style};
use anyhow::Context;
use color_print::cformat;
use ratatui::text::Line;
use serde::Deserialize;
use skim::prelude::*;
use unicode_width::UnicodeWidthStr;
use worktrunk::git::{CiPlatform, Repository};
use worktrunk::styling::{INFO_SYMBOL, StyledLine, format_with_gutter, warning_message};

use super::super::list::ci_status::{
    CiSource, CiStatus, GitHubPrInfo, PrRef, PrStatus, ReviewState, non_interactive_cmd,
    tool_available,
};
use super::super::list::columns::ColumnKind;
use super::super::list::layout::ColumnGrid;
use super::items::{TabAvailability, ansi_to_line, render_preview_tabs};
use super::preview::{PreviewMode, PreviewStateData};

/// One-shot handoff of the picker's column geometry from the collect thread
/// (which computes the layout at skeleton time) to the `--prs` thread (which
/// renders rows once the forge call returns). First write wins — an alt-r
/// reload re-fires the skeleton at the same width, so later grids are
/// identical.
pub(super) struct GridSlot {
    grid: Mutex<Option<ColumnGrid>>,
    ready: Condvar,
}

impl GridSlot {
    pub(super) fn new() -> Self {
        Self {
            grid: Mutex::new(None),
            ready: Condvar::new(),
        }
    }

    pub(super) fn set(&self, grid: ColumnGrid) {
        let mut slot = self.grid.lock().unwrap();
        if slot.is_none() {
            *slot = Some(grid);
        }
        self.ready.notify_all();
    }

    /// Block until the grid is set or `timeout` elapses. The timeout covers
    /// collect exiting without a skeleton (zero items, error) — the rows
    /// then render freeform rather than never.
    fn wait(&self, timeout: Duration) -> Option<ColumnGrid> {
        let (slot, _) = self
            .ready
            .wait_timeout_while(self.grid.lock().unwrap(), timeout, |grid| grid.is_none())
            .unwrap();
        slot.clone()
    }
}

/// Open PRs/MRs to list. One page is one API call; 50 covers any repo a human
/// browses interactively without paginating.
const MAX_PRS: u8 = 50;

/// Gutter sigil for a `--prs` row — a PR/MR with no local branch or worktree.
/// `#` completes the picker's gutter scheme alongside worktree rows
/// (`@`/`^`/`+`) and branch rows (`/` local, `|` remote — see
/// `BranchScope::gutter_sigil`), rendered dim and single-width ASCII to match
/// them and dodge skim's `width_cjk` clipping. The trailing space pads the
/// 2-cell gutter column.
const PR_GUTTER_SIGIL: &str = "# ";

/// Whether a listed ref is a GitHub PR or a GitLab MR. Drives the `output()`
/// shortcut (`pr:`/`mr:`) and the row label.
#[derive(Clone, Copy)]
enum RefKind {
    Pr,
    Mr,
}

impl RefKind {
    /// Shortcut prefix understood by `wt switch` (`pr` / `mr`).
    fn shortcut(self) -> &'static str {
        match self {
            RefKind::Pr => "pr",
            RefKind::Mr => "mr",
        }
    }
}

/// One open PR/MR, normalized across forges for the picker row.
struct PrEntry {
    number: u32,
    title: String,
    head_branch: String,
    author: String,
    is_draft: bool,
    url: Option<String>,
    kind: RefKind,
    /// PR/MR description (GitHub `body`, GitLab `description`), rendered as
    /// markdown in the `pr` preview tab. Rides the one list call — empty when
    /// the forge returns no body.
    body: String,
    /// CI + review status for the CI column, built from the same forge call.
    /// `None` when the forge can't supply it in one call (the row then keeps
    /// its `#N` in the title instead of the CI column).
    status: Option<PrStatus>,
}

impl PrEntry {
    /// The forge-correct reference: `#N` on GitHub, `!N` on GitLab. Shared by
    /// the row and preview renderers so both pick the sigil from one place.
    fn pr_ref(&self) -> PrRef {
        match self.kind {
            RefKind::Pr => PrRef::pr(u64::from(self.number)),
            RefKind::Mr => PrRef::mr(u64::from(self.number)),
        }
    }
}

/// Stream the open PRs/MRs into the picker, then clear the header's "loading…"
/// marker — whatever the outcome.
///
/// The forge call drives [`fetch_and_stream`]; this wrapper owns the marker so
/// every exit (rows sent, no PRs, or error) drops it and repaints. The
/// success path's row send already wakes skim, but the empty/error paths send
/// nothing, so without the poke the marker would linger until the next
/// keystroke.
pub(super) fn stream_open_prs(
    repo: &Repository,
    list_width: usize,
    tx: &SkimItemSender,
    stashed_warnings: &Mutex<Vec<String>>,
    grid_slot: &GridSlot,
    prs_loading: &AtomicBool,
    render_tx: &OnceLock<tokio::sync::mpsc::Sender<Event>>,
) {
    fetch_and_stream(repo, list_width, tx, stashed_warnings, grid_slot);

    prs_loading.store(false, Ordering::Relaxed);
    if let Some(tx) = render_tx.get() {
        let _ = tx.try_send(Event::Render);
    }
}

/// Fetch open PRs/MRs, build picker rows, and stream them into skim.
///
/// On failure (forge unsupported, CLI missing/unauthenticated, network error)
/// the reason is stashed for display after skim releases the terminal — the
/// picker stays usable with its worktree rows.
fn fetch_and_stream(
    repo: &Repository,
    list_width: usize,
    tx: &SkimItemSender,
    stashed_warnings: &Mutex<Vec<String>>,
    grid_slot: &GridSlot,
) {
    let entries = match fetch_open_prs(repo) {
        Ok(entries) => entries,
        Err(e) => {
            stashed_warnings
                .lock()
                .unwrap()
                .push(warning_message(format!("{e:#}")).to_string());
            return;
        }
    };

    if entries.is_empty() {
        let noun = forge_noun(repo);
        stashed_warnings
            .lock()
            .unwrap()
            .push(warning_message(format!("No open {noun} found")).to_string());
        return;
    }

    // The forge call above (~1s) almost always outlasts the skeleton
    // (~50ms), so this returns immediately; the wait covers a mocked forge
    // CLI winning the race.
    let grid = grid_slot.wait(Duration::from_secs(5));

    // skim 4.x takes a batch per send; the forge call already returned every
    // row, so stream them in one shot.
    let items: Vec<Arc<dyn SkimItem>> = entries
        .into_iter()
        .map(|entry| {
            Arc::new(PrSkimItem::new(entry, list_width, grid.as_ref())) as Arc<dyn SkimItem>
        })
        .collect();
    let _ = tx.send(items);
}

/// Plural noun for the forge's change-request — "PRs" on GitHub, "MRs" on
/// GitLab. Used for the empty-list message and the header "loading…" marker,
/// where there's no entry to read the kind from.
pub(super) fn forge_noun(repo: &Repository) -> &'static str {
    match repo.ci_platform(None) {
        Some(CiPlatform::GitLab) => "MRs",
        _ => "PRs",
    }
}

/// Dispatch to the forge that hosts this repository's primary remote.
fn fetch_open_prs(repo: &Repository) -> anyhow::Result<Vec<PrEntry>> {
    let repo_root = repo
        .current_worktree()
        .root()
        .context("Failed to resolve worktree root for --prs")?;

    match repo.ci_platform(None) {
        Some(CiPlatform::GitHub) => fetch_github(&repo_root),
        Some(CiPlatform::GitLab) => fetch_gitlab(&repo_root),
        Some(other) => {
            anyhow::bail!("--prs supports GitHub and GitLab; this repository's forge is {other}")
        }
        None => anyhow::bail!("--prs could not determine the forge from the remote URL"),
    }
}

#[derive(Deserialize)]
struct GhPr {
    title: String,
    #[serde(rename = "headRefName")]
    head_ref_name: String,
    #[serde(default)]
    author: GhAuthor,
    /// PR description; shown in the `pr` preview tab. Rides the one list call.
    #[serde(default)]
    body: String,
    /// CI/review fields reused via the shared `gh pr list` mapping: number,
    /// `isDraft`, `url`, `statusCheckRollup`, `reviewDecision`,
    /// `mergeStateStatus`. Flattened so one parse feeds both display and the
    /// CI-column status.
    #[serde(flatten)]
    info: GitHubPrInfo,
}

#[derive(Deserialize, Default)]
struct GhAuthor {
    #[serde(default)]
    login: String,
}

fn fetch_github(repo_root: &Path) -> anyhow::Result<Vec<PrEntry>> {
    if !tool_available("gh", &["--version"]) {
        anyhow::bail!("gh CLI not found; install gh to browse PRs with --prs");
    }

    let output = non_interactive_cmd("gh")
        .args([
            "pr",
            "list",
            "--state",
            "open",
            "--limit",
            &MAX_PRS.to_string(),
            "--json",
            // CI/review fields and the description ride the one call; no extra round-trip.
            "number,title,headRefName,author,isDraft,url,body,statusCheckRollup,reviewDecision,mergeStateStatus",
        ])
        .current_dir(repo_root)
        .run()
        .context("Failed to run gh pr list")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("gh pr list failed: {}", stderr.trim());
    }

    parse_github_prs(&output.stdout)
}

/// Map `gh pr list --json …` output to picker entries.
fn parse_github_prs(stdout: &[u8]) -> anyhow::Result<Vec<PrEntry>> {
    let prs: Vec<GhPr> =
        serde_json::from_slice(stdout).context("Failed to parse gh pr list JSON")?;

    Ok(prs
        .into_iter()
        .map(|pr| PrEntry {
            number: pr.info.number.unwrap_or(0) as u32,
            title: pr.title,
            head_branch: pr.head_ref_name,
            author: pr.author.login,
            is_draft: pr.info.is_draft == Some(true),
            url: pr.info.url.clone(),
            kind: RefKind::Pr,
            body: pr.body,
            status: Some(pr.info.open_pr_status()),
        })
        .collect())
}

#[derive(Deserialize)]
struct GlabMr {
    iid: u32,
    title: String,
    #[serde(default)]
    source_branch: String,
    #[serde(default)]
    author: GlabAuthor,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    web_url: Option<String>,
    /// MR description; shown in the `pr` preview tab. Rides the one list call.
    #[serde(default)]
    description: String,
    /// Coarse merge/CI signal the list call carries (full pipeline status
    /// needs a per-MR `glab mr view`, which `--prs` avoids).
    #[serde(default)]
    detailed_merge_status: Option<String>,
}

#[derive(Deserialize, Default)]
struct GlabAuthor {
    #[serde(default)]
    username: String,
}

fn fetch_gitlab(repo_root: &Path) -> anyhow::Result<Vec<PrEntry>> {
    if !tool_available("glab", &["--version"]) {
        anyhow::bail!("glab CLI not found; install glab to browse MRs with --prs");
    }

    let output = non_interactive_cmd("glab")
        .args([
            "mr",
            "list",
            "--per-page",
            &MAX_PRS.to_string(),
            "--output",
            "json",
        ])
        .current_dir(repo_root)
        .run()
        .context("Failed to run glab mr list")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("glab mr list failed: {}", stderr.trim());
    }

    parse_gitlab_mrs(&output.stdout)
}

/// Map `glab mr list --output json` output to picker entries.
fn parse_gitlab_mrs(stdout: &[u8]) -> anyhow::Result<Vec<PrEntry>> {
    let mrs: Vec<GlabMr> =
        serde_json::from_slice(stdout).context("Failed to parse glab mr list JSON")?;

    Ok(mrs
        .into_iter()
        .map(|mr| {
            let status = gitlab_mr_status(
                mr.iid,
                mr.draft,
                mr.detailed_merge_status.as_deref(),
                mr.web_url.clone(),
            );
            PrEntry {
                number: mr.iid,
                title: mr.title,
                head_branch: mr.source_branch,
                author: mr.author.username,
                is_draft: mr.draft,
                url: mr.web_url,
                kind: RefKind::Mr,
                body: mr.description,
                status: Some(status),
            }
        })
        .collect())
}

/// Best-effort MR status from the single `glab mr list` call. The list payload
/// carries `draft` and `detailed_merge_status` but not pipeline detail (that
/// needs a per-MR `glab mr view`, which `--prs` avoids), so CI is coarse:
/// conflicts and a still-running merge pipeline are the only states the list
/// reports. `not_approved` maps to a pending review; draft outranks it.
fn gitlab_mr_status(
    iid: u32,
    draft: bool,
    detailed_merge_status: Option<&str>,
    url: Option<String>,
) -> PrStatus {
    let ci_status = match detailed_merge_status {
        Some("broken_status") | Some("conflict") => CiStatus::Conflicts,
        Some("ci_still_running") => CiStatus::Running,
        _ => CiStatus::NoCI,
    };
    let review_state = if draft {
        Some(ReviewState::Draft)
    } else if detailed_merge_status == Some("not_approved") {
        Some(ReviewState::Pending)
    } else {
        None
    };
    PrStatus {
        ci_status,
        source: CiSource::PullRequest,
        is_stale: false,
        is_priming: false,
        url,
        number: Some(PrRef::mr(u64::from(iid))),
        review_state,
    }
}

/// A picker row for one open PR/MR. Distinct from `WorktreeSkimItem`: it
/// carries no `ListItem` and resolves to a `pr:`/`mr:` shortcut rather than a
/// branch or worktree path.
pub(super) struct PrSkimItem {
    /// What skim's fuzzy matcher sees: kind, number, title, branch, author.
    search_text: String,
    /// ANSI-colored display line — cells on the worktree rows' column grid,
    /// or a freeform line when no grid is available.
    rendered: String,
    /// Selection result — the `pr:{N}` / `mr:{N}` shortcut. Routed verbatim
    /// through `resolve_identifier` → `SwitchPipeline`.
    output_token: String,
    /// The tab-6 (`pr`) pane: PR/MR metadata and web URL, built once at
    /// construction from already-fetched data. A `--prs` row has no local
    /// worktree, so tabs 1-5 render an empty placeholder instead.
    pr_pane: String,
}

impl PrSkimItem {
    fn new(entry: PrEntry, list_width: usize, grid: Option<&ColumnGrid>) -> Self {
        let label = entry.kind.shortcut();
        let output_token = format!("{label}:{}", entry.number);

        // Trailing gutter glyph (the `#` from `PR_GUTTER_SIGIL`, sans pad) so
        // typing `#` filters to PR/MR rows, matching how the worktree/branch
        // rows fold their sigil in (see `progressive_handler` `on_skeleton`).
        let gutter = PR_GUTTER_SIGIL.trim_end();
        let search_text = format!(
            "{label} {} {} {} {} {gutter}",
            entry.number, entry.title, entry.head_branch, entry.author
        );

        let rendered = match grid {
            Some(grid) => render_grid_row(&entry, grid, list_width),
            None => render_freeform_row(&entry, list_width),
        };

        let pr_ref = entry.pr_ref();
        let PrEntry {
            title,
            head_branch,
            author,
            is_draft,
            url,
            body,
            ..
        } = entry;
        // A full `{reset}` (\x1b[0m) closes every styled span: skim's ANSI
        // parser drops color_print's `</>` (SGR 22/39), so without it the
        // header's bold and each label's dim bleed across the values and on
        // down the pane. Same workaround as `pr_row_empty_placeholder` and the
        // `compute_*` preview helpers; see `render_preview_tabs` for the why.
        let reset = Reset;
        let mut pr_pane = cformat!(
            "<bold>{pr_ref}</>{reset}  {title}\n\n<dim>branch</>{reset}   {head_branch}\n<dim>author</>{reset}   @{author}\n"
        );
        if is_draft {
            pr_pane.push_str(&cformat!(
                "<dim>state</>{reset}    <yellow>draft</>{reset}\n"
            ));
        }
        if let Some(url) = url {
            pr_pane.push_str(&cformat!("<dim>url</>{reset}      {url}\n"));
        }
        pr_pane.push_str(&render_pr_description(&body, list_width));

        Self {
            search_text,
            rendered,
            output_token,
            pr_pane,
        }
    }
}

/// The PR/MR description block for the `pr` preview pane: the body rendered as
/// markdown (bold headers, styled lists and inline code — the same renderer the
/// `summary` tab uses) and quoted in the house gutter ([`format_with_gutter`],
/// a bg-color bar that closes each line with a full `\x1b[0m`, skim-safe). The
/// whole body renders; the preview pane scrolls (`ctrl-u`/`ctrl-d`) through a
/// long one. Empty body → empty string, so the block is skipped. The leading
/// `\x1b[0m` is a defensive boundary so the first gutter line renders clean
/// regardless of what precedes it (the metadata lines already reset their own
/// spans).
///
/// `width` is the list width, a close proxy for the preview pane width in both
/// layouts (Right splits ~50/50; Down gives list and preview the full width).
/// The markdown wraps to the gutter's inner width (the bar plus its pad take
/// two columns) so the gutter's own wrap is a no-op rather than re-breaking the
/// already-styled lines.
fn render_pr_description(body: &str, width: usize) -> String {
    let body = body.trim();
    if body.is_empty() {
        return String::new();
    }
    let reset = Reset;
    let rendered =
        crate::md_help::render_markdown_in_help_with_width(body, Some(width.saturating_sub(2)));
    let gutter = format_with_gutter(&rendered, Some(width));
    format!("\n{reset}{gutter}\n")
}

/// The pane for tabs 1-5 on a `--prs` row. The head branch isn't checked out
/// locally, so there's no working tree / log / diff to show — point the user
/// at the `pr` tab, which holds the PR/MR metadata.
fn pr_row_empty_placeholder() -> String {
    let reset = Reset;
    cformat!(
        "{INFO_SYMBOL}{reset} Not checked out locally — press <bold>alt-6</>{reset} for PR details, Enter to fetch & switch\n"
    )
}

/// Place the PR's cells on the worktree rows' grid so every column lines up:
/// a dim `#` gutter sigil (see `PR_GUTTER_SIGIL`), the head branch in the
/// Branch column, and the number.
///
/// The number is the row's identity, so it always shows. With a CI column it
/// rides there, colored by CI + review state like worktree rows' PR numbers
/// (`format_cell`). The picker only allocates a CI column when some worktree
/// row had a cached status, so without one — a repo with no CI cache — the
/// number falls back to a dim reference just after the Branch column rather
/// than hiding.
///
/// The PR title and author are NOT on the row — they have no worktree-column
/// equivalent, so showing them would either misalign PR rows against worktree
/// rows or overrun the status columns. They live in the `pr` preview tab
/// instead (see `PrSkimItem::pr_pane`); the title still feeds `search_text`, so
/// fuzzy matching on it works even though it isn't displayed. The other
/// worktree-data columns PR rows leave blank (status, diffs, URL, age) are a
/// follow-up — see TODO(pr-row-columns).
fn render_grid_row(entry: &PrEntry, grid: &ColumnGrid, list_width: usize) -> String {
    let dim = Style::new().dimmed();
    let mut segments: Vec<(usize, StyledLine)> = Vec::new();

    if let Some(col) = grid.column(ColumnKind::Gutter) {
        let mut cell = StyledLine::new();
        cell.push_styled(PR_GUTTER_SIGIL, dim);
        segments.push((col.start, cell));
    }

    let branch_col = grid.column(ColumnKind::Branch);
    if let Some(col) = branch_col {
        let mut cell = StyledLine::new();
        cell.push_raw(entry.head_branch.clone());
        segments.push((col.start, cell.truncate_to_width(col.width)));
    }

    match grid.column(ColumnKind::CiStatus).zip(entry.status.as_ref()) {
        Some((col, status)) => {
            let mut cell = StyledLine::new();
            cell.push_raw(status.format_cell(col.width, false));
            segments.push((col.start, cell));
        }
        None => {
            // No CI column — show the dim reference after the branch so the
            // number never hides.
            let start = branch_col.map_or(0, |col| col.start + col.width + 2);
            let mut cell = StyledLine::new();
            cell.push_styled(entry.pr_ref().to_string(), dim);
            segments.push((
                start,
                cell.truncate_to_width(list_width.saturating_sub(start)),
            ));
        }
    }

    // All cells sit in fixed columns within the pane (branch truncates to its
    // column, the number is fixed-width), so the row can't overflow.
    segments.sort_by_key(|(start, _)| *start);
    let mut line = StyledLine::new();
    for (start, cell) in segments {
        line.pad_to(start);
        line.extend(cell);
    }
    line.render()
}

/// Freeform row for when no grid is available: `#N  branch`. Like the grid row,
/// the title and author live only in the preview; the `pr_ref` already carries
/// the `#`/`!` sigil, so no separate gutter marker is needed.
fn render_freeform_row(entry: &PrEntry, list_width: usize) -> String {
    let pr_ref = entry.pr_ref();
    let prefix_plain = format!("{pr_ref}  ");
    let branch_budget = list_width.saturating_sub(prefix_plain.width()).max(8);
    let head_branch = crate::display::truncate_to_width(&entry.head_branch, branch_budget);
    cformat!("<bold>{pr_ref}</>  <cyan>{head_branch}</>")
}

// TODO(pr-row-columns): fill the worktree-data columns PR rows leave blank.
// The status, branch-diff, and age columns have no PR equivalent in the
// `gh pr list` / `glab mr list` payload today, but some map cleanly: the PR's
// CI conclusion could drive a status glyph, and the PR's age its own column.
// Each needs a field added to the row-list `--json` (cheap, one call) or a
// derived value — keep them off the flexible region so the grid stays aligned.
//
// TODO(pr-preview-log): give the `log` tab (and ideally `summary`) content on
// `--prs` rows. The body/description rides the one `gh pr list` call cheaply,
// but a commit log does not: for a checked-out branch it's a local `git log`,
// but a `--prs` head branch isn't fetched, so the commits need either a fetch
// or `gh pr view <n> --json commits` / `glab mr view`. That payload is too
// heavy to fold into the row-list call (commits for ~50 PRs), so it must load
// in the background per row — which `--prs` rows don't do yet: today the whole
// row, `pr_pane` included, is built once when the list call returns. The
// mechanism would mirror the worktree rows' `PreviewOrchestrator` cache: a
// shared map the `preview()` callback reads, populated off-thread, with a
// "loading…" placeholder on a miss (skim re-queries on selection/tab change).
// Remote-branch rows (`--remotes`) are the cheap half — their commits are
// already fetched, so their `log` tab is a plain local `git log`.
//
// TODO(pr-preview-comments): add a `7: comments` tab showing the PR/MR
// discussion, rendered with the house gutter per comment (author + body, like
// `render_pr_description`). Needs the same background per-row fetch as the log
// (`gh pr view <n> --json comments`), and a 7th tab widens the preview tab bar
// — already ~63 cols with six numbered tabs — so it clips sooner on narrow
// (≤~125-col, Right-layout) previews. Weigh an abbreviated/scrolling tab bar
// alongside it.
impl SkimItem for PrSkimItem {
    fn text(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.search_text)
    }

    fn display(&self, _context: DisplayContext) -> Line<'_> {
        ansi_to_line(&self.rendered)
    }

    fn output(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.output_token)
    }

    fn preview(&self, _context: PreviewContext<'_>) -> ItemPreview {
        // Share the worktree rows' tab bar. A `--prs` row has content only on
        // the `pr` tab (tabs 1-5 empty → de-emphasized); the active tab is the
        // same global digit, so an empty tab shows the placeholder until the
        // user presses alt-6 / Tab.
        let mode = PreviewStateData::read_mode();
        let mut result = render_preview_tabs(mode, TabAvailability::pull_request());
        if mode == PreviewMode::Pr {
            result.push_str(&self.pr_pane);
        } else {
            result.push_str(&pr_row_empty_placeholder());
        }
        ItemPreview::AnsiText(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: RefKind, number: u32, title: &str) -> PrEntry {
        let number_ref = match kind {
            RefKind::Pr => PrRef::pr(u64::from(number)),
            RefKind::Mr => PrRef::mr(u64::from(number)),
        };
        PrEntry {
            number,
            title: title.to_string(),
            head_branch: "feature/auth".to_string(),
            author: "alice".to_string(),
            is_draft: false,
            url: Some("https://github.com/owner/repo/pull/123".to_string()),
            kind,
            body: String::new(),
            status: Some(PrStatus {
                ci_status: CiStatus::Passed,
                source: CiSource::PullRequest,
                is_stale: false,
                is_priming: false,
                url: None,
                number: Some(number_ref),
                review_state: None,
            }),
        }
    }

    /// Grid that includes a CI column (the picker's layout once CiStatus is no
    /// longer skipped). Gutter 0–2, Branch 2–22, Status 24–32, CI 34–40.
    fn grid_with_ci() -> ColumnGrid {
        ColumnGrid {
            columns: vec![
                grid_col(ColumnKind::Gutter, 0, 2),
                grid_col(ColumnKind::Branch, 2, 20),
                grid_col(ColumnKind::Status, 24, 8),
                grid_col(ColumnKind::CiStatus, 34, 6),
            ],
        }
    }

    #[test]
    fn output_token_is_the_switch_shortcut() {
        let pr = PrSkimItem::new(entry(RefKind::Pr, 123, "Fix the flaky test"), 120, None);
        assert_eq!(pr.output(), "pr:123");

        let mr = PrSkimItem::new(entry(RefKind::Mr, 7, "Add caching"), 120, None);
        assert_eq!(mr.output(), "mr:7");
    }

    #[test]
    fn search_text_covers_number_title_branch_author() {
        let pr = PrSkimItem::new(entry(RefKind::Pr, 42, "Speed up startup"), 120, None);
        let text = pr.text();
        assert!(text.contains("42"));
        assert!(text.contains("Speed up startup"));
        assert!(text.contains("feature/auth"));
        assert!(text.contains("alice"));
        // Gutter glyph folded in so `#` filters to PR/MR rows.
        assert!(text.trim_end().ends_with('#'), "gutter glyph: {text:?}");
    }

    /// `#` is a plain literal under skim's default engine (unlike `^`/`|`), so
    /// it filters to PR/MR rows and leaves worktree/branch rows out. Companion
    /// to `progressive_handler`'s `gutter_glyphs_filter_under_skims_default_engine`.
    #[test]
    fn hash_sigil_filters_to_pr_rows_under_skims_default_engine() {
        struct TextItem(String);
        impl SkimItem for TextItem {
            fn text(&self) -> Cow<'_, str> {
                Cow::Borrowed(&self.0)
            }
        }
        let matches = |haystack: &dyn SkimItem| -> bool {
            AndOrEngineFactory::new(ExactOrFuzzyEngineFactory::builder().build())
                .create_engine("#")
                .match_item(haystack)
                .is_some()
        };

        let pr = PrSkimItem::new(entry(RefKind::Pr, 42, "Speed up startup"), 120, None);
        assert!(matches(&pr), "# selects the PR row");
        // A worktree-style row carries no `#` in its folded search_text.
        assert!(
            !matches(&TextItem("cur /tmp/cur @".into())),
            "# must not match a worktree row"
        );
    }

    #[test]
    fn freeform_row_shows_reference_and_branch_only() {
        // No grid: the freeform fallback shows reference and branch. The title
        // and author have no column, so they stay off the row — but the title
        // still feeds `search_text`.
        let pr = PrSkimItem::new(entry(RefKind::Pr, 1, "Retry the flaky test"), 80, None);
        let row = plain(&pr.rendered);
        assert!(row.contains("feature/auth"), "branch on the row: {row:?}");
        assert!(row.contains("#1"), "reference on the row: {row:?}");
        assert!(
            !row.contains("Retry the flaky test"),
            "title stays off the row: {row:?}"
        );
        assert!(!row.contains("@alice"), "author stays off the row: {row:?}");
        assert!(
            pr.text().contains("Retry the flaky test"),
            "title searchable"
        );
    }

    #[test]
    fn freeform_row_truncates_a_long_branch_to_the_pane() {
        // The branch absorbs the remaining width after the reference and
        // truncates so the row stays inside the pane.
        let mut e = entry(RefKind::Pr, 1, "Title");
        e.head_branch = "a-very-long-branch-name-that-would-otherwise-overflow".to_string();
        let pr = PrSkimItem::new(e, 40, None);
        let row = plain(&pr.rendered);
        assert!(row.contains("#1"), "reference survives: {row:?}");
        assert!(row.contains('…'), "branch truncated: {row:?}");
        assert!(row.width() <= 40, "row within pane: {row:?}");
    }

    #[test]
    fn draft_prs_are_flagged_in_the_preview() {
        // The row carries no title or inline flag, so a draft shows in the pr
        // pane (and, with a CI column, as the dimmed number — see
        // `grid_row_with_ci_dims_drafts_instead_of_flagging_them`).
        let mut e = entry(RefKind::Pr, 9, "WIP refactor");
        e.is_draft = true;
        let pr = PrSkimItem::new(e, 120, None);
        assert!(pr.pr_pane.contains("draft"));
    }

    use super::super::super::list::layout::GridColumn;

    fn grid_col(kind: ColumnKind, start: usize, width: usize) -> GridColumn {
        GridColumn { kind, start, width }
    }

    /// Gutter 0–2, Branch 2–22, Status 24–32, Summary 34–64, Message 66–96 —
    /// the shape `calculate_layout_with_width` produces for the picker.
    fn grid() -> ColumnGrid {
        ColumnGrid {
            columns: vec![
                grid_col(ColumnKind::Gutter, 0, 2),
                grid_col(ColumnKind::Branch, 2, 20),
                grid_col(ColumnKind::Status, 24, 8),
                grid_col(ColumnKind::Summary, 34, 30),
                grid_col(ColumnKind::Message, 66, 30),
            ],
        }
    }

    fn plain(rendered: &str) -> String {
        use ansi_str::AnsiStr;
        rendered.ansi_strip().to_string()
    }

    /// Display column where `needle` starts (unicode-width-aware, so an
    /// earlier multi-byte ellipsis doesn't skew the position).
    fn display_col(text: &str, needle: &str) -> usize {
        let byte_idx = text
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} not found in {text:?}"));
        text[..byte_idx].width()
    }

    #[test]
    fn grid_row_places_cells_on_layout_columns() {
        // grid() has no CI column, so the number falls back to just after the
        // Branch column (which ends at 22, so the reference lands at 24). The
        // title and author stay off the row.
        let pr = PrSkimItem::new(
            entry(RefKind::Pr, 123, "Fix the flaky test"),
            120,
            Some(&grid()),
        );
        let text = plain(&pr.rendered);
        // The dim `#` gutter sigil sits at column 0; branch in the Branch column.
        assert!(text.starts_with("# feature/auth"));
        assert_eq!(display_col(&text, "feature/auth"), 2, "branch column");
        assert_eq!(
            display_col(&text, "#123"),
            24,
            "number falls back to after the branch"
        );
        assert!(
            !text.contains("Fix the flaky test"),
            "no title on the row: {text:?}"
        );
        assert!(!text.contains("@alice"), "no author on the row: {text:?}");
    }

    #[test]
    fn grid_row_truncates_long_branch_to_its_column() {
        let mut e = entry(RefKind::Pr, 5, "Title");
        e.head_branch = "a-very-long-branch-name-overflowing".to_string();
        let pr = PrSkimItem::new(e, 120, Some(&grid_with_ci()));
        let text = plain(&pr.rendered);
        // The branch is shortened to its column; the number still lands in CI.
        assert!(text.contains('…'));
        assert_eq!(display_col(&text, "#5"), 34, "number in CI column");
    }

    #[test]
    fn rows_use_the_forge_sigil_for_the_reference() {
        // GitLab MRs render `!N`, not `#N` — matching `PrRef` everywhere else
        // (the CI column, `wt list`). The CI-column number, freeform row, and
        // preview all derive the sigil from `PrEntry::pr_ref`.
        let mr = PrSkimItem::new(
            entry(RefKind::Mr, 42, "Add caching"),
            120,
            Some(&grid_with_ci()),
        );
        let row = plain(&mr.rendered);
        assert!(row.contains("!42"), "grid row uses ! for MRs: {row:?}");
        assert!(
            !row.contains("#42"),
            "grid row must not use # for MRs: {row:?}"
        );
        assert!(mr.pr_pane.contains("!42"), "preview uses ! for MRs");

        let mr_freeform = PrSkimItem::new(entry(RefKind::Mr, 42, "Add caching"), 120, None);
        assert!(
            plain(&mr_freeform.rendered).contains("!42"),
            "freeform row uses !"
        );

        // GitHub PRs keep `#N`.
        let pr = PrSkimItem::new(
            entry(RefKind::Pr, 42, "Add caching"),
            120,
            Some(&grid_with_ci()),
        );
        assert!(
            plain(&pr.rendered).contains("#42"),
            "grid row uses # for PRs"
        );
    }

    #[test]
    fn grid_row_stays_within_the_list_pane() {
        // Even with a long branch the row stays inside the pane: the branch
        // truncates to its column and nothing flexible follows.
        let no_flexible = ColumnGrid {
            columns: vec![
                grid_col(ColumnKind::Gutter, 0, 2),
                grid_col(ColumnKind::Branch, 2, 20),
            ],
        };
        let mut e = entry(RefKind::Pr, 1, "Title");
        e.head_branch = "a-very-long-branch-name-that-runs-past-the-edge".to_string();
        let pr = PrSkimItem::new(e, 60, Some(&no_flexible));
        let text = plain(&pr.rendered);
        assert!(text.width() <= 60);
        // Skim's overflow check uses CJK widths, where the truncation `…`
        // counts as 2 — the row must pass it too or skim repaints the last
        // two columns as `..`.
        assert!(text.width_cjk() <= 60);
    }

    #[test]
    fn grid_row_places_the_number_in_the_ci_column() {
        let pr = PrSkimItem::new(
            entry(RefKind::Pr, 123, "Fix the flaky test"),
            120,
            Some(&grid_with_ci()),
        );
        let text = plain(&pr.rendered);
        // The number sits in the CI column (start 34), aligned with worktree
        // rows; the title is not on the row at all.
        assert_eq!(display_col(&text, "#123"), 34, "number in CI column");
        assert!(!text.contains("Fix"), "title stays off the row: {text:?}");
    }

    #[test]
    fn grid_row_omits_the_title_even_with_a_message_column() {
        // The title stays off the row regardless of layout — even when a
        // Message column (where worktree rows show their commit subject) is
        // present, only the gutter, branch, and CI number land.
        let grid = ColumnGrid {
            columns: vec![
                grid_col(ColumnKind::Gutter, 0, 2),
                grid_col(ColumnKind::Branch, 2, 20),
                grid_col(ColumnKind::CiStatus, 24, 6),
                grid_col(ColumnKind::Message, 32, 40),
            ],
        };
        let pr = PrSkimItem::new(
            entry(RefKind::Pr, 42, "Retry the flaky test"),
            120,
            Some(&grid),
        );
        let text = plain(&pr.rendered);
        assert_eq!(display_col(&text, "feature/auth"), 2, "branch");
        assert_eq!(display_col(&text, "#42"), 24, "number in CI column");
        assert!(
            !text.contains("Retry the flaky test"),
            "title stays off the row: {text:?}"
        );
    }

    #[test]
    fn grid_row_dims_drafts_via_the_ci_number() {
        // A draft shows only as the dimmed number in the CI column (review
        // state Draft) — never as a "draft" word on the row.
        let mut e = entry(RefKind::Pr, 9, "WIP");
        e.is_draft = true;
        if let Some(status) = e.status.as_mut() {
            status.review_state = Some(ReviewState::Draft);
        }
        let pr = PrSkimItem::new(e, 120, Some(&grid_with_ci()));
        let text = plain(&pr.rendered);
        assert!(
            !text.contains("draft"),
            "no draft flag on the row: {text:?}"
        );
        assert_eq!(display_col(&text, "#9"), 34, "number still in CI column");
    }

    #[test]
    fn parse_github_builds_ci_and_review_status() {
        // statusCheckRollup → CI status; reviewDecision → review state; both
        // ride the single `gh pr list` call.
        let json = br#"[
          {"number":10,"title":"t","headRefName":"b","statusCheckRollup":[{"status":"COMPLETED","conclusion":"SUCCESS"}],"reviewDecision":"APPROVED"}
        ]"#;
        let entries = parse_github_prs(json).unwrap();
        let status = entries[0].status.as_ref().expect("status built");
        assert_eq!(status.ci_status, CiStatus::Passed);
        assert_eq!(status.review_state, Some(ReviewState::Approved));
        assert_eq!(status.number.map(|r| r.to_string()).as_deref(), Some("#10"));
    }

    #[test]
    fn parse_gitlab_builds_coarse_status_from_the_list_call() {
        // The single `glab mr list` call carries draft + detailed_merge_status,
        // not pipeline detail: draft dims, conflict reports Conflicts.
        let json = br#"[
          {"iid":3,"title":"t","source_branch":"b","draft":true,"detailed_merge_status":"conflict"}
        ]"#;
        let entries = parse_gitlab_mrs(json).unwrap();
        let status = entries[0].status.as_ref().expect("status built");
        assert_eq!(status.ci_status, CiStatus::Conflicts);
        assert_eq!(status.review_state, Some(ReviewState::Draft));
        assert_eq!(status.number.map(|r| r.to_string()).as_deref(), Some("!3"));
    }

    #[test]
    fn parse_gitlab_running_pipeline_and_pending_review() {
        // `ci_still_running` → Running CI; a non-draft `not_approved` MR maps to
        // a pending review (draft would outrank it).
        let json = br#"[
          {"iid":4,"title":"t","source_branch":"b","draft":false,"detailed_merge_status":"ci_still_running"},
          {"iid":5,"title":"t","source_branch":"b","draft":false,"detailed_merge_status":"not_approved"}
        ]"#;
        let entries = parse_gitlab_mrs(json).unwrap();
        assert_eq!(
            entries[0].status.as_ref().unwrap().ci_status,
            CiStatus::Running
        );
        assert_eq!(
            entries[1].status.as_ref().unwrap().review_state,
            Some(ReviewState::Pending)
        );
    }

    #[test]
    fn parse_github_maps_fields_including_fork_author_and_draft() {
        // Two PRs: one ready from a fork, one draft. Mirrors the
        // `gh pr list --json number,title,headRefName,author,isDraft,url` shape.
        let json = br#"[
          {"number":2964,"title":"ci: freshen","headRefName":"fix/ci","author":{"login":"octocat"},"isDraft":false,"url":"https://github.com/o/r/pull/2964","body":"Bumps the CI image and re-pins actions."},
          {"number":2969,"title":"wip","headRefName":"wip-branch","author":{"login":"forkuser"},"isDraft":true,"url":"https://github.com/o/r/pull/2969"}
        ]"#;
        let entries = parse_github_prs(json).unwrap();
        assert_eq!(entries.len(), 2);

        assert_eq!(entries[0].number, 2964);
        assert_eq!(entries[0].title, "ci: freshen");
        assert_eq!(entries[0].head_branch, "fix/ci");
        assert_eq!(entries[0].author, "octocat");
        assert!(!entries[0].is_draft);
        assert!(matches!(entries[0].kind, RefKind::Pr));
        assert_eq!(entries[0].body, "Bumps the CI image and re-pins actions.");

        assert_eq!(entries[1].number, 2969);
        assert!(entries[1].is_draft);
        assert_eq!(entries[1].author, "forkuser");
        // Absent `body` defaults to empty — the description block is skipped.
        assert_eq!(entries[1].body, "");
    }

    #[test]
    fn parse_github_tolerates_missing_optional_fields() {
        // `author` can be absent (ghost user / deleted account); `url` and
        // `isDraft` default. The row must still parse.
        let json = br#"[{"number":1,"title":"t","headRefName":"b"}]"#;
        let entries = parse_github_prs(json).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].author, "");
        assert!(entries[0].url.is_none());
        assert!(!entries[0].is_draft);
    }

    #[test]
    fn parse_github_empty_list_is_empty() {
        assert!(parse_github_prs(b"[]").unwrap().is_empty());
    }

    #[test]
    fn fetch_open_prs_bails_without_a_forge_remote() {
        // A repo with no remote can't be classified as GitHub or GitLab, so
        // `--prs` reports that instead of shelling out to gh/glab.
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let err = match fetch_open_prs(&test.repo) {
            Ok(_) => panic!("expected --prs to bail without a forge remote"),
            Err(e) => e,
        };
        assert!(
            err.to_string().contains("could not determine the forge"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn fetch_open_prs_bails_on_an_unsupported_forge() {
        // A recognized-but-unsupported forge (here Gitea, by its host) reports
        // the GitHub/GitLab limitation rather than shelling out.
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        test.run_git(&["remote", "add", "origin", "https://gitea.com/o/r.git"]);
        let err = match fetch_open_prs(&test.repo) {
            Ok(_) => panic!("expected --prs to bail on an unsupported forge"),
            Err(e) => e,
        };
        assert!(
            err.to_string().contains("supports GitHub and GitLab"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn stream_open_prs_clears_loading_and_pokes_render_on_bail() {
        // No forge remote → the fetch bails, but the wrapper still drops the
        // header's loading marker and wakes skim. Without the poke the marker
        // would linger past the (failed) fetch until the next keystroke.
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let (tx, _rx): (SkimItemSender, SkimItemReceiver) = unbounded();
        let warnings = Mutex::new(Vec::new());
        let grid = GridSlot::new();
        let loading = AtomicBool::new(true);
        let (rtx, mut rrx) = tokio::sync::mpsc::channel(8);
        let render_tx = OnceLock::new();
        render_tx.set(rtx).unwrap();

        stream_open_prs(&test.repo, 80, &tx, &warnings, &grid, &loading, &render_tx);

        assert!(!loading.load(Ordering::Relaxed), "loading flag cleared");
        assert!(matches!(rrx.try_recv(), Ok(Event::Render)), "render poked");
    }

    #[test]
    fn parse_gitlab_maps_iid_source_branch_and_username() {
        // `glab mr list --output json`: iid (not number), source_branch,
        // author.username, draft, web_url.
        let json = br#"[
          {"iid":7,"title":"Add caching","source_branch":"feat/cache","author":{"username":"alice"},"draft":false,"web_url":"https://gitlab.com/o/r/-/merge_requests/7","description":"Caches the dependency graph between jobs."},
          {"iid":8,"title":"WIP","source_branch":"wip","author":{"username":"bob"},"draft":true,"web_url":"https://gitlab.com/o/r/-/merge_requests/8"}
        ]"#;
        let entries = parse_gitlab_mrs(json).unwrap();
        assert_eq!(entries.len(), 2);

        assert_eq!(entries[0].number, 7);
        assert_eq!(entries[0].head_branch, "feat/cache");
        assert_eq!(entries[0].author, "alice");
        assert!(matches!(entries[0].kind, RefKind::Mr));
        assert_eq!(entries[0].body, "Caches the dependency graph between jobs.");
        // GitLab's `description` maps to the same `body` slot; absent → empty.
        assert_eq!(entries[1].body, "");
        // The MR's `output()` shortcut uses the iid.
        assert_eq!(
            PrSkimItem::new(entries.into_iter().next().unwrap(), 120, None).output(),
            "mr:7"
        );
    }

    #[test]
    fn parse_invalid_json_errors() {
        assert!(parse_github_prs(b"not json").is_err());
        assert!(parse_gitlab_mrs(b"not json").is_err());
    }

    #[test]
    fn description_empty_or_blank_renders_nothing() {
        // No body, or whitespace-only — the block is skipped entirely so the
        // pane doesn't show an empty gutter.
        assert_eq!(render_pr_description("", 80), "");
        assert_eq!(render_pr_description("   \n\t \n", 80), "");
    }

    #[test]
    fn description_wraps_into_the_house_gutter() {
        let out = render_pr_description("Fixes the flaky retry logic.", 80);
        // Leading full reset clears inherited style; the house gutter sets a
        // bg color and closes each line with a skim-safe `\x1b[0m`.
        assert!(out.starts_with("\n\x1b[0m"), "leading reset: {out:?}");
        assert!(out.contains("\x1b[107m"), "house gutter bg: {out:?}");
        assert!(
            out.contains("Fixes the flaky retry logic."),
            "body: {out:?}"
        );
    }

    #[test]
    fn description_renders_the_whole_body() {
        // One word per line so each survives as its own gutter line; the pane
        // scrolls, so the full body renders with no truncation hint.
        let body = (0..50)
            .map(|i| format!("- word{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let out = render_pr_description(&body, 80);
        assert!(!out.contains("more line"), "no truncation hint: {out:?}");
        assert!(out.contains("word0"), "head kept: {out:?}");
        assert!(out.contains("word49"), "tail kept: {out:?}");
    }

    #[test]
    fn description_renders_markdown() {
        // Markdown is styled, not shown verbatim: a bold span carries the SGR-1
        // termimad emits, and the literal `**` markers are gone.
        let out = render_pr_description("Fixes the **flaky** retry.", 80);
        assert!(out.contains("\x1b[1m"), "bold rendered: {out:?}");
        assert!(!out.contains("**"), "markers consumed: {out:?}");
    }

    #[test]
    fn pr_pane_shows_description_only_when_present() {
        let mut with_body = entry(RefKind::Pr, 1, "t");
        with_body.body = "A short summary of the change.".to_string();
        let pr = PrSkimItem::new(with_body, 120, Some(&grid()));
        assert!(pr.pr_pane.contains("A short summary of the change."));
        assert!(pr.pr_pane.contains("\x1b[107m"), "gutter present");

        // The base fixture has an empty body — no gutter, no description.
        let plain_pr = PrSkimItem::new(entry(RefKind::Pr, 2, "t"), 120, Some(&grid()));
        assert!(
            !plain_pr.pr_pane.contains("\x1b[107m"),
            "no gutter when empty"
        );
    }

    #[test]
    fn preview_renders_tabs_and_placeholder_off_the_pr_tab() {
        // With no per-process preview-state file, `read_mode()` returns the
        // default (WorkingTree) — empty on a --prs row — so `preview()` renders
        // the shared tab bar plus the "not checked out" placeholder. Drives the
        // real `SkimItem::preview` (the `--prs` streaming path is too async to
        // exercise it reliably under a PTY); `PreviewContext` is ignored by the
        // impl, so a minimal one suffices.
        let pr = PrSkimItem::new(entry(RefKind::Pr, 7, "Title"), 120, Some(&grid()));
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
        let ItemPreview::AnsiText(text) = pr.preview(ctx) else {
            panic!("expected AnsiText preview");
        };
        assert!(text.contains("6: pr"), "tab bar present: {text:?}");
        assert!(
            text.contains("Not checked out locally"),
            "placeholder present: {text:?}"
        );
    }
}
