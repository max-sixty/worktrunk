//! CI status detection for GitHub, GitLab, Gitea, and Azure DevOps.
//!
//! This module provides CI status detection by querying GitHub PRs/workflows,
//! GitLab MRs/pipelines, Gitea PRs/commit-statuses, and Azure DevOps
//! PRs/pipelines using their respective CLI tools (`gh`, `glab`, `tea`, and
//! `az`).

mod azure;
mod cache;
mod gitea;
mod github;
mod gitlab;
mod platform;

use std::process::Output;

use anstyle::{AnsiColor, Color, Style};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use worktrunk::git::{BranchRef, Repository, parse_owner_repo};
use worktrunk::shell_exec::Cmd;
use worktrunk::utils::epoch_now;

/// A parsed branch name for CI status detection.
///
/// CI tools like `gh` and `glab` expect bare branch names (e.g., `"feature"`),
/// not remote-prefixed refs (e.g., `"origin/feature"`). This type holds
/// parsed branch components:
/// 1. `name` - bare branch name for CI tool API calls
/// 2. `remote` - remote name for URL lookups (if remote branch)
/// 3. `full_name` - original name for cache keys
#[derive(Debug, Clone)]
pub struct CiBranchName {
    /// The original full name (e.g., "origin/feature" or "feature")
    pub full_name: String,
    /// For remote branches: the remote name (e.g., "origin")
    pub remote: Option<String>,
    /// The bare branch name (e.g., "feature")
    pub name: String,
}

impl CiBranchName {
    /// Create from a [`BranchRef`], using its short name and remote/local kind.
    ///
    /// For remote branches (e.g., "origin/feature"), splits at the first `/`
    /// to extract the remote name and bare branch name.
    /// For local branches, the name is already bare.
    ///
    /// Returns `None` for detached HEAD (no short name).
    pub fn from_branch_ref(branch_ref: &BranchRef) -> Option<Self> {
        let short = branch_ref.short_name()?;
        if branch_ref.is_remote() {
            // Remote branch — split "origin/feature" into remote + bare name.
            if let Some((remote, name)) = short.split_once('/') {
                return Some(Self {
                    full_name: short.to_string(),
                    remote: Some(remote.to_string()),
                    name: name.to_string(),
                });
            }
        }
        // Local branch — name is already bare
        Some(Self {
            full_name: short.to_string(),
            remote: None,
            name: short.to_string(),
        })
    }

    /// Returns true if this is a remote branch reference.
    pub fn is_remote(&self) -> bool {
        self.remote.is_some()
    }

    /// Check if this branch has upstream (remote tracking) configured.
    ///
    /// Remote branches inherently "have upstream" since they ARE the upstream.
    /// Local branches need tracking config to have upstream.
    pub fn has_upstream(&self, repo: &Repository) -> bool {
        self.is_remote() || repo.branch(&self.name).upstream().ok().flatten().is_some()
    }
}

// Re-export public types
pub(crate) use cache::{CachedCiStatus, MaxPrNumber};
// Only the `--prs` picker consumes this re-export.
pub(crate) use github::GitHubPrInfo;

/// Maximum number of PRs/MRs to fetch when filtering by source repository.
///
/// We fetch multiple results because the same branch name may exist in
/// multiple forks. 20 should be sufficient for most cases.
///
/// # Limitation
///
/// If more than 20 PRs/MRs exist for the same branch name, we only search the
/// first page. This means in extremely busy repos with many forks, our PR/MR
/// could be on page 2+ and not be found. This is a trade-off: pagination would
/// require multiple API calls and slow down status detection. In practice, 20
/// is sufficient for most workflows.
const MAX_PRS_TO_FETCH: u8 = 20;

/// Create a Cmd configured for non-interactive batch execution.
///
/// This prevents tools like `gh` and `glab` from:
/// - Prompting for user input
/// - Using TTY-specific output formatting
/// - Opening browsers for authentication
pub(crate) fn non_interactive_cmd(program: &str) -> Cmd {
    Cmd::new(program)
        .env_remove("CLICOLOR_FORCE")
        .env_remove("GH_FORCE_TTY")
        .env("NO_COLOR", "1")
        .env("CLICOLOR", "0")
        .env("GH_PROMPT_DISABLED", "1")
}

/// Check if a CLI tool is available
///
/// On Windows, CreateProcessW (via Cmd) searches PATH for .exe files.
/// We provide .exe mocks in tests via mock-stub, so this works consistently.
pub(crate) fn tool_available(tool: &str, args: &[&str]) -> bool {
    Cmd::new(tool)
        .args(args.iter().copied())
        .run()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Parse JSON output from CLI tools
fn parse_json<T: DeserializeOwned>(stdout: &[u8], command: &str, branch: &str) -> Option<T> {
    serde_json::from_slice(stdout)
        .map_err(|e| tracing::warn!(command = %command, branch = %branch, error = %e, "Failed to parse {} JSON for {}: {}", command, branch, e))
        .ok()
}

/// Combine stderr and stdout for retriable-error sniffing.
///
/// Some CLIs (notably `tea`) report API errors as JSON on stdout while
/// transport errors land on stderr — checking both avoids missing retriable
/// errors when the tool routes them differently.
fn output_error_text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    )
}

/// If a non-success `Output` indicates a retriable failure, surface it as a
/// PR-status warning. Returns `None` for non-retriable failures so callers
/// can `?` it to fall through to "no CI status".
fn retriable_pr_error(output: &Output) -> Option<PrStatus> {
    is_retriable_error(&output_error_text(output)).then(PrStatus::error)
}

/// Resolve `(owner, repo)` for a branch's effective remote.
///
/// Thin wrapper over [`branch_remote_url`] + [`parse_owner_repo`]. The
/// platform was already chosen upstream by [`Repository::ci_platform`] —
/// either from explicit `forge.platform` config or from the URL host —
/// so backends don't re-filter here. Re-checking via the host heuristic
/// (`is_gitea` / `is_github`) would silently drop legitimate hosts that
/// rely on the explicit override (e.g. `codeberg.org` for Forgejo,
/// `git.mycompany.com` for self-hosted GHE).
fn branch_owner_repo(repo: &Repository, branch: &CiBranchName) -> Option<(String, String)> {
    parse_owner_repo(&branch_remote_url(repo, branch)?)
}

/// Resolve the effective URL for a branch's remote without parsing.
///
/// Resolution chain:
/// - Remote-branch refs (`origin/feature`) read from the branch's own
///   remote via [`Repository::effective_remote_url`] (honors
///   `url.insteadOf` rewrites).
/// - Local branches prefer the branch's push destination
///   (`branch.<n>.pushRemote` → `remote.pushDefault` → tracking remote),
///   falling back to the repo's primary remote so a tracking-less branch
///   still resolves.
///
/// Backends with their own URL parser (e.g. Azure DevOps' org/project shape)
/// compose this directly; backends that want `(owner, repo)` use
/// [`branch_owner_repo`].
fn branch_remote_url(repo: &Repository, branch: &CiBranchName) -> Option<String> {
    if let Some(remote_name) = &branch.remote {
        repo.effective_remote_url(remote_name)
    } else {
        repo.branch(&branch.name).push_remote_url().or_else(|| {
            let remote = repo.primary_remote().ok()?;
            repo.effective_remote_url(&remote)
        })
    }
}

/// Check if stderr indicates a retriable error (rate limit, network issues)
fn is_retriable_error(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    [
        "rate limit",
        "api rate",
        "403",
        "429",
        "timeout",
        "connection",
        "network",
    ]
    .iter()
    .any(|p| lower.contains(p))
}

/// Status of CI tools availability
#[derive(Debug, Clone, Copy)]
pub struct CiToolsStatus {
    /// gh is installed (can run --version)
    pub gh_installed: bool,
    /// gh is installed and authenticated
    pub gh_authenticated: bool,
    /// glab is installed (can run --version)
    pub glab_installed: bool,
    /// glab is installed and authenticated
    pub glab_authenticated: bool,
    /// tea is installed (can run --version)
    pub tea_installed: bool,
    /// tea is installed and has a login configured
    pub tea_authenticated: bool,
    /// az is installed (can run --version)
    pub az_installed: bool,
    /// az is installed and authenticated (logged in)
    pub az_authenticated: bool,
}

impl CiToolsStatus {
    /// Check which CI tools are available
    ///
    /// If `gitlab_host` is provided, checks glab auth status against that specific
    /// host instead of the default. This is important for self-hosted GitLab instances
    /// where the default host (gitlab.com) may be unreachable.
    pub fn detect(gitlab_host: Option<&str>) -> Self {
        let gh_installed = tool_available("gh", &["--version"]);
        let gh_authenticated = gh_installed && tool_available("gh", &["auth", "status"]);
        let glab_installed = tool_available("glab", &["--version"]);
        let glab_authenticated = glab_installed
            && if let Some(host) = gitlab_host {
                tool_available("glab", &["auth", "status", "--hostname", host])
            } else {
                tool_available("glab", &["auth", "status"])
            };
        let tea_installed = tool_available("tea", &["--version"]);
        // `tea` stores logins in its config file; reading it (rather than
        // invoking `tea`) avoids the OAuth-refresh side effect a `tea` lookup
        // can trigger. See `git::remote_ref::gitea`.
        let tea_authenticated = tea_installed && worktrunk::git::remote_ref::gitea::has_any_login();
        let az_installed = tool_available("az", &["--version"]);
        // `az account show` exits non-zero when logged out — works whether or not
        // the azure-devops extension is installed.
        let az_authenticated = az_installed && tool_available("az", &["account", "show"]);
        Self {
            gh_installed,
            gh_authenticated,
            glab_installed,
            glab_authenticated,
            tea_installed,
            tea_authenticated,
            az_installed,
            az_authenticated,
        }
    }
}

/// CI status from GitHub/GitLab checks
/// Matches the statusline.sh color scheme:
/// - Passed: Green (all checks passed)
/// - Running: Blue (checks in progress)
/// - Failed: Red (checks failed)
/// - Conflicts: Yellow (merge conflicts)
/// - NoCI: Gray (no PR/checks)
/// - Error: Yellow (CI fetch failed, e.g., rate limit)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::IntoStaticStr)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum CiStatus {
    Passed,
    Running,
    Failed,
    Conflicts,
    NoCI,
    /// CI status could not be fetched (rate limit, network error, etc.)
    Error,
}

/// Source of CI status (PR/MR vs branch workflow)
///
/// Serialized to JSON as "pr" or "branch" for programmatic consumers.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::IntoStaticStr, JsonSchema,
)]
#[strum(serialize_all = "kebab-case")]
pub enum CiSource {
    /// Pull request or merge request
    #[serde(rename = "pr", alias = "pull-request")]
    PullRequest,
    /// Branch workflow/pipeline (no PR/MR)
    #[serde(rename = "branch")]
    Branch,
}

/// A PR/MR reference: number plus the forge's display sigil.
///
/// Displays as `#3035` (GitHub, Gitea, Azure DevOps) or `!3035` (GitLab).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrRef {
    pub number: u64,
    /// Display sigil: `#` (GitHub, Gitea, Azure DevOps) or `!` (GitLab)
    pub sigil: char,
}

impl PrRef {
    /// A pull-request reference: `#3035` (GitHub, Gitea, Azure DevOps).
    pub fn pr(number: u64) -> Self {
        Self { number, sigil: '#' }
    }

    /// A merge-request reference: `!3035` (GitLab).
    pub fn mr(number: u64) -> Self {
        Self { number, sigil: '!' }
    }

    /// Rendered width in terminal columns (sigil + digits).
    pub fn width(self) -> usize {
        pr_ref_width(self.number)
    }
}

impl std::fmt::Display for PrRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", self.sigil, self.number)
    }
}

/// Rendered width of a PR/MR reference with the given number: one sigil
/// column plus the decimal digits. Sigil-independent (`#` and `!` are both
/// one column), so layout can size the CI column from a bare number.
pub fn pr_ref_width(number: u64) -> usize {
    2 + number.checked_ilog10().unwrap_or(0) as usize
}

/// Review state of a PR/MR.
///
/// The vocabulary matches Claude Code's statusline `pr.review_state` field so
/// the two surfaces never disagree on names. A PR with no review signal at all
/// (e.g. GitHub's `reviewDecision` is empty on repos without required
/// reviewers and no reviews) carries `None` on [`PrStatus`], not `Pending`,
/// so unreviewed branches keep their plain CI colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    Approved,
    ChangesRequested,
    /// Review is required before merge (e.g. branch protection) but not given yet
    Pending,
    Draft,
}

/// CI status from PR/MR or branch workflow
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrStatus {
    pub ci_status: CiStatus,
    /// Source of the CI status (PR/MR or branch workflow)
    pub source: CiSource,
    /// True if local HEAD differs from remote HEAD (unpushed changes)
    pub is_stale: bool,
    /// True for the picker's cache-prime placeholder: the number is shown
    /// from cache while the live `CiStatus` task refreshes, but the cached
    /// CI color isn't yet trusted, so the cell renders neutral-dim instead of
    /// green/red until the fetch overwrites it. Set only by
    /// [`populate_from_cache`]; a transient render hint, never cached.
    #[serde(skip)]
    pub is_priming: bool,
    /// URL to the PR/MR (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// PR/MR reference (absent for branch workflows). `serde(default)` keeps
    /// cache entries written before this field existed readable — they render
    /// as the bare `#` until their TTL expires and a fresh fetch fills the
    /// number in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<PrRef>,
    /// Review state of the PR/MR (absent when the forge reports no review signal)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_state: Option<ReviewState>,
    /// PR/MR title. Absent for branch workflows (no PR) and for cache entries
    /// written before this field existed. Shown in the picker's worktree-row
    /// `pr` preview pane, riding the same forge call the CI column already makes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// PR/MR author login (GitHub) / username (GitLab). Absent as for `title`.
    /// Folded into a worktree row's matcher text so a PR filters by author the
    /// same whether it's shown as a worktree row or a listed `--prs` row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// PR/MR description (GitHub `body`, GitLab/Azure/Gitea `description`/`body`),
    /// rendered as markdown in the `pr` preview pane. Absent as for `title`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Number of conversation comments on the PR/MR — GitHub issue comments,
    /// GitLab user notes, Gitea PR comments — shown as a `comments` line in the
    /// picker's `pr` preview pane. Rides the same forge list call as
    /// `title`/`body`. Zero is flattened to `None` at the mapping boundary so a
    /// PR with no comments shows nothing; also `None` for branch workflows (no
    /// PR), for Azure DevOps (its PR-list call carries no comment count), and
    /// for cache entries written before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment_count: Option<u32>,
}

impl CiStatus {
    /// Get the ANSI color for this CI status.
    ///
    /// - Passed: Green
    /// - Running: Blue
    /// - Failed: Red
    /// - Conflicts: Yellow
    /// - NoCI: BrightBlack (dimmed)
    /// - Error: Yellow (warning color)
    pub fn color(&self) -> AnsiColor {
        match self {
            Self::Passed => AnsiColor::Green,
            Self::Running => AnsiColor::Blue,
            Self::Failed => AnsiColor::Red,
            Self::Conflicts | Self::Error => AnsiColor::Yellow,
            Self::NoCI => AnsiColor::BrightBlack,
        }
    }
}

impl PrStatus {
    /// Display color merging CI status with review state.
    ///
    /// Review states slot in where their required action ranks: conflicts and
    /// fetch errors still lead; changes-requested outranks a running build
    /// because waiting can't clear it; an outstanding required review only
    /// recolors an otherwise green or quiet branch. Cool colors mean waiting
    /// (blue: on CI, cyan: on a reviewer), warm colors mean act.
    pub fn color(&self) -> AnsiColor {
        match (self.ci_status, self.review_state) {
            (CiStatus::Conflicts | CiStatus::Error, _) => self.ci_status.color(),
            (_, Some(ReviewState::ChangesRequested)) => AnsiColor::Magenta,
            (CiStatus::Running | CiStatus::Failed, _) => self.ci_status.color(),
            (_, Some(ReviewState::Pending)) => AnsiColor::Cyan,
            _ => self.ci_status.color(),
        }
    }

    /// Get the style for this PR status (color + dimming for stale/draft)
    pub fn style(&self) -> Style {
        // Cache-prime placeholder: the number is real but the cached color is
        // a guess the live fetch is about to overwrite, so render neutral-dim
        // (no green/red) rather than assert a CI verdict we don't yet trust.
        if self.is_priming {
            return Style::new().dimmed();
        }
        let style = Style::new().fg_color(Some(Color::Ansi(self.color())));
        if self.is_stale || self.review_state == Some(ReviewState::Draft) {
            style.dimmed()
        } else {
            style
        }
    }

    /// Get the indicator symbol for this status
    ///
    /// - Error: ⚠ (warning indicator)
    /// - All others: # (a PR reference with the number unavailable)
    pub fn indicator(&self) -> &'static str {
        if matches!(self.ci_status, CiStatus::Error) {
            "⚠"
        } else {
            "#"
        }
    }

    /// Wrap `text` in this status's style, optionally as an OSC 8 hyperlink
    /// to the PR/pipeline URL.
    fn styled(&self, text: &str, include_link: bool) -> String {
        if let (true, Some(url)) = (include_link, &self.url) {
            let style = self.style().underline();
            format!(
                "{}{}{}{}{}",
                style,
                osc8::Hyperlink::new(url),
                text,
                osc8::Hyperlink::END,
                style.render_reset()
            )
        } else {
            let style = self.style();
            format!("{style}{text}{style:#}")
        }
    }

    /// Format CI status for a cell `max_width` columns wide.
    ///
    /// Shows the PR/MR reference (`#3035`, `!3035`) colored by CI status when
    /// one exists and fits; otherwise falls back to the bare indicator. The
    /// fallback covers branch workflows (no PR), pre-number cache entries,
    /// and numbers wider than the column's pre-allocated estimate — the
    /// column never resizes mid-render. Statusline callers pass `usize::MAX`
    /// (no width cap). `Error` always renders `⚠`, even when a reference is
    /// known: Error and Conflicts share the warning color, so a yellow
    /// `#3035` would be indistinguishable from a conflicted PR.
    ///
    /// When `include_link` is false, the cell is colored but not clickable
    /// (for environments without OSC 8 hyperlinks, e.g. Claude Code).
    pub fn format_cell(&self, max_width: usize, include_link: bool) -> String {
        match self.number {
            Some(r) if !matches!(self.ci_status, CiStatus::Error) && r.width() <= max_width => {
                self.styled(&r.to_string(), include_link)
            }
            _ => self.styled(self.indicator(), include_link),
        }
    }

    /// Create an error status for retriable failures (rate limit, network errors)
    fn error() -> Self {
        Self {
            ci_status: CiStatus::Error,
            source: CiSource::Branch,
            is_stale: false,
            is_priming: false,
            url: None,
            number: None,
            review_state: None,
            title: None,
            body: None,
            author: None,
            comment_count: None,
        }
    }

    /// Detect CI status for a branch using the forge CLI (`gh`/`glab`/`tea`/`az`)
    /// First tries to find PR/MR status, then falls back to workflow/pipeline runs
    /// Returns None if no CI found or CLI tools unavailable
    ///
    /// # Caching
    /// Results (including None) are cached in `.git/wt/cache/ci-status/<branch>.json`
    /// for 30-60 seconds to avoid hitting GitHub API rate limits. TTL uses deterministic jitter
    /// based on repo path to spread cache expirations across concurrent statuslines. Invalidated
    /// when HEAD changes.
    ///
    /// # Fork Support
    /// Runs gh commands from the repository directory to enable auto-detection of
    /// upstream repositories for forks. This ensures PRs opened against upstream
    /// repos are properly detected.
    ///
    /// # Arguments
    /// * `branch` - The parsed branch name (may be local or remote).
    /// * `local_head` - The commit SHA to check CI status for.
    pub fn detect(repo: &Repository, branch: &CiBranchName, local_head: &str) -> Option<Self> {
        let has_upstream = branch.has_upstream(repo);
        let repo_path = repo.current_worktree().root().ok()?;

        // Check cache first to avoid hitting API rate limits
        // Use full_name as cache key to distinguish local "feature" from remote "origin/feature"
        let now_secs = epoch_now();

        // `age` saturates: under a pinned test clock (or real clock skew)
        // `now_secs` can predate `checked_at`, so a raw subtraction would
        // underflow-panic when this `debug!` fires (its args build at `-vv`).
        // `saturating_sub` clamps to 0, matching `CachedCiStatus::is_valid`.
        let status = match CachedCiStatus::read(repo, &branch.full_name) {
            Some(cached) if cached.is_valid(local_head, now_secs, &repo_path) => {
                tracing::debug!(
                    branch = %branch.full_name,
                    age = %(now_secs.saturating_sub(cached.checked_at)),
                    ttl = %CachedCiStatus::ttl_for_repo(&repo_path),
                    status = ?cached.status.as_ref().map(|s| &s.ci_status),
                    "Using cached CI status for {} (age={}s, ttl={}s, status={:?})",
                    branch.full_name,
                    now_secs.saturating_sub(cached.checked_at),
                    CachedCiStatus::ttl_for_repo(&repo_path),
                    cached.status.as_ref().map(|s| &s.ci_status)
                );
                cached.status
            }
            cached => {
                if let Some(cached) = cached {
                    tracing::debug!(
                        branch = %branch.full_name,
                        age = %(now_secs.saturating_sub(cached.checked_at)),
                        ttl = %CachedCiStatus::ttl_for_repo(&repo_path),
                        head_match = %(cached.head == local_head),
                        "Cache expired for {} (age={}s, ttl={}s, head_match={})",
                        branch.full_name,
                        now_secs.saturating_sub(cached.checked_at),
                        CachedCiStatus::ttl_for_repo(&repo_path),
                        cached.head == local_head
                    );
                }

                let status = Self::detect_uncached(repo, branch, local_head, has_upstream);

                // Cache the result (including None - means no CI found for this branch)
                let cached = CachedCiStatus {
                    status: status.clone(),
                    checked_at: now_secs,
                    head: local_head.to_string(),
                    branch: branch.full_name.clone(),
                };
                cached.write(repo, &branch.full_name);

                status
            }
        };

        // Ratchet the repo-level width hint that sizes the `wt list` CI column.
        // Runs on cache hits too, so a deleted or racily regressed max.json
        // heals from locally cached numbers instead of waiting out the TTL.
        if let Some(r) = status.as_ref().and_then(|s| s.number) {
            MaxPrNumber::ratchet(repo, r.number);
        }

        status
    }

    /// Detect CI status without caching (internal implementation)
    ///
    /// Platform is determined from project config (`forge.platform`), falling
    /// back to the remote URL host. Returns `None` if the platform cannot be
    /// determined (user should set `forge.platform` for non-standard hostnames).
    /// PR/MR detection always runs. Workflow/pipeline fallback only runs if `has_upstream`.
    fn detect_uncached(
        repo: &Repository,
        branch: &CiBranchName,
        local_head: &str,
        has_upstream: bool,
    ) -> Option<Self> {
        // Determine platform (project config, branch's remote, or primary remote URL).
        match repo.ci_platform(branch.remote.as_deref()) {
            Some(p) => platform::detect_ci(p, repo, branch, local_head, has_upstream),
            None => {
                // Unknown platform — user should set forge.platform in project config
                tracing::debug!(
                    "Could not detect CI platform from remote URL; \
                     set forge.platform in .config/wt.toml for CI status"
                );
                None
            }
        }
    }
}

/// Prime `pr_status` on items from the CI cache without touching the network.
///
/// The interactive picker fetches CI live, but a cached result is local data,
/// so priming it lets the CI column and `pr` tab show cached status on the
/// first frame while the live `CiStatus` task refreshes each row behind it. A valid
/// entry (HEAD unchanged, within TTL) is used as-is. An expired entry that
/// names a PR/MR is kept with `is_priming` set: a PR/MR number is stable per
/// branch *identity*, so it still identifies the PR, but the cached CI color
/// may be outdated, so `is_priming` renders the number neutral-dim (no
/// green/red) until the live fetch overwrites it. An expired entry without a
/// number is dropped — a stale dot conveys nothing but the outdated color.
///
/// Rows with no usable cache entry are left `None` (pending): the live task
/// fills them, so they must not be resolved to "no PR" here.
///
/// `item.branch` is the cache key for every row shape: local worktrees and
/// branches cache under the bare name, remote rows under `origin/...` —
/// the same `full_name` the `CiStatus` task writes.
pub(crate) fn populate_from_cache(repo: &Repository, items: &mut [super::model::ListItem]) {
    // Common never-fetched case (no `wt list --full`/statusline run yet):
    // skip the per-row file probes entirely.
    if !CachedCiStatus::cache_dir_exists(repo) {
        return;
    }
    let Ok(repo_path) = repo.current_worktree().root() else {
        return;
    };
    let now_secs = epoch_now();
    for item in items.iter_mut() {
        let Some(branch) = item.branch.as_deref() else {
            continue;
        };
        let Some(cached) = CachedCiStatus::read(repo, branch) else {
            continue;
        };
        if cached.is_valid(&item.head, now_secs, &repo_path) {
            item.pr_status = Some(cached.status);
        } else if let Some(stale) = cached.status.filter(|s| s.number.is_some()) {
            // Show the number now, but render it neutral-dim: the cached color
            // is stale and the live task will overwrite this cell shortly.
            // `is_stale` (a SHA mismatch) carries through from the cache as-is —
            // the live fetch recomputes it; `is_priming` alone drives the
            // placeholder rendering, so this never fabricates a SHA mismatch.
            item.pr_status = Some(Some(PrStatus {
                is_priming: true,
                ..stale
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::model::ListItem;
    use super::*;
    use worktrunk::testing::TestRepo;

    #[test]
    fn test_is_retriable_error() {
        // Rate limit errors
        assert!(is_retriable_error("API rate limit exceeded"));
        assert!(is_retriable_error("rate limit exceeded for requests"));
        assert!(is_retriable_error("Error 403: forbidden"));
        assert!(is_retriable_error("HTTP 429 Too Many Requests"));

        // Network errors
        assert!(is_retriable_error("connection timed out"));
        assert!(is_retriable_error("network error"));
        assert!(is_retriable_error("timeout waiting for response"));

        // Case insensitivity
        assert!(is_retriable_error("RATE LIMIT"));
        assert!(is_retriable_error("Connection Reset"));

        // Non-retriable errors
        assert!(!is_retriable_error("branch not found"));
        assert!(!is_retriable_error("invalid credentials"));
        assert!(!is_retriable_error("permission denied"));
        assert!(!is_retriable_error(""));
    }

    #[test]
    fn test_ci_status_color() {
        use anstyle::AnsiColor;

        assert_eq!(CiStatus::Passed.color(), AnsiColor::Green);
        assert_eq!(CiStatus::Running.color(), AnsiColor::Blue);
        assert_eq!(CiStatus::Failed.color(), AnsiColor::Red);
        assert_eq!(CiStatus::Conflicts.color(), AnsiColor::Yellow);
        assert_eq!(CiStatus::Error.color(), AnsiColor::Yellow);
        assert_eq!(CiStatus::NoCI.color(), AnsiColor::BrightBlack);
    }

    #[test]
    fn test_pr_status_indicator() {
        let pr_passed = PrStatus {
            ci_status: CiStatus::Passed,
            source: CiSource::PullRequest,
            is_stale: false,
            is_priming: false,
            url: None,
            number: None,
            review_state: None,
            title: None,
            body: None,
            author: None,
            comment_count: None,
        };
        assert_eq!(pr_passed.indicator(), "#");

        let branch_running = PrStatus {
            ci_status: CiStatus::Running,
            source: CiSource::Branch,
            is_stale: false,
            is_priming: false,
            url: None,
            number: None,
            review_state: None,
            title: None,
            body: None,
            author: None,
            comment_count: None,
        };
        assert_eq!(branch_running.indicator(), "#");

        let error_status = PrStatus {
            ci_status: CiStatus::Error,
            source: CiSource::PullRequest,
            is_stale: false,
            is_priming: false,
            url: None,
            number: None,
            review_state: None,
            title: None,
            body: None,
            author: None,
            comment_count: None,
        };
        assert_eq!(error_status.indicator(), "⚠");
    }

    #[test]
    fn test_format_cell() {
        use insta::assert_snapshot;

        let pr = PrStatus {
            ci_status: CiStatus::Passed,
            source: CiSource::PullRequest,
            is_stale: false,
            is_priming: false,
            url: Some("https://github.com/owner/repo/pull/123".to_string()),
            number: Some(PrRef::pr(123)),
            review_state: None,
            title: None,
            body: None,
            author: None,
            comment_count: None,
        };

        // Number fits → PR reference, hyperlinked when supported
        assert_snapshot!(pr.format_cell(4, false), @"[32m#123[0m");
        assert_snapshot!(pr.format_cell(4, true), @r"[4m[32m]8;;https://github.com/owner/repo/pull/123\#123]8;;\[0m");
        // Number wider than the column → bare # indicator, still hyperlinked
        assert_snapshot!(pr.format_cell(3, false), @"[32m#[0m");
        assert_snapshot!(pr.format_cell(3, true), @r"[4m[32m]8;;https://github.com/owner/repo/pull/123\#]8;;\[0m");

        // No number (branch workflow or pre-number cache entry) → bare # indicator
        let branch = PrStatus {
            number: None,
            ..pr.clone()
        };
        assert_snapshot!(branch.format_cell(10, false), @"[32m#[0m");

        // Error renders ⚠ even when the number fits: Error and Conflicts
        // share yellow, so a yellow "#123" would read as a conflicted PR
        let error = PrStatus {
            ci_status: CiStatus::Error,
            ..pr.clone()
        };
        assert_snapshot!(error.format_cell(usize::MAX, false), @"[33m⚠[0m");

        // GitLab sigil
        let mr = PrStatus {
            number: Some(PrRef::mr(7)),
            ..pr
        };
        assert_snapshot!(mr.format_cell(usize::MAX, false), @"[32m!7[0m");
    }

    #[test]
    fn test_pr_ref_width() {
        assert_eq!(pr_ref_width(1), 2);
        assert_eq!(pr_ref_width(9), 2);
        assert_eq!(pr_ref_width(10), 3);
        assert_eq!(pr_ref_width(3035), 5);
        assert_eq!(pr_ref_width(99999), 6);
        assert_eq!(PrRef::mr(3035).to_string(), "!3035");
        assert_eq!(PrRef::mr(3035).width(), 5);
    }

    #[test]
    fn test_pr_status_error_constructor() {
        let error = PrStatus::error();
        assert_eq!(error.ci_status, CiStatus::Error);
        assert_eq!(error.source, CiSource::Branch);
        assert!(!error.is_stale);
        assert!(error.url.is_none());
        assert!(error.number.is_none());
    }

    #[test]
    fn test_ci_branch_name_from_local_branch_ref() {
        let branch_ref = BranchRef::local_branch("feature", "abc123");
        let ci = CiBranchName::from_branch_ref(&branch_ref).expect("local has short_name");
        assert_eq!(ci.full_name, "feature");
        assert_eq!(ci.name, "feature");
        assert_eq!(ci.remote, None);
        assert!(!ci.is_remote());
    }

    #[test]
    fn test_ci_branch_name_from_remote_branch_ref() {
        let branch_ref = BranchRef::remote_branch("origin/feature", "abc123");
        let ci = CiBranchName::from_branch_ref(&branch_ref).expect("remote has short_name");
        assert_eq!(ci.full_name, "origin/feature");
        assert_eq!(ci.name, "feature");
        assert_eq!(ci.remote.as_deref(), Some("origin"));
        assert!(ci.is_remote());
    }

    #[test]
    fn test_ci_branch_name_from_detached_head() {
        let detached = BranchRef {
            full_ref: None,
            commit_sha: "abc123".to_string(),
            worktree_path: None,
        };
        assert!(CiBranchName::from_branch_ref(&detached).is_none());
    }

    #[test]
    fn test_pr_status_style() {
        let passed = |is_stale: bool, is_priming: bool| PrStatus {
            ci_status: CiStatus::Passed,
            source: CiSource::PullRequest,
            is_stale,
            is_priming,
            url: None,
            number: Some(PrRef::pr(12)),
            review_state: None,
            title: None,
            body: None,
            author: None,
            comment_count: None,
        };
        let green = "\u{1b}[32m";
        let dim = "\u{1b}[2m";

        // Fresh verdict: green, not dimmed.
        let fresh = passed(false, false).format_cell(3, false);
        assert!(
            fresh.contains(green) && !fresh.contains(dim),
            "fresh: green, no dim: {fresh:?}"
        );

        // SHA-mismatch stale keeps its verdict color, dimmed — `wt list` flags a
        // failing/passing pushed commit even when local HEAD has moved on.
        let stale = passed(true, false).format_cell(3, false);
        assert!(
            stale.contains(green) && stale.contains(dim),
            "stale: green + dim: {stale:?}"
        );

        // Cache-prime placeholder: dimmed and neutral — the number shows, but no
        // green/red is asserted until the live fetch lands.
        let priming = passed(false, true).format_cell(3, false);
        assert!(
            priming.contains(dim) && !priming.contains(green),
            "priming: dim, neutral: {priming:?}"
        );
    }

    #[test]
    fn test_pr_status_color_merges_review_state() {
        let pr = |ci_status, review_state| PrStatus {
            ci_status,
            source: CiSource::PullRequest,
            is_stale: false,
            is_priming: false,
            url: None,
            number: None,
            review_state,
            title: None,
            body: None,
            author: None,
            comment_count: None,
        };

        // Changes-requested outranks running and passed — waiting can't clear it
        assert_eq!(
            pr(CiStatus::Running, Some(ReviewState::ChangesRequested)).color(),
            AnsiColor::Magenta
        );
        assert_eq!(
            pr(CiStatus::Passed, Some(ReviewState::ChangesRequested)).color(),
            AnsiColor::Magenta
        );
        // Conflicts and fetch errors still lead
        assert_eq!(
            pr(CiStatus::Conflicts, Some(ReviewState::ChangesRequested)).color(),
            AnsiColor::Yellow
        );
        assert_eq!(
            pr(CiStatus::Error, Some(ReviewState::ChangesRequested)).color(),
            AnsiColor::Yellow
        );
        // An outstanding required review only recolors a green or quiet branch
        assert_eq!(
            pr(CiStatus::Passed, Some(ReviewState::Pending)).color(),
            AnsiColor::Cyan
        );
        assert_eq!(
            pr(CiStatus::NoCI, Some(ReviewState::Pending)).color(),
            AnsiColor::Cyan
        );
        assert_eq!(
            pr(CiStatus::Failed, Some(ReviewState::Pending)).color(),
            AnsiColor::Red
        );
        assert_eq!(
            pr(CiStatus::Running, Some(ReviewState::Pending)).color(),
            AnsiColor::Blue
        );
        // Approved and absent review keep plain CI colors
        assert_eq!(
            pr(CiStatus::Passed, Some(ReviewState::Approved)).color(),
            AnsiColor::Green
        );
        assert_eq!(pr(CiStatus::Passed, None).color(), AnsiColor::Green);

        // Draft dims rather than recoloring
        let draft = pr(CiStatus::Passed, Some(ReviewState::Draft));
        assert_eq!(draft.color(), AnsiColor::Green);
        assert_eq!(
            draft.style(),
            Style::new()
                .fg_color(Some(Color::Ansi(AnsiColor::Green)))
                .dimmed()
        );
    }

    /// Build a synthetic non-success `Output` with the given stderr/stdout
    /// bodies — `Command::output()` is the only "real" constructor and
    /// would require spawning a process. Status uses `ExitStatus::default()`
    /// (success), but the retriable-error helpers ignore status and only
    /// look at the bytes.
    fn fake_output(stderr: &str, stdout: &str) -> Output {
        Output {
            status: Default::default(),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    /// `output_error_text` must combine both streams — `tea` routes API
    /// errors to stdout while transport errors land on stderr, so a
    /// stderr-only sniff would miss rate-limit messages from the JSON body.
    #[test]
    fn test_output_error_text_combines_streams() {
        let out = fake_output("transport: connection reset", r#"{"message":"rate limit"}"#);
        let text = output_error_text(&out);
        assert!(text.contains("transport: connection reset"));
        assert!(text.contains("rate limit"));
    }

    /// `retriable_pr_error` returns `Some(PrStatus::error())` when either
    /// stream contains a retriable marker, and `None` otherwise — the
    /// fall-through case where `?` propagates "no CI status".
    #[test]
    fn test_retriable_pr_error_routing() {
        // Retriable from stderr.
        let out = fake_output("HTTP 429 Too Many Requests", "");
        let status = retriable_pr_error(&out).expect("retriable should yield Some");
        assert_eq!(status.ci_status, CiStatus::Error);

        // Retriable from stdout (the `tea` shape).
        let out = fake_output("", r#"{"message":"rate limit exceeded"}"#);
        assert!(retriable_pr_error(&out).is_some());

        // Non-retriable failure → None, so caller's `?` falls through.
        let out = fake_output("not found", "");
        assert!(retriable_pr_error(&out).is_none());

        // No body at all → None.
        let out = fake_output("", "");
        assert!(retriable_pr_error(&out).is_none());
    }

    fn passed_pr_status(number: Option<u64>) -> PrStatus {
        PrStatus {
            ci_status: CiStatus::Passed,
            source: CiSource::PullRequest,
            is_stale: false,
            is_priming: false,
            url: None,
            number: number.map(PrRef::pr),
            review_state: None,
            title: None,
            body: None,
            author: None,
            comment_count: None,
        }
    }

    fn seed_cache(
        repo: &Repository,
        branch: &str,
        status: Option<PrStatus>,
        checked_at: u64,
        head: &str,
    ) {
        CachedCiStatus {
            status,
            checked_at,
            head: head.to_string(),
            branch: branch.to_string(),
        }
        .write(repo, branch);
    }

    #[test]
    fn test_populate_from_cache() {
        let test = TestRepo::new();
        let repo = &test.repo;
        let now = epoch_now();

        seed_cache(repo, "fresh", Some(passed_pr_status(Some(123))), now, "aaa");
        // Expired (past the 60s max TTL) but carries a number
        seed_cache(
            repo,
            "expired-pr",
            Some(passed_pr_status(Some(77))),
            now - 10_000,
            "bbb",
        );
        // Expired branch-workflow dot — no number to preserve
        seed_cache(
            repo,
            "expired-dot",
            Some(passed_pr_status(None)),
            now - 10_000,
            "ccc",
        );
        // Fresh "no CI found" entry
        seed_cache(repo, "fresh-none", None, now, "ddd");

        let mut items = vec![
            ListItem::new_branch("aaa".to_string(), "fresh".to_string()),
            ListItem::new_branch("bbb".to_string(), "expired-pr".to_string()),
            ListItem::new_branch("ccc".to_string(), "expired-dot".to_string()),
            ListItem::new_branch("ddd".to_string(), "fresh-none".to_string()),
            ListItem::new_branch("eee".to_string(), "uncached".to_string()),
        ];
        populate_from_cache(repo, &mut items);

        let fresh = items[0].pr_status.as_ref().unwrap().as_ref().unwrap();
        assert_eq!(fresh.number.unwrap().number, 123);
        assert!(!fresh.is_stale);

        let expired = items[1].pr_status.as_ref().unwrap().as_ref().unwrap();
        assert_eq!(expired.number.unwrap().number, 77);
        assert!(expired.is_priming, "expired entries render neutral-dim");
        // Same HEAD, only TTL expired — not a SHA mismatch, so `is_stale` stays
        // as cached (false). The neutral-dim comes from `is_priming`, not a
        // fabricated `is_stale`.
        assert!(!expired.is_stale);

        assert!(
            items[2].pr_status.is_none(),
            "expired dot with no number conveys nothing — left pending for the live fetch"
        );
        assert!(
            matches!(items[3].pr_status, Some(None)),
            "a valid no-CI entry resolves the cell to empty"
        );
        assert!(
            items[4].pr_status.is_none(),
            "uncached row stays pending — the live fetch will fill it"
        );
    }

    #[test]
    fn test_populate_from_cache_head_moved_primes_number() {
        let test = TestRepo::new();
        let repo = &test.repo;

        // Fresh timestamp but the branch has new commits since the fetch —
        // the number still identifies the PR, the colors may not.
        seed_cache(
            repo,
            "feature",
            Some(passed_pr_status(Some(9))),
            epoch_now(),
            "old-head",
        );
        let mut items = vec![ListItem::new_branch(
            "new-head".to_string(),
            "feature".to_string(),
        )];
        populate_from_cache(repo, &mut items);
        let status = items[0].pr_status.as_ref().unwrap().as_ref().unwrap();
        assert_eq!(status.number.unwrap().number, 9);
        // Primed as a placeholder: the number shows now, but neutral-dim — the
        // cached color isn't asserted until the live fetch refreshes it (which
        // is also where `is_stale` is recomputed against the moved HEAD).
        assert!(status.is_priming);
    }

    #[test]
    fn test_populate_from_cache_leaves_uncached_rows_pending() {
        // No cache at all: the row stays pending for the live fetch.
        let test = TestRepo::new();
        let mut items = vec![ListItem::new_branch(
            "aaa".to_string(),
            "feature".to_string(),
        )];
        populate_from_cache(&test.repo, &mut items);
        assert!(items[0].pr_status.is_none());

        // A fresh "no CI" entry is a real cache hit, so it resolves the cell
        // to empty rather than leaving it pending for the live fetch.
        seed_cache(&test.repo, "feature", None, epoch_now(), "aaa");
        populate_from_cache(&test.repo, &mut items);
        assert!(matches!(items[0].pr_status, Some(None)));
    }
}
