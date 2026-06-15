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
//! heartbeat contract — see [`super::handle_picker`].
//!
//! # Alignment
//!
//! PR rows render on the same column grid as the worktree rows: the head
//! branch in the Branch column, `#N title  @author` in the flexible text
//! region (see [`render_grid_row`] for which column that is), blanks under
//! the status/diff columns. The geometry
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
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use anstyle::{AnsiColor, Color, Style};
use anyhow::Context;
use color_print::cformat;
use serde::Deserialize;
use skim::prelude::*;
use unicode_width::UnicodeWidthStr;
use worktrunk::git::{CiPlatform, Repository};
use worktrunk::styling::{StyledLine, warning_message};

use super::super::list::ci_status::{
    CiSource, CiStatus, GitHubPrInfo, PrRef, PrStatus, ReviewState, non_interactive_cmd,
    tool_available,
};
use super::super::list::columns::ColumnKind;
use super::super::list::layout::ColumnGrid;

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

/// Fetch open PRs/MRs, build picker rows, and stream them into skim.
///
/// On failure (forge unsupported, CLI missing/unauthenticated, network error)
/// the reason is stashed for display after skim releases the terminal — the
/// picker stays usable with its worktree rows.
pub(super) fn stream_open_prs(
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

    for entry in entries {
        let _ = tx.send(Arc::new(PrSkimItem::new(entry, list_width, grid.as_ref())));
    }
}

/// Plural noun for the forge's change-request — "PRs" on GitHub, "MRs" on
/// GitLab. Used for the empty-list message, where there's no entry to read
/// the kind from.
fn forge_noun(repo: &Repository) -> &'static str {
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
            // CI/review fields ride the one call; no extra round-trip.
            "number,title,headRefName,author,isDraft,url,statusCheckRollup,reviewDecision,mergeStateStatus",
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
    /// Static info pane (the head branch isn't local yet, so there's no diff
    /// to preview — show metadata and the web URL instead).
    preview_text: String,
}

impl PrSkimItem {
    fn new(entry: PrEntry, list_width: usize, grid: Option<&ColumnGrid>) -> Self {
        let label = entry.kind.shortcut();
        let output_token = format!("{label}:{}", entry.number);

        let search_text = format!(
            "{label} {} {} {} {}",
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
            ..
        } = entry;
        let mut preview_text = cformat!(
            "<bold>{pr_ref}</>  {title}\n\n<dim>branch</>   {head_branch}\n<dim>author</>   @{author}\n"
        );
        if is_draft {
            preview_text.push_str(&cformat!("<dim>state</>    <yellow>draft</>\n"));
        }
        if let Some(url) = url {
            preview_text.push_str(&cformat!("<dim>url</>      {url}\n"));
        }
        preview_text.push_str(&cformat!(
            "\n<dim>Enter: fetch & switch to this branch ({output_token})</>\n"
        ));

        Self {
            search_text,
            rendered,
            output_token,
            preview_text,
        }
    }
}

/// Place the PR's cells on the worktree rows' grid: head branch in the
/// Branch column, the number in the CI column (colored by CI + review state,
/// like worktree rows), the title in the flexible text region, author in the
/// Message column. The gutter is blank like a branch-only row's.
///
/// The number lives in the CI column when the grid has one and a status was
/// fetched — aligning with the worktree rows' PR numbers. Without a CI column
/// (narrow layouts) it falls back to a dim `#N` prefix on the title.
///
/// The Summary column only exists when `[list] summary` is enabled, and
/// Message only on wide layouts — without either there is no flexible text
/// column, so the title runs from the first column after Branch up to the CI
/// column (or the pane edge). The worktree-data columns it underlaps (status,
/// diffs, URL, age) are blank on PR rows, so nothing collides.
fn render_grid_row(entry: &PrEntry, grid: &ColumnGrid, list_width: usize) -> String {
    let yellow = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow)));
    let dim = Style::new().dimmed();

    let branch_col = grid.column(ColumnKind::Branch);
    let summary_col = grid.column(ColumnKind::Summary);
    let message_col = grid.column(ColumnKind::Message);

    // The number rides in the CI column — the same cell worktree rows show
    // their PR number in. Falls back to a `#N` title prefix without one.
    let number_cell = grid.column(ColumnKind::CiStatus).zip(entry.status.as_ref());

    let (title_start, title_width) = match (summary_col, message_col) {
        // Confined to Summary so the columns after it stay on grid.
        (Some(col), _) => (col.start, col.width),
        // Message is the last column; running to the pane edge is safe.
        (None, Some(col)) => (col.start, list_width.saturating_sub(col.start)),
        (None, None) => {
            let start = branch_col.map_or(0, |col| col.start + col.width + 2);
            // Stop before the CI column so the title never overruns the number.
            let end = number_cell
                .map(|(col, _)| col.start.saturating_sub(1))
                .unwrap_or(list_width);
            (start, end.saturating_sub(start))
        }
    };

    // The author rides in the Message column when a Summary column already
    // claimed the title; otherwise it trails the title.
    let author_col = summary_col.and(message_col);

    // Assemble the row for a given title width. Cells are emitted left-to-right
    // by column start (`pad_to` only moves forward, and the CI column can sit
    // either side of the title). Built as a closure so the title — the one
    // flexible cell — can be re-truncated to satisfy skim's overflow check
    // without disturbing the fixed branch and CI-number cells.
    let assemble = |title_w: usize| -> StyledLine {
        let mut segments: Vec<(usize, StyledLine)> = Vec::new();

        if let Some(col) = branch_col {
            let mut cell = StyledLine::new();
            cell.push_raw(entry.head_branch.clone());
            segments.push((col.start, cell.truncate_to_width(col.width)));
        }

        let mut title = StyledLine::new();
        if number_cell.is_none() {
            title.push_styled(entry.pr_ref().to_string(), dim);
            title.push_raw(" ");
            if entry.is_draft {
                title.push_styled("draft ", yellow);
            }
        }
        title.push_raw(entry.title.clone());
        if author_col.is_none() && !entry.author.is_empty() {
            title.push_styled(format!("  @{}", entry.author), dim);
        }
        segments.push((title_start, title.truncate_to_width(title_w)));

        if let Some((col, status)) = number_cell {
            let mut cell = StyledLine::new();
            cell.push_raw(status.format_cell(col.width, false));
            segments.push((col.start, cell));
        }

        if let Some(col) = author_col
            && !entry.author.is_empty()
        {
            let mut cell = StyledLine::new();
            cell.push_styled(format!("@{}", entry.author), dim);
            segments.push((col.start, cell.truncate_to_width(col.width)));
        }

        segments.sort_by_key(|(start, _)| *start);
        let mut line = StyledLine::new();
        for (start, cell) in segments {
            line.pad_to(start);
            line.extend(cell);
        }
        line
    };

    let mut line = assemble(title_width);

    // Skim's overflow check measures the line with `width_cjk`, counting
    // ambiguous-width characters (the `…` our truncation adds, or the status
    // arrows) as 2 columns, while terminals — and the column math above —
    // render them as 1. A row that overflows there gets its last two columns
    // repainted as `..`.
    //
    // Only one case is fixable here. When the title is the rightmost cell (no
    // CI column), shrink it until the row passes — each pass removes at least
    // one column. When the number is in the CI column it anchors the right
    // edge: trimming the title only opens blank space upstream and can't shrink
    // the line, and the number itself can't be trimmed without mangling it, so
    // at narrow widths such a row may still lose its last two columns to skim's
    // spurious `..` — the same width_cjk bug worktree rows hit, accepted as a
    // known limitation.
    //
    // TODO(vendor-skim): a one-word fix in skim's `draw_item` removes the check
    // entirely (and with it the CI-column clip). See `vendor/NOTES.md` →
    // "width_cjk vs width mismatch".
    if number_cell.is_none() {
        let mut title_w = title_width;
        while title_w > 0 && line.width_cjk() > list_width {
            let excess = line.width_cjk() - list_width;
            title_w = title_w.saturating_sub(excess.max(1));
            line = assemble(title_w);
        }
    }

    line.render()
}

/// Freeform row for when no grid is available:
/// `pr #N  title  branch  @author`.
fn render_freeform_row(entry: &PrEntry, list_width: usize) -> String {
    let label = entry.kind.shortcut();
    let pr_ref = entry.pr_ref();
    let head_branch = &entry.head_branch;
    let author = &entry.author;

    // Truncate the title so the branch and author stay visible. Measure
    // the fixed pieces (plain text) and give the rest to the title.
    let draft_plain = if entry.is_draft { "draft " } else { "" };
    let prefix_plain = format!("{label} {pr_ref}  ");
    let suffix_plain = format!("  {head_branch}  @{author}");
    let fixed = prefix_plain.width() + draft_plain.width() + suffix_plain.width();
    let title_budget = list_width.saturating_sub(fixed).max(8);
    let title = crate::display::truncate_to_width(&entry.title, title_budget);

    let draft = if entry.is_draft {
        cformat!("<yellow>draft</> ")
    } else {
        String::new()
    };
    cformat!(
        "<dim>{label}</> <bold>{pr_ref}</>  {draft}{title}  <cyan>{head_branch}</>  <dim>@{author}</>"
    )
}

impl SkimItem for PrSkimItem {
    fn text(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.search_text)
    }

    fn display<'a>(&'a self, _context: skim::DisplayContext<'a>) -> skim::AnsiString<'a> {
        skim::AnsiString::parse(&self.rendered)
    }

    fn output(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.output_token)
    }

    fn preview(&self, _context: PreviewContext<'_>) -> ItemPreview {
        ItemPreview::AnsiText(self.preview_text.clone())
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
            status: Some(PrStatus {
                ci_status: CiStatus::Passed,
                source: CiSource::PullRequest,
                is_stale: false,
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
    }

    #[test]
    fn long_title_is_truncated_to_fit_narrow_lists() {
        let long = "A very long pull request title that would otherwise overflow the list pane and push the branch and author columns off the screen entirely";
        let pr = PrSkimItem::new(entry(RefKind::Pr, 1, long), 60, None);
        // Branch and author survive truncation; the title is shortened.
        assert!(pr.rendered.contains("feature/auth"));
        assert!(pr.rendered.contains("@alice"));
        assert!(pr.rendered.contains('…'));
    }

    #[test]
    fn draft_prs_are_flagged() {
        let mut e = entry(RefKind::Pr, 9, "WIP refactor");
        e.is_draft = true;
        let pr = PrSkimItem::new(e, 120, None);
        assert!(pr.rendered.contains("draft"));
        assert!(pr.preview_text.contains("draft"));
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
        let pr = PrSkimItem::new(
            entry(RefKind::Pr, 123, "Fix the flaky test"),
            120,
            Some(&grid()),
        );
        let text = plain(&pr.rendered);
        assert_eq!(display_col(&text, "feature/auth"), 2, "branch column");
        assert_eq!(
            display_col(&text, "#123 Fix the flaky test"),
            34,
            "summary column"
        );
        assert_eq!(display_col(&text, "@alice"), 66, "message column");
        // Gutter and status/diff columns stay blank.
        assert!(text.starts_with("  feature/auth"));
    }

    #[test]
    fn grid_row_truncates_long_branch_to_its_column() {
        let mut e = entry(RefKind::Pr, 5, "Title");
        e.head_branch = "a-very-long-branch-name-overflowing".to_string();
        let pr = PrSkimItem::new(e, 120, Some(&grid()));
        let text = plain(&pr.rendered);
        // The branch is shortened so the title still lands on its column.
        assert!(text.contains('…'));
        assert_eq!(display_col(&text, "#5 Title"), 34);
    }

    #[test]
    fn grid_row_truncates_long_title_to_summary_column() {
        let long = "A very long pull request title that overflows the summary column";
        let pr = PrSkimItem::new(entry(RefKind::Pr, 2, long), 120, Some(&grid()));
        let text = plain(&pr.rendered);
        assert!(text.contains('…'));
        // The author still lands on the Message column.
        assert_eq!(display_col(&text, "@alice"), 66);
    }

    #[test]
    fn grid_row_flags_drafts_before_the_title() {
        let mut e = entry(RefKind::Pr, 9, "WIP refactor");
        e.is_draft = true;
        let pr = PrSkimItem::new(e, 120, Some(&grid()));
        assert_eq!(
            display_col(&plain(&pr.rendered), "#9 draft WIP refactor"),
            34
        );
    }

    #[test]
    fn rows_use_the_forge_sigil_for_the_reference() {
        // GitLab MRs render `!N`, not `#N` — matching `PrRef` everywhere else
        // (the CI column, `wt list`). The grid row, freeform row, and preview
        // all derive the sigil from `PrEntry::pr_ref`.
        let mr = PrSkimItem::new(entry(RefKind::Mr, 42, "Add caching"), 120, Some(&grid()));
        let row = plain(&mr.rendered);
        assert!(row.contains("!42"), "grid row uses ! for MRs: {row:?}");
        assert!(
            !row.contains("#42"),
            "grid row must not use # for MRs: {row:?}"
        );
        assert!(mr.preview_text.contains("!42"), "preview uses ! for MRs");

        let mr_freeform = PrSkimItem::new(entry(RefKind::Mr, 42, "Add caching"), 120, None);
        assert!(
            plain(&mr_freeform.rendered).contains("!42"),
            "freeform row uses !"
        );

        // GitHub PRs keep `#N`.
        let pr = PrSkimItem::new(entry(RefKind::Pr, 42, "Add caching"), 120, Some(&grid()));
        assert!(
            plain(&pr.rendered).contains("#42"),
            "grid row uses # for PRs"
        );
    }

    #[test]
    fn grid_row_without_summary_falls_back_to_message_then_after_branch() {
        // Layout that dropped Summary: the title claims Message and the
        // author trails the title instead of getting its own column.
        let message_only = ColumnGrid {
            columns: vec![
                grid_col(ColumnKind::Gutter, 0, 2),
                grid_col(ColumnKind::Branch, 2, 20),
                grid_col(ColumnKind::Message, 24, 30),
            ],
        };
        let pr = PrSkimItem::new(entry(RefKind::Pr, 1, "Title"), 120, Some(&message_only));
        let text = plain(&pr.rendered);
        assert_eq!(display_col(&text, "#1 Title  @alice"), 24);

        // No flexible text column at all (summaries disabled — the default
        // picker layout): the title runs from the column after Branch, not
        // from past the last column (off-pane).
        let no_flexible = ColumnGrid {
            columns: vec![
                grid_col(ColumnKind::Gutter, 0, 2),
                grid_col(ColumnKind::Branch, 2, 20),
                grid_col(ColumnKind::Status, 24, 8),
                grid_col(ColumnKind::Time, 90, 4),
            ],
        };
        let pr = PrSkimItem::new(entry(RefKind::Pr, 1, "Title"), 120, Some(&no_flexible));
        assert_eq!(display_col(&plain(&pr.rendered), "#1 Title  @alice"), 24);
    }

    #[test]
    fn grid_row_stays_within_the_list_pane() {
        // The freeform off-grid run (no Summary/Message) must truncate at
        // the pane edge rather than spill into skim's `..` overflow.
        let no_flexible = ColumnGrid {
            columns: vec![
                grid_col(ColumnKind::Gutter, 0, 2),
                grid_col(ColumnKind::Branch, 2, 20),
            ],
        };
        let long = "A very long pull request title that runs past the edge of a narrow pane";
        let pr = PrSkimItem::new(entry(RefKind::Pr, 1, long), 60, Some(&no_flexible));
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
        // rows — not prefixing the title.
        assert_eq!(display_col(&text, "#123"), 34, "number in CI column");
        // The title starts in the after-branch region with no `#N` prefix.
        assert_eq!(display_col(&text, "Fix"), 24, "title after branch");
        assert!(!text.contains("#123 Fix"), "no #N prefix on the title");
    }

    #[test]
    fn grid_row_with_ci_dims_drafts_instead_of_flagging_them() {
        // With a CI column, draft shows as the dimmed number there (review
        // state Draft), so the title drops the inline "draft" flag.
        let mut e = entry(RefKind::Pr, 9, "WIP");
        e.is_draft = true;
        if let Some(status) = e.status.as_mut() {
            status.review_state = Some(ReviewState::Draft);
        }
        let pr = PrSkimItem::new(e, 120, Some(&grid_with_ci()));
        let text = plain(&pr.rendered);
        assert!(!text.contains("draft"), "no draft flag in title: {text:?}");
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
    fn parse_github_maps_fields_including_fork_author_and_draft() {
        // Two PRs: one ready from a fork, one draft. Mirrors the
        // `gh pr list --json number,title,headRefName,author,isDraft,url` shape.
        let json = br#"[
          {"number":2964,"title":"ci: freshen","headRefName":"fix/ci","author":{"login":"octocat"},"isDraft":false,"url":"https://github.com/o/r/pull/2964"},
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

        assert_eq!(entries[1].number, 2969);
        assert!(entries[1].is_draft);
        assert_eq!(entries[1].author, "forkuser");
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
    fn parse_gitlab_maps_iid_source_branch_and_username() {
        // `glab mr list --output json`: iid (not number), source_branch,
        // author.username, draft, web_url.
        let json = br#"[
          {"iid":7,"title":"Add caching","source_branch":"feat/cache","author":{"username":"alice"},"draft":false,"web_url":"https://gitlab.com/o/r/-/merge_requests/7"},
          {"iid":8,"title":"WIP","source_branch":"wip","author":{"username":"bob"},"draft":true,"web_url":"https://gitlab.com/o/r/-/merge_requests/8"}
        ]"#;
        let entries = parse_gitlab_mrs(json).unwrap();
        assert_eq!(entries.len(), 2);

        assert_eq!(entries[0].number, 7);
        assert_eq!(entries[0].head_branch, "feat/cache");
        assert_eq!(entries[0].author, "alice");
        assert!(matches!(entries[0].kind, RefKind::Mr));
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
}
