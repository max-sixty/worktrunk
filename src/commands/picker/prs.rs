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
use worktrunk::styling::{INFO_SYMBOL, StyledLine, WARNING_SYMBOL, warning_message};

use super::super::list::ci_status::{
    CiSource, CiStatus, GitHubPrInfo, PrRef, PrStatus, ReviewState, non_interactive_cmd,
    tool_available,
};
use super::super::list::columns::ColumnKind;
use super::super::list::layout::ColumnGrid;
use super::items::{
    PreviewCache, TabAvailability, WorktreeSkimItem, ansi_to_line, render_preview_tabs,
};
use super::pr_pane;
use super::preview::{PreviewMode, PreviewStateData};
use super::preview_orchestrator::PreviewOrchestrator;

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
    /// The head commit SHA, when the forge list call supplies it (GitHub
    /// `headRefOid`, GitLab `sha`). Lets the `log` tab render the rich local
    /// `git log` when the commit is already in the object store, falling back
    /// to the forge API otherwise — see [`compute_pr_log`]. `None` when the
    /// forge omits it; the row then always takes the API path.
    head_oid: Option<String>,
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

    /// The `pr:{N}` / `mr:{N}` shortcut. Doubles as the row's selection
    /// `output()` and as the preview-cache key prefix — git forbids `:` in ref
    /// names, so it can never collide with a worktree row's branch-name key.
    fn output_token(&self) -> String {
        format!("{}:{}", self.kind.shortcut(), self.number)
    }
}

/// How the `--prs` thread reports that its forge call has resolved: drop the
/// header's "loading…" marker (`pending`) and poke skim to repaint (`render_tx`).
/// Bundled so `stream_open_prs` stays within the argument budget and the two
/// always travel together.
pub(super) struct PrsStreamSignal<'a> {
    pub pending: &'a AtomicBool,
    pub render_tx: &'a OnceLock<tokio::sync::mpsc::Sender<Event>>,
}

/// The two widths a `--prs` row renders against: `list_width` for the row's own
/// column cell (matching the worktree rows' grid), and `preview_dims` for the
/// deferred preview panes — the preview window is sized differently from the
/// list column in both layouts, and the local `git log` path also needs the
/// pane *height* for its line budget. Bundled so the streaming entry points stay
/// within the argument budget.
pub(super) struct PrsLayout {
    pub list_width: usize,
    pub preview_dims: (usize, usize),
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
    layout: &PrsLayout,
    tx: &SkimItemSender,
    stashed_warnings: &Mutex<Vec<String>>,
    grid_slot: &GridSlot,
    orchestrator: &PreviewOrchestrator,
    signal: &PrsStreamSignal,
) {
    fetch_and_stream(repo, layout, tx, stashed_warnings, grid_slot, orchestrator);

    signal.pending.store(false, Ordering::Relaxed);
    if let Some(tx) = signal.render_tx.get() {
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
    layout: &PrsLayout,
    tx: &SkimItemSender,
    stashed_warnings: &Mutex<Vec<String>>,
    grid_slot: &GridSlot,
    orchestrator: &PreviewOrchestrator,
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
    // row, so stream them in one shot. As each row is built, kick off its
    // deferred preview fetches (commit log) on `COLLECT_POOL` — the row-list
    // call carries only the cheap description, so the heavier per-PR panes load
    // off-thread and `preview()` reads them from the shared cache.
    let items: Vec<Arc<dyn SkimItem>> = entries
        .into_iter()
        .map(|entry| {
            spawn_pr_previews(orchestrator, &entry, layout.preview_dims);
            Arc::new(PrSkimItem::new(
                entry,
                layout.list_width,
                grid.as_ref(),
                &orchestrator.cache,
            )) as Arc<dyn SkimItem>
        })
        .collect();
    let _ = tx.send(items);
}

/// Spawn the deferred per-row preview fetches for one `--prs` row, keyed by the
/// row's `pr:{N}` / `mr:{N}` token so [`PrSkimItem::preview`] reads them back.
/// Each is fire-and-forget on `COLLECT_POOL`, spawned once per row. `preview()`
/// only reads the cache, and the fetch runs once per row with no in-session
/// retry, so each closure resolves to a terminal pane: the rendered content, or
/// [`pr_unavailable_pane`] when the forge fetch fails. A failure therefore shows
/// a "couldn't load" pane (cleared on the next picker open), never a perpetual
/// loading placeholder (see [`PreviewOrchestrator::spawn_compute`]).
///
/// Both tabs are spawned eagerly, once per row, for all rows — so a `--prs` open
/// queues up to `2 × MAX_PRS` (~100) per-PR forge calls. This is deliberate and
/// mirrors how the picker already fetches per-row CI status: spawning once here
/// (not from `preview()`) keeps [`PrSkimItem::preview`] a pure cache read with no
/// in-flight bookkeeping, so skim's UI thread never blocks and a row can't spawn
/// a duplicate fetch on every repaint. `COLLECT_POOL` bounds how many run at once,
/// and the picker's lifetime is user-bounded, so a slow forge call never blocks
/// the command (see the "Network Access" notes in CLAUDE.md).
fn spawn_pr_previews(
    orchestrator: &PreviewOrchestrator,
    entry: &PrEntry,
    preview_dims: (usize, usize),
) {
    let token = entry.output_token();
    let (kind, number) = (entry.kind, entry.number);
    let (width, height) = preview_dims;
    let head_oid = entry.head_oid.clone();
    let head_branch = entry.head_branch.clone();
    orchestrator.spawn_compute((token.clone(), PreviewMode::Log), move |repo| {
        Some(
            compute_pr_log(
                repo,
                kind,
                number,
                head_oid.as_deref(),
                &head_branch,
                width,
                height,
            )
            .unwrap_or_else(|| pr_unavailable_pane("commit log")),
        )
    });
    orchestrator.spawn_compute((token, PreviewMode::Comments), move |repo| {
        Some(
            compute_pr_comments(repo, kind, number, width)
                .unwrap_or_else(|| pr_unavailable_pane("comments")),
        )
    });
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
    #[serde(rename = "headRefName")]
    head_ref_name: String,
    /// Head commit SHA; lets the `log` tab use a local `git log` when present.
    #[serde(rename = "headRefOid", default)]
    head_ref_oid: String,
    #[serde(default)]
    author: GhAuthor,
    /// CI/review and display fields reused via the shared `gh pr list` mapping:
    /// number, `title`, `body`, `isDraft`, `url`, `statusCheckRollup`,
    /// `reviewDecision`, `mergeStateStatus`. Flattened so one parse feeds the
    /// row display, the `pr` preview pane, and the CI-column status. `title`/
    /// `body` live on [`GitHubPrInfo`] (not here) so the worktree-row fetch,
    /// which parses `GitHubPrInfo` directly, gets them from the same widened call.
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
            "number,title,headRefName,headRefOid,author,isDraft,url,body,statusCheckRollup,reviewDecision,mergeStateStatus",
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
            title: pr.info.title.clone().unwrap_or_default(),
            head_branch: pr.head_ref_name,
            head_oid: Some(pr.head_ref_oid).filter(|s| !s.is_empty()),
            author: pr.author.login,
            is_draft: pr.info.is_draft == Some(true),
            url: pr.info.url.clone(),
            kind: RefKind::Pr,
            body: pr.info.body.clone().unwrap_or_default(),
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
    /// Diff head SHA; lets the `log` tab use a local `git log` when present.
    #[serde(default)]
    sha: String,
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
                head_oid: Some(mr.sha).filter(|s| !s.is_empty()),
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
        // The `--prs` pane reads title/body from the `PrEntry`, not this status
        // (which feeds only the CI column), so they stay absent here.
        title: None,
        body: None,
        comment_count: None,
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
    /// worktree, so the working-tree/branch-diff/upstream tabs render an empty
    /// placeholder instead.
    pr_pane: String,
    /// Shared preview cache (same map the worktree rows use), read by the
    /// deferred `log` tab. Keyed by `(output_token, mode)`; the background
    /// fetch spawned in [`spawn_pr_previews`] populates it off-thread, and a
    /// miss falls back to a loading placeholder (skim re-queries on the next
    /// selection/tab change).
    preview_cache: PreviewCache,
}

impl PrSkimItem {
    fn new(
        entry: PrEntry,
        list_width: usize,
        grid: Option<&ColumnGrid>,
        preview_cache: &PreviewCache,
    ) -> Self {
        let label = entry.kind.shortcut();
        let output_token = entry.output_token();

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
        // The shared `pr_pane` helpers build the same header / metadata / body
        // shape as the worktree-row pane (see `items::render_worktree_pr`), so
        // the two read alike. They close each styled span with a full `{reset}`
        // (\x1b[0m) because skim's ANSI parser drops color_print's `</>` (SGR
        // 22/24/39 — bold/underline/color); the `draft` value below does the same.
        let reset = Reset;
        let mut pr_pane = pr_pane::header(pr_ref, Some(&title));
        pr_pane.push_str(&pr_pane::branch_line(&head_branch));
        pr_pane.push_str(&pr_pane::metadata_line("author", &format!("@{author}")));
        if is_draft {
            pr_pane.push_str(&pr_pane::metadata_line(
                "state",
                &cformat!("<yellow>draft</>{reset}"),
            ));
        }
        if let Some(url) = url {
            pr_pane.push_str(&pr_pane::url_line(&url));
        }
        pr_pane.push_str(&pr_pane::description(&body, list_width));

        Self {
            search_text,
            rendered,
            output_token,
            pr_pane,
            preview_cache: Arc::clone(preview_cache),
        }
    }

    /// Read a deferred tab's pane from the shared cache, or a loading
    /// placeholder on a miss. The background fetch (see [`spawn_pr_previews`])
    /// keys by this row's `output_token`, the same as [`PrEntry::output_token`].
    fn cached_pane(&self, mode: PreviewMode) -> String {
        self.preview_cache
            .get(&(self.output_token.clone(), mode))
            .map(|v| v.value().clone())
            .unwrap_or_else(|| pr_deferred_loading(mode))
    }
}

/// The pane for the tabs a `--prs` row leaves empty — working-tree (1),
/// branch-diff (3), upstream (4), summary (5). The head branch isn't checked
/// out locally, so there's no working tree or diff to show; point the user at
/// the `pr` tab, which holds the PR/MR metadata. The `log` tab (2) is *not*
/// empty — it loads commits in the background (see [`compute_pr_log`]).
fn pr_row_empty_placeholder() -> String {
    let reset = Reset;
    cformat!(
        "{INFO_SYMBOL}{reset} Not checked out locally — press <bold>alt-6</>{reset} for PR details, Enter to fetch & switch\n"
    )
}

/// Placeholder for a `--prs` row's deferred tab while its background fetch is
/// still in flight. skim can't re-query a preview on its own, so the hint points
/// at the accelerator that re-reads the cache once the fetch lands — the same
/// contract as the worktree rows' `loading_placeholder`. Shown only during the
/// in-flight window: once the fetch resolves, a terminal pane replaces it — the
/// rendered content or [`pr_unavailable_pane`] on failure — so it never persists
/// past the fetch.
fn pr_deferred_loading(mode: PreviewMode) -> String {
    let reset = Reset;
    let (label, key) = match mode {
        PreviewMode::Log => ("commit log", 2u8),
        PreviewMode::Comments => ("comments", 7u8),
        // Only the deferred tabs reach here; other modes render synchronously.
        _ => ("preview", mode as u8),
    };
    cformat!(
        "{INFO_SYMBOL}{reset} Loading {label}… press <bold>alt-{key}</>{reset} again to refresh\n"
    )
}

/// Terminal pane for a `--prs` deferred tab whose background fetch failed (forge
/// CLI missing/unauthenticated, network error, unparsable JSON). The deferred
/// closures cache this in place of an empty slot, so the tab shows a clear
/// "couldn't load" rather than [`pr_deferred_loading`]'s spinner forever: the
/// fetch is spawned once per row and never retried in-session (see
/// [`spawn_pr_previews`]), so a `None` left uncached would strand the tab on
/// "Loading…". The warning glyph distinguishes it from the benign info states
/// (`No commits` / `No comments`); reopening the picker starts a fresh cache and
/// re-fetches.
fn pr_unavailable_pane(label: &str) -> String {
    let reset = Reset;
    cformat!("{WARNING_SYMBOL}{reset} Couldn't load {label} — reopen the picker to retry\n")
}

/// Run a forge CLI and return its stdout, or `None` on any failure (root
/// unresolvable, spawn error, non-zero exit). Shared by the deferred `log` /
/// `comments` fetches: a PR runs `gh <gh_args>`, an MR runs `glab <glab_args>`.
/// Runs off-thread on `COLLECT_POOL`, never on skim's UI thread; a `None` result
/// signals a failed fetch, which the deferred-preview closures turn into a
/// terminal [`pr_unavailable_pane`] (see [`spawn_pr_previews`]).
fn fetch_forge_json(
    repo: &Repository,
    kind: RefKind,
    gh_args: &[&str],
    glab_args: &[&str],
) -> Option<Vec<u8>> {
    let repo_root = repo.current_worktree().root().ok()?;
    let (program, args) = match kind {
        RefKind::Pr => ("gh", gh_args),
        RefKind::Mr => ("glab", glab_args),
    };
    let output = non_interactive_cmd(program)
        .args(args.iter().copied())
        .current_dir(&repo_root)
        .run()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

/// Fetch and render the commit log for a `--prs` row's `log` tab.
///
/// When the PR/MR head commit (`head_oid`, from the forge list call) is already
/// in the local object store — a same-repo PR off a fetched `origin` — the tab
/// renders the rich local `git log`: graph, dim/bright merge-base split, and
/// relative timestamps, identical to a worktree row's `log` tab and sharing its
/// SHA-keyed disk cache. No network for that row.
///
/// Otherwise (a fork PR, an unfetched head, or a forge that omits the SHA) the
/// commit isn't local, so the commits come from the forge API instead: `gh pr
/// view <n> --json commits` (GitHub) or `glab api
/// projects/:fullpath/merge_requests/<n>/commits` (GitLab). That path renders a
/// flat `git log --oneline`-style list — no graph or merge-base coloring, since
/// the objects aren't present to compute them. `glab`'s `--paginate` follows
/// every page so a long PR isn't capped at GitLab's default page size, matching
/// `gh`'s complete `--json` result.
fn compute_pr_log(
    repo: &Repository,
    kind: RefKind,
    number: u32,
    head_oid: Option<&str>,
    head_branch: &str,
    width: usize,
    height: usize,
) -> Option<String> {
    if let Some(oid) = head_oid
        && let Some(local) = local_log(repo, oid, head_branch, width, height)
    {
        return Some(local);
    }

    let number = number.to_string();
    let endpoint = format!("projects/:fullpath/merge_requests/{number}/commits");
    let stdout = fetch_forge_json(
        repo,
        kind,
        &["pr", "view", &number, "--json", "commits"],
        &["api", "--paginate", &endpoint],
    )?;
    match kind {
        RefKind::Pr => render_github_commits(&stdout, width),
        RefKind::Mr => render_gitlab_commits(&stdout, width),
    }
}

/// Render the local `git log` for `oid` when it's present in the object store,
/// else `None` so [`compute_pr_log`] falls back to the forge API. The
/// `^{commit}` peel both confirms the object exists locally and rejects a SHA
/// that resolves to a non-commit (a stray blob/tree), and `--quiet` keeps a
/// miss to a silent exit-1 rather than a `fatal:` on stderr.
fn local_log(
    repo: &Repository,
    oid: &str,
    head_branch: &str,
    width: usize,
    height: usize,
) -> Option<String> {
    let spec = format!("{oid}^{{commit}}");
    if !repo
        .run_command_check(&[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &spec,
        ])
        .unwrap_or(false)
    {
        return None;
    }
    let rendered = WorktreeSkimItem::compute_log_for_head(repo, oid, head_branch, width, height);
    (!rendered.is_empty()).then_some(rendered)
}

#[derive(Deserialize)]
struct GhCommitsResponse {
    #[serde(default)]
    commits: Vec<GhCommit>,
}

#[derive(Deserialize)]
struct GhCommit {
    #[serde(default)]
    oid: String,
    #[serde(rename = "messageHeadline", default)]
    message_headline: String,
}

/// Map `gh pr view <n> --json commits` to the `log` pane. gh returns commits
/// oldest-first; the log reads newest-first like `git log`, so reverse.
fn render_github_commits(stdout: &[u8], width: usize) -> Option<String> {
    let parsed: GhCommitsResponse = serde_json::from_slice(stdout).ok()?;
    let lines: Vec<(String, String)> = parsed
        .commits
        .into_iter()
        .rev()
        .map(|c| (short_hash(&c.oid), c.message_headline))
        .collect();
    Some(render_commit_lines(&lines, width))
}

#[derive(Deserialize)]
struct GlabCommit {
    #[serde(default)]
    short_id: String,
    #[serde(default)]
    title: String,
}

/// Map `glab api …/merge_requests/<n>/commits` to the `log` pane. GitLab's
/// commits endpoint returns newest-first already, so keep the order.
fn render_gitlab_commits(stdout: &[u8], width: usize) -> Option<String> {
    let commits: Vec<GlabCommit> = serde_json::from_slice(stdout).ok()?;
    let lines: Vec<(String, String)> = commits.into_iter().map(|c| (c.short_id, c.title)).collect();
    Some(render_commit_lines(&lines, width))
}

/// Abbreviate a full commit hash to the conventional short form. GitLab already
/// supplies a `short_id`; GitHub's `oid` is the full SHA.
fn short_hash(oid: &str) -> String {
    oid.chars().take(8).collect()
}

/// Render a `git log --oneline`-style list for the `log` pane: a dim short hash,
/// then the subject. The preview pane doesn't wrap, so each subject truncates to
/// the pane width rather than letting skim clip mid-escape. An empty list (a PR
/// with no commits the API returned) renders an info line so `spawn_compute`
/// caches a terminal value rather than leaving the slot empty (an empty string
/// is skipped, which would keep the loading placeholder).
fn render_commit_lines(commits: &[(String, String)], width: usize) -> String {
    let reset = Reset;
    if commits.is_empty() {
        return cformat!("{INFO_SYMBOL}{reset} No commits\n");
    }
    let mut out = String::new();
    for (short, headline) in commits {
        let budget = width.saturating_sub(short.width() + 2).max(8);
        let headline = crate::display::truncate_to_width(headline, budget);
        out.push_str(&cformat!("<dim>{short}</>{reset}  {headline}\n"));
    }
    out
}

/// Fetch and render the comments on a `--prs` row's `comments` tab:
/// `gh pr view <n> --json comments` (GitHub, the conversation-level comments) or
/// `glab api …/merge_requests/<n>/notes?sort=asc` (GitLab, human notes). `sort=asc`
/// matches GitHub's oldest-first order (GitLab defaults to newest-first), and
/// `--paginate` follows every page so a long thread isn't capped at GitLab's
/// default page size. Returns `None` on any failure; the caller maps that to a
/// terminal [`pr_unavailable_pane`] (see [`spawn_pr_previews`]).
fn compute_pr_comments(
    repo: &Repository,
    kind: RefKind,
    number: u32,
    width: usize,
) -> Option<String> {
    let number = number.to_string();
    let endpoint = format!("projects/:fullpath/merge_requests/{number}/notes?sort=asc");
    let stdout = fetch_forge_json(
        repo,
        kind,
        &["pr", "view", &number, "--json", "comments"],
        &["api", "--paginate", &endpoint],
    )?;
    match kind {
        RefKind::Pr => render_github_comments(&stdout, width),
        RefKind::Mr => render_gitlab_notes(&stdout, width),
    }
}

/// One PR/MR comment, normalized across forges for the `comments` pane.
struct Comment {
    author: String,
    body: String,
    /// RFC 3339 timestamp; rendered as relative time when parseable.
    created_at: String,
}

#[derive(Deserialize)]
struct GhCommentsResponse {
    #[serde(default)]
    comments: Vec<GhComment>,
}

#[derive(Deserialize)]
struct GhComment {
    #[serde(default)]
    author: GhAuthor,
    #[serde(default)]
    body: String,
    #[serde(rename = "createdAt", default)]
    created_at: String,
}

/// Map `gh pr view <n> --json comments` to the `comments` pane. gh returns the
/// thread oldest-first, which reads top-to-bottom.
fn render_github_comments(stdout: &[u8], width: usize) -> Option<String> {
    let parsed: GhCommentsResponse = serde_json::from_slice(stdout).ok()?;
    let comments: Vec<Comment> = parsed
        .comments
        .into_iter()
        .map(|c| Comment {
            author: c.author.login,
            body: c.body,
            created_at: c.created_at,
        })
        .collect();
    Some(render_comment_blocks(&comments, width))
}

#[derive(Deserialize)]
struct GlabNote {
    #[serde(default)]
    body: String,
    #[serde(default)]
    author: GlabAuthor,
    #[serde(default)]
    created_at: String,
    /// GitLab tags activity events (label changes, "changed the description") as
    /// system notes; only human comments belong in the pane.
    #[serde(default)]
    system: bool,
}

/// Map `glab api …/notes` to the `comments` pane, dropping system notes (label
/// changes, description edits, …) so only human comments show.
fn render_gitlab_notes(stdout: &[u8], width: usize) -> Option<String> {
    let notes: Vec<GlabNote> = serde_json::from_slice(stdout).ok()?;
    let comments: Vec<Comment> = notes
        .into_iter()
        .filter(|n| !n.system)
        .map(|n| Comment {
            author: n.author.username,
            body: n.body,
            created_at: n.created_at,
        })
        .collect();
    Some(render_comment_blocks(&comments, width))
}

/// Render the `comments` pane: each comment as a header line (author + relative
/// time) followed by its body as markdown in the house gutter — the same
/// [`pr_pane::markdown_in_gutter`] the PR/MR body uses. An empty thread renders
/// an info line so `spawn_compute` caches a terminal value rather than leaving
/// the slot empty (an empty string is skipped, which would keep the loading placeholder).
fn render_comment_blocks(comments: &[Comment], width: usize) -> String {
    let reset = Reset;
    if comments.is_empty() {
        return cformat!("{INFO_SYMBOL}{reset} No comments\n");
    }
    let mut out = String::new();
    for comment in comments {
        let author = if comment.author.is_empty() {
            "unknown"
        } else {
            &comment.author
        };
        let header = match relative_time(&comment.created_at) {
            Some(when) => cformat!("<bold>@{author}</>{reset} <dim>· {when}</>{reset}"),
            None => cformat!("<bold>@{author}</>{reset}"),
        };
        out.push_str(&header);
        out.push('\n');

        let body = comment.body.trim();
        if body.is_empty() {
            out.push_str(&cformat!("<dim>(no body)</>{reset}\n"));
        } else {
            out.push_str(&format!(
                "{reset}{}\n",
                pr_pane::markdown_in_gutter(body, width)
            ));
        }
        // Blank line between comments.
        out.push('\n');
    }
    out
}

/// Parse an RFC 3339 timestamp to abbreviated relative time ("2h", "3d"),
/// honoring `WORKTRUNK_TEST_EPOCH` via `format_relative_time_short`. `None` when
/// the forge omits or malforms the date, so the header just drops the time.
fn relative_time(iso: &str) -> Option<String> {
    let ts = chrono::DateTime::parse_from_rfc3339(iso).ok()?.timestamp();
    Some(crate::display::format_relative_time_short(ts))
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
// TODO(pr-preview-summary): give the `summary` tab content on `--prs` rows.
// The `log` tab now loads commits in the background (see `compute_pr_log`); a
// summary would feed those commits (or the PR body) through the same
// `[commit.generation]` LLM path the worktree `summary` tab uses, keyed and
// cached the same way via `spawn_pr_previews`.
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

    fn preview(&self, context: PreviewContext<'_>) -> ItemPreview {
        // Share the worktree rows' tab bar. A `--prs` row has content on the
        // `pr` tab (built at construction) and the `log` / `comments` tabs
        // (fetched in the background, read from the shared cache); the
        // working-tree/branch-diff/upstream/summary tabs are de-emphasized and
        // show a placeholder. The active tab is the same global digit, so an
        // empty tab shows its placeholder until the user switches with alt-N / Tab.
        let mode = PreviewStateData::read_mode();
        let mut result = render_preview_tabs(mode, TabAvailability::pull_request(), context.width);
        match mode {
            PreviewMode::Pr => result.push_str(&self.pr_pane),
            PreviewMode::Log | PreviewMode::Comments => result.push_str(&self.cached_pane(mode)),
            _ => result.push_str(&pr_row_empty_placeholder()),
        }
        ItemPreview::AnsiText(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dashmap::DashMap;

    /// Build a `PrSkimItem` with a throwaway empty preview cache — the deferred
    /// `log` tab is exercised separately (see `log_tab_reads_cache_then_placeholder`).
    fn pr_item(entry: PrEntry, list_width: usize, grid: Option<&ColumnGrid>) -> PrSkimItem {
        PrSkimItem::new(entry, list_width, grid, &Arc::new(DashMap::new()))
    }

    fn entry(kind: RefKind, number: u32, title: &str) -> PrEntry {
        let number_ref = match kind {
            RefKind::Pr => PrRef::pr(u64::from(number)),
            RefKind::Mr => PrRef::mr(u64::from(number)),
        };
        PrEntry {
            number,
            title: title.to_string(),
            head_branch: "feature/auth".to_string(),
            head_oid: None,
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
                title: None,
                body: None,
                comment_count: None,
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
        let pr = pr_item(entry(RefKind::Pr, 123, "Fix the flaky test"), 120, None);
        assert_eq!(pr.output(), "pr:123");

        let mr = pr_item(entry(RefKind::Mr, 7, "Add caching"), 120, None);
        assert_eq!(mr.output(), "mr:7");
    }

    #[test]
    fn search_text_covers_number_title_branch_author() {
        let pr = pr_item(entry(RefKind::Pr, 42, "Speed up startup"), 120, None);
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

        let pr = pr_item(entry(RefKind::Pr, 42, "Speed up startup"), 120, None);
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
        let pr = pr_item(entry(RefKind::Pr, 1, "Retry the flaky test"), 80, None);
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
        let pr = pr_item(e, 40, None);
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
        let pr = pr_item(e, 120, None);
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
        let pr = pr_item(
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
        let pr = pr_item(e, 120, Some(&grid_with_ci()));
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
        let mr = pr_item(
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

        let mr_freeform = pr_item(entry(RefKind::Mr, 42, "Add caching"), 120, None);
        assert!(
            plain(&mr_freeform.rendered).contains("!42"),
            "freeform row uses !"
        );

        // GitHub PRs keep `#N`.
        let pr = pr_item(
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
        let pr = pr_item(e, 60, Some(&no_flexible));
        let text = plain(&pr.rendered);
        assert!(text.width() <= 60);
        // Skim's overflow check uses CJK widths, where the truncation `…`
        // counts as 2 — the row must pass it too or skim repaints the last
        // two columns as `..`.
        assert!(text.width_cjk() <= 60);
    }

    #[test]
    fn grid_row_places_the_number_in_the_ci_column() {
        let pr = pr_item(
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
        let pr = pr_item(
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
        let pr = pr_item(e, 120, Some(&grid_with_ci()));
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
          {"number":2964,"title":"ci: freshen","headRefName":"fix/ci","headRefOid":"abc1234500000000000000000000000000000000","author":{"login":"octocat"},"isDraft":false,"url":"https://github.com/o/r/pull/2964","body":"Bumps the CI image and re-pins actions."},
          {"number":2969,"title":"wip","headRefName":"wip-branch","author":{"login":"forkuser"},"isDraft":true,"url":"https://github.com/o/r/pull/2969"}
        ]"#;
        let entries = parse_github_prs(json).unwrap();
        assert_eq!(entries.len(), 2);

        assert_eq!(entries[0].number, 2964);
        assert_eq!(entries[0].title, "ci: freshen");
        assert_eq!(entries[0].head_branch, "fix/ci");
        // `headRefOid` feeds the `log` tab's local-`git log` fast path.
        assert_eq!(
            entries[0].head_oid.as_deref(),
            Some("abc1234500000000000000000000000000000000")
        );
        assert_eq!(entries[0].author, "octocat");
        assert!(!entries[0].is_draft);
        assert!(matches!(entries[0].kind, RefKind::Pr));
        assert_eq!(entries[0].body, "Bumps the CI image and re-pins actions.");

        assert_eq!(entries[1].number, 2969);
        assert!(entries[1].is_draft);
        assert_eq!(entries[1].author, "forkuser");
        // Absent `headRefOid` → `None`, so that row always takes the forge API.
        assert!(entries[1].head_oid.is_none());
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
        let orchestrator = PreviewOrchestrator::new(test.repo.clone());
        let loading = AtomicBool::new(true);
        let (rtx, mut rrx) = tokio::sync::mpsc::channel(8);
        let render_tx = OnceLock::new();
        render_tx.set(rtx).unwrap();

        stream_open_prs(
            &test.repo,
            &PrsLayout {
                list_width: 80,
                preview_dims: (80, 24),
            },
            &tx,
            &warnings,
            &grid,
            &orchestrator,
            &PrsStreamSignal {
                pending: &loading,
                render_tx: &render_tx,
            },
        );

        assert!(!loading.load(Ordering::Relaxed), "loading flag cleared");
        assert!(matches!(rrx.try_recv(), Ok(Event::Render)), "render poked");
    }

    #[test]
    fn parse_gitlab_maps_iid_source_branch_and_username() {
        // `glab mr list --output json`: iid (not number), source_branch,
        // author.username, draft, web_url.
        let json = br#"[
          {"iid":7,"title":"Add caching","source_branch":"feat/cache","sha":"abc1234500000000000000000000000000000000","author":{"username":"alice"},"draft":false,"web_url":"https://gitlab.com/o/r/-/merge_requests/7","description":"Caches the dependency graph between jobs."},
          {"iid":8,"title":"WIP","source_branch":"wip","author":{"username":"bob"},"draft":true,"web_url":"https://gitlab.com/o/r/-/merge_requests/8"}
        ]"#;
        let entries = parse_gitlab_mrs(json).unwrap();
        assert_eq!(entries.len(), 2);

        assert_eq!(entries[0].number, 7);
        assert_eq!(entries[0].head_branch, "feat/cache");
        // `sha` feeds the `log` tab's local-`git log` fast path; absent → None.
        assert_eq!(
            entries[0].head_oid.as_deref(),
            Some("abc1234500000000000000000000000000000000")
        );
        assert!(entries[1].head_oid.is_none());
        assert_eq!(entries[0].author, "alice");
        assert!(matches!(entries[0].kind, RefKind::Mr));
        assert_eq!(entries[0].body, "Caches the dependency graph between jobs.");
        // GitLab's `description` maps to the same `body` slot; absent → empty.
        assert_eq!(entries[1].body, "");
        // The MR's `output()` shortcut uses the iid.
        assert_eq!(
            pr_item(entries.into_iter().next().unwrap(), 120, None).output(),
            "mr:7"
        );
    }

    #[test]
    fn parse_invalid_json_errors() {
        assert!(parse_github_prs(b"not json").is_err());
        assert!(parse_gitlab_mrs(b"not json").is_err());
    }

    #[test]
    fn pr_pane_shows_description_only_when_present() {
        let mut with_body = entry(RefKind::Pr, 1, "t");
        with_body.body = "A short summary of the change.".to_string();
        let pr = pr_item(with_body, 120, Some(&grid()));
        assert!(pr.pr_pane.contains("A short summary of the change."));
        // The body renders flush, not quoted in a gutter bar; `description`
        // prefixes its block with a blank line + full reset.
        assert!(
            !pr.pr_pane.contains("\x1b[107m"),
            "no gutter bar: {:?}",
            pr.pr_pane
        );
        assert!(
            pr.pr_pane.contains("\n\n\x1b[0m"),
            "description block present"
        );
        // The block is headed by a cyan `DESCRIPTION` label, matching branch/url.
        assert!(
            pr.pr_pane.contains("DESCRIPTION"),
            "DESCRIPTION label present: {:?}",
            pr.pr_pane
        );

        // The base fixture has an empty body — the description block is skipped,
        // label and all.
        let plain_pr = pr_item(entry(RefKind::Pr, 2, "t"), 120, Some(&grid()));
        assert!(
            !plain_pr.pr_pane.contains("\n\n\x1b[0m"),
            "no description block when empty"
        );
        assert!(
            !plain_pr.pr_pane.contains("DESCRIPTION"),
            "no DESCRIPTION label when empty: {:?}",
            plain_pr.pr_pane
        );
    }

    #[test]
    fn preview_renders_tabs_and_placeholder_off_the_pr_tab() {
        // No tab switch happens here, so the in-memory `read_mode()` is its
        // default (WorkingTree) — empty on a --prs row — so `preview()` renders
        // the shared tab bar plus the "not checked out" placeholder. Drives the
        // real `SkimItem::preview` (the `--prs` streaming path is too async to
        // exercise it reliably under a PTY). `context.width` (80) is wide enough
        // for the full tab bar, so the `pr` / `comments` tabs show their labels.
        let pr = pr_item(entry(RefKind::Pr, 7, "Title"), 120, Some(&grid()));
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
        // Assert on the ANSI-stripped bar so the check targets the tab labels
        // themselves, not a coincidental substring of the styled controls line.
        let bar = plain(&text);
        assert!(bar.contains("6: pr"), "pr tab present: {bar:?}");
        assert!(bar.contains("7: comments"), "comments tab present: {bar:?}");
        assert!(
            text.contains("Not checked out locally"),
            "placeholder present: {text:?}"
        );
    }

    #[test]
    fn log_tab_reads_cache_then_placeholder() {
        // The deferred `log` tab reads the shared cache keyed by the row's
        // output token. A miss shows the loading placeholder (pointing at
        // alt-2); once the background fetch lands a value under that key, the
        // pane shows it.
        let cache: PreviewCache = Arc::new(DashMap::new());
        let pr = PrSkimItem::new(entry(RefKind::Pr, 42, "t"), 120, None, &cache);

        let miss = pr.cached_pane(PreviewMode::Log);
        assert!(miss.contains("Loading commit log"), "miss: {miss:?}");
        assert!(miss.contains("alt-2"), "refresh hint: {miss:?}");

        cache.insert(
            ("pr:42".to_string(), PreviewMode::Log),
            "abc12345  Fix it\n".to_string(),
        );
        assert_eq!(pr.cached_pane(PreviewMode::Log), "abc12345  Fix it\n");
    }

    #[test]
    fn render_github_commits_oneline_newest_first() {
        // gh returns commits oldest-first; the `log` pane shows them
        // newest-first like `git log`, with a dim 8-char short hash.
        let json = br#"{"commits":[
          {"oid":"aaaaaaaa0000000000000000000000000000aaaa","messageHeadline":"older change"},
          {"oid":"bbbbbbbb1111111111111111111111111111bbbb","messageHeadline":"newer change"}
        ]}"#;
        let out = plain(&render_github_commits(json, 80).unwrap());
        assert!(
            out.find("bbbbbbbb").unwrap() < out.find("aaaaaaaa").unwrap(),
            "newest-first: {out:?}"
        );
        assert!(out.contains("newer change") && out.contains("older change"));
        // Hash abbreviated to 8 chars, not the full 40.
        assert!(!out.contains("bbbbbbbb1"), "short hash only: {out:?}");
    }

    #[test]
    fn render_gitlab_commits_keeps_order_and_uses_short_id() {
        // GitLab's commits endpoint returns newest-first already, and supplies a
        // ready `short_id`, so the order is preserved as-is.
        let json = br#"[
          {"short_id":"deadbeef","title":"newer change"},
          {"short_id":"cafef00d","title":"older change"}
        ]"#;
        let out = plain(&render_gitlab_commits(json, 80).unwrap());
        assert!(out.contains("deadbeef") && out.contains("newer change"));
        assert!(
            out.find("deadbeef").unwrap() < out.find("cafef00d").unwrap(),
            "order preserved: {out:?}"
        );
    }

    #[test]
    fn render_commits_empty_and_truncation() {
        // A PR the API reports with no commits caches an info line (a terminal
        // value) rather than leaving the slot empty.
        assert!(
            plain(&render_github_commits(br#"{"commits":[]}"#, 80).unwrap()).contains("No commits")
        );

        // The preview doesn't wrap, so a long subject truncates to the pane
        // width (after the dim hash + two spaces) rather than clipping mid-escape.
        let commits = vec![("abc12345".to_string(), "subject-".repeat(20))];
        let line = render_commit_lines(&commits, 40);
        let first = plain(&line);
        let first = first.lines().next().unwrap();
        assert!(first.width() <= 40, "within pane: {first:?}");
        assert!(first.contains('…'), "truncated: {first:?}");
    }

    #[test]
    fn render_commits_invalid_json_is_none() {
        // A forge that returns junk yields `None` from the renderer; the deferred
        // closure maps that to a couldn't-load pane (`pr_unavailable_pane`) rather
        // than caching garbage.
        assert!(render_github_commits(b"not json", 80).is_none());
        assert!(render_gitlab_commits(b"not json", 80).is_none());
    }

    #[test]
    fn log_tab_uses_local_git_log_when_head_present() {
        // When the PR head commit is already in the local object store, the
        // `log` tab renders the rich local `git log` — graph marker and full
        // subject — with no forge call, sharing the worktree rows' log path. An
        // absent SHA falls through to `None` so `compute_pr_log` can hit the
        // forge API instead. Both checks are hermetic: `local_log` never shells
        // out to a forge CLI.
        let t = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = Repository::at(t.path()).unwrap();
        std::fs::write(t.path().join("feature.txt"), "x\n").unwrap();
        repo.run_command(&["add", "feature.txt"]).unwrap();
        repo.run_command(&["commit", "-m", "Add the retry"])
            .unwrap();
        let oid = repo
            .run_command(&["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();

        // Present locally → the local git-log render (graph `*` + subject).
        let local = local_log(&repo, &oid, "feature", 80, 24).expect("head present → local log");
        let plain_local = plain(&local);
        assert!(
            plain_local.contains("Add the retry"),
            "subject present: {plain_local:?}"
        );
        assert!(plain_local.contains('*'), "graph marker: {plain_local:?}");

        // Absent locally → `None`, leaving the forge API path to the caller.
        assert!(
            local_log(&repo, &"0".repeat(40), "feature", 80, 24).is_none(),
            "absent head → no local render"
        );

        // `compute_pr_log` takes the local path for a present head without
        // touching the forge.
        let via_compute = compute_pr_log(&repo, RefKind::Pr, 42, Some(&oid), "feature", 80, 24)
            .expect("compute_pr_log renders the local log");
        assert!(plain(&via_compute).contains("Add the retry"));
    }

    #[test]
    fn comments_tab_reads_cache_then_placeholder() {
        // Like the log tab, the deferred `comments` tab reads the shared cache
        // keyed by the row's output token, with a loading placeholder (pointing
        // at alt-7) on a miss.
        let cache: PreviewCache = Arc::new(DashMap::new());
        let pr = PrSkimItem::new(entry(RefKind::Mr, 7, "t"), 120, None, &cache);

        let miss = pr.cached_pane(PreviewMode::Comments);
        assert!(miss.contains("Loading comments"), "miss: {miss:?}");
        assert!(miss.contains("alt-7"), "refresh hint: {miss:?}");

        cache.insert(
            ("mr:7".to_string(), PreviewMode::Comments),
            "rendered thread".to_string(),
        );
        assert_eq!(pr.cached_pane(PreviewMode::Comments), "rendered thread");
    }

    #[test]
    fn unavailable_pane_reads_as_a_clear_failure() {
        // A failed deferred fetch caches this terminal pane instead of leaving
        // the slot empty, so the tab shows a clear "couldn't load" rather than
        // `pr_deferred_loading`'s spinner forever. It names the tab, drops the
        // "press alt-N again" refresh hint (there's no in-session retry — only
        // reopening the picker re-fetches), and carries the warning glyph that
        // sets it apart from the benign info states ("No commits"/"No comments").
        let pane = pr_unavailable_pane("commit log");
        let stripped = plain(&pane);
        assert!(
            stripped.contains("Couldn't load commit log"),
            "names the tab: {stripped:?}"
        );
        assert!(
            !stripped.contains("Loading"),
            "not a loading state: {stripped:?}"
        );
        assert!(
            stripped.contains("reopen the picker"),
            "points at the only retry: {stripped:?}"
        );
        assert!(stripped.contains('▲'), "warning glyph: {stripped:?}");
    }

    #[test]
    fn render_github_comments_threads_author_time_and_body() {
        // Each comment renders an author + relative-time header and its body in
        // the house gutter. The timestamps are old, so a relative-time suffix
        // renders; we assert the separator's presence, not the exact value, so
        // the test is stable regardless of the wall clock.
        let json = br#"{"comments":[
          {"author":{"login":"octocat"},"body":"Looks **good** to me.","createdAt":"2024-12-01T00:00:00Z"},
          {"author":{"login":"hubot"},"body":"Reran CI.","createdAt":"2024-11-01T00:00:00Z"}
        ]}"#;
        let out = render_github_comments(json, 80).unwrap();
        let plain = plain(&out);
        assert!(
            plain.contains("@octocat") && plain.contains("@hubot"),
            "authors: {plain:?}"
        );
        // Body rendered as markdown (the `**` markers are consumed) in the gutter.
        assert!(out.contains("\x1b[107m"), "house gutter bg: {out:?}");
        assert!(
            plain.contains("good") && !plain.contains("**good**"),
            "markdown: {plain:?}"
        );
        // A relative-time suffix is present (e.g. "1mo" / "2mo").
        assert!(plain.contains('·'), "time separator: {plain:?}");
    }

    #[test]
    fn render_gitlab_notes_drops_system_notes() {
        // GitLab `/notes` interleaves human comments with system activity
        // ("changed the description", label events); only the human comment
        // survives.
        let json = br#"[
          {"body":"changed the description","author":{"username":"alice"},"created_at":"2024-12-01T00:00:00Z","system":true},
          {"body":"Nice cleanup.","author":{"username":"bob"},"created_at":"2024-12-02T00:00:00Z","system":false}
        ]"#;
        let plain = plain(&render_gitlab_notes(json, 80).unwrap());
        assert!(
            plain.contains("@bob") && plain.contains("Nice cleanup."),
            "human note: {plain:?}"
        );
        assert!(
            !plain.contains("changed the description"),
            "system note dropped: {plain:?}"
        );
        assert!(
            !plain.contains("@alice"),
            "system author dropped: {plain:?}"
        );
    }

    #[test]
    fn render_comments_empty_and_missing_fields() {
        // No comments (or only system notes filtered out) → an info line, so the
        // slot caches a terminal value rather than staying empty.
        assert!(
            plain(&render_github_comments(br#"{"comments":[]}"#, 80).unwrap())
                .contains("No comments")
        );
        // A comment missing author/date still renders: "@unknown", no time, body.
        let json = br#"{"comments":[{"body":"orphaned"}]}"#;
        let missing = plain(&render_github_comments(json, 80).unwrap());
        assert!(missing.contains("@unknown"), "fallback author: {missing:?}");
        assert!(missing.contains("orphaned"), "body kept: {missing:?}");
        assert!(
            !missing.contains('·'),
            "no time when date absent: {missing:?}"
        );

        // An empty-body comment keeps its header and shows a "(no body)" marker
        // (a reaction-only or deleted comment), rather than a blank gutter.
        let empty_body = br#"{"comments":[{"author":{"login":"octocat"},"body":"","createdAt":"2024-12-01T00:00:00Z"}]}"#;
        let rendered = plain(&render_github_comments(empty_body, 80).unwrap());
        assert!(
            rendered.contains("@octocat"),
            "header survives: {rendered:?}"
        );
        assert!(
            rendered.contains("(no body)"),
            "empty-body marker: {rendered:?}"
        );
    }

    #[test]
    fn render_comments_invalid_json_is_none() {
        assert!(render_github_comments(b"not json", 80).is_none());
        assert!(render_gitlab_notes(b"not json", 80).is_none());
    }

    #[test]
    fn relative_time_parses_rfc3339_else_none() {
        // A parseable timestamp yields a short relative string; junk yields None
        // so the comment header just drops the time.
        assert!(relative_time("2024-12-01T00:00:00Z").is_some());
        assert!(relative_time("not a date").is_none());
        assert!(relative_time("").is_none());
    }
}
