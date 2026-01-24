//! CI status detection for GitHub and GitLab.
//!
//! This module provides CI status detection by querying GitHub PRs/workflows
//! and GitLab MRs/pipelines using their respective CLI tools (`gh` and `glab`).

mod cache;
mod github;
mod gitlab;
mod platform;

use anstyle::{AnsiColor, Color, Style};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use worktrunk::git::Repository;
use worktrunk::shell_exec::Cmd;
use worktrunk::utils::get_now;

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
    /// Create from a branch name with authoritative `is_remote` flag.
    ///
    /// For remote branches (e.g., "origin/feature"), extracts the remote name
    /// and bare branch name by finding the matching remote prefix.
    /// For local branches, the name is already bare.
    ///
    /// The `is_remote` flag should come from an authoritative source:
    /// - `BranchRef::is_remote` (from collection phase)
    /// - `git show-ref --verify refs/remotes/<branch>` (for CLI input)
    pub fn from_branch_ref(branch: &str, is_remote: bool, repo: &Repository) -> Self {
        if is_remote {
            // Remote branch - find the remote prefix to extract bare name
            for (remote_name, _) in repo.all_remote_urls() {
                let prefix = format!("{}/", remote_name);
                if let Some(name) = branch.strip_prefix(&prefix) {
                    log::debug!(
                        "Remote branch {} -> remote={}, name={}",
                        branch,
                        remote_name,
                        name
                    );
                    return Self {
                        full_name: branch.to_string(),
                        remote: Some(remote_name),
                        name: name.to_string(),
                    };
                }
            }
            // Fallback: couldn't find matching remote, use first segment as remote
            if let Some((remote, name)) = branch.split_once('/') {
                log::warn!(
                    "Remote branch {} has unknown remote '{}', using as-is",
                    branch,
                    remote
                );
                return Self {
                    full_name: branch.to_string(),
                    remote: Some(remote.to_string()),
                    name: name.to_string(),
                };
            }
        }
        // Local branch - name is already bare
        Self {
            full_name: branch.to_string(),
            remote: None,
            name: branch.to_string(),
        }
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
pub(crate) use cache::CachedCiStatus;
pub use platform::{CiPlatform, get_platform_for_repo};

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
fn non_interactive_cmd(program: &str) -> Cmd {
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
fn tool_available(tool: &str, args: &[&str]) -> bool {
    Cmd::new(tool)
        .args(args.iter().copied())
        .run()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Parse JSON output from CLI tools
fn parse_json<T: DeserializeOwned>(stdout: &[u8], command: &str, branch: &str) -> Option<T> {
    serde_json::from_slice(stdout)
        .map_err(|e| log::warn!("Failed to parse {} JSON for {}: {}", command, branch, e))
        .ok()
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
        Self {
            gh_installed,
            gh_authenticated,
            glab_installed,
            glab_authenticated,
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

/// CI status from PR/MR or branch workflow
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrStatus {
    pub ci_status: CiStatus,
    /// Source of the CI status (PR/MR or branch workflow)
    pub source: CiSource,
    /// True if local HEAD differs from remote HEAD (unpushed changes)
    pub is_stale: bool,
    /// URL to the PR/MR (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
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
    /// Get the style for this PR status (color + optional dimming for stale)
    pub fn style(&self) -> Style {
        let style = Style::new().fg_color(Some(Color::Ansi(self.ci_status.color())));
        if self.is_stale { style.dimmed() } else { style }
    }

    /// Get the indicator symbol for this status
    ///
    /// - Error: ⚠ (warning indicator)
    /// - All others: ● (filled circle)
    pub fn indicator(&self) -> &'static str {
        if matches!(self.ci_status, CiStatus::Error) {
            "⚠"
        } else {
            "●"
        }
    }

    /// Format CI status with control over link inclusion.
    ///
    /// When `include_link` is false, the indicator is colored but not clickable.
    /// Used for environments that don't support OSC 8 hyperlinks (e.g., Claude Code).
    pub fn format_indicator(&self, include_link: bool) -> String {
        let indicator = self.indicator();
        if let (true, Some(url)) = (include_link, &self.url) {
            let style = self.style().underline();
            format!(
                "{}{}{}{}{}",
                style,
                osc8::Hyperlink::new(url),
                indicator,
                osc8::Hyperlink::END,
                style.render_reset()
            )
        } else {
            let style = self.style();
            format!("{style}{indicator}{style:#}")
        }
    }

    /// Create an error status for retriable failures (rate limit, network errors)
    fn error() -> Self {
        Self {
            ci_status: CiStatus::Error,
            source: CiSource::Branch,
            is_stale: false,
            url: None,
        }
    }

    /// Detect CI status for a branch using gh/glab CLI
    /// First tries to find PR/MR status, then falls back to workflow/pipeline runs
    /// Returns None if no CI found or CLI tools unavailable
    ///
    /// # Caching
    /// Results (including None) are cached in `.git/wt-cache/ci-status/<branch>.json`
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
        let now_secs = get_now();

        if let Some(cached) = CachedCiStatus::read(repo, &branch.full_name) {
            if cached.is_valid(local_head, now_secs, &repo_path) {
                log::debug!(
                    "Using cached CI status for {} (age={}s, ttl={}s, status={:?})",
                    branch.full_name,
                    now_secs - cached.checked_at,
                    CachedCiStatus::ttl_for_repo(&repo_path),
                    cached.status.as_ref().map(|s| &s.ci_status)
                );
                return cached.status;
            }
            log::debug!(
                "Cache expired for {} (age={}s, ttl={}s, head_match={})",
                branch.full_name,
                now_secs - cached.checked_at,
                CachedCiStatus::ttl_for_repo(&repo_path),
                cached.head == local_head
            );
        }

        // Cache miss or expired - fetch fresh status
        let status = Self::detect_uncached(repo, branch, local_head, has_upstream);

        // Cache the result (including None - means no CI found for this branch)
        let cached = CachedCiStatus {
            status: status.clone(),
            checked_at: now_secs,
            head: local_head.to_string(),
        };
        cached.write(repo, &branch.full_name);

        status
    }

    /// Detect CI status without caching (internal implementation)
    ///
    /// Platform is determined by project config override or remote URL detection.
    /// Returns `None` if the platform cannot be determined (user should set
    /// `ci.platform` in project config for non-standard hostnames).
    /// PR/MR detection always runs. Workflow/pipeline fallback only runs if `has_upstream`.
    fn detect_uncached(
        repo: &Repository,
        branch: &CiBranchName,
        local_head: &str,
        has_upstream: bool,
    ) -> Option<Self> {
        // Load project config for platform override (cached in Repository)
        let project_config = repo.load_project_config().ok().flatten();
        let platform_override = project_config.as_ref().and_then(|c| c.ci_platform());

        // Determine platform (config override, branch's remote, or any remote URL)
        // For remote branches, use their specific remote to get the correct platform
        let platform = get_platform_for_repo(repo, platform_override, branch.remote.as_deref());

        match platform {
            Some(p) => p.detect_ci(repo, branch, local_head, has_upstream),
            None => {
                // Unknown platform - user should set ci.platform in project config
                log::debug!(
                    "Could not detect CI platform from remote URL; \
                     set ci.platform in .config/wt.toml for CI status"
                );
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            url: None,
        };
        assert_eq!(pr_passed.indicator(), "●");

        let branch_running = PrStatus {
            ci_status: CiStatus::Running,
            source: CiSource::Branch,
            is_stale: false,
            url: None,
        };
        assert_eq!(branch_running.indicator(), "●");

        let error_status = PrStatus {
            ci_status: CiStatus::Error,
            source: CiSource::PullRequest,
            is_stale: false,
            url: None,
        };
        assert_eq!(error_status.indicator(), "⚠");
    }

    #[test]
    fn test_format_indicator_with_url() {
        let pr_with_url = PrStatus {
            ci_status: CiStatus::Passed,
            source: CiSource::PullRequest,
            is_stale: false,
            url: Some("https://github.com/owner/repo/pull/123".to_string()),
        };

        // Call format_indicator(true) directly
        let formatted = pr_with_url.format_indicator(true);
        // Should contain OSC 8 hyperlink escape sequences
        assert!(formatted.contains("\x1b]8;;"));
        assert!(formatted.contains("https://github.com/owner/repo/pull/123"));
        assert!(formatted.contains("●"));
    }

    #[test]
    fn test_format_indicator_without_url() {
        let pr_no_url = PrStatus {
            ci_status: CiStatus::Passed,
            source: CiSource::PullRequest,
            is_stale: false,
            url: None,
        };

        // Call format_indicator(true) directly
        let formatted = pr_no_url.format_indicator(true);
        // Should NOT contain OSC 8 hyperlink
        assert!(
            !formatted.contains("\x1b]8;;"),
            "Should not contain OSC 8 sequences"
        );
        assert!(formatted.contains("●"));
    }

    #[test]
    fn test_format_indicator_skips_link() {
        // When include_link=false, should not include OSC 8 even when URL is present
        let pr_with_url = PrStatus {
            ci_status: CiStatus::Passed,
            source: CiSource::PullRequest,
            is_stale: false,
            url: Some("https://github.com/owner/repo/pull/123".to_string()),
        };

        let with_link = pr_with_url.format_indicator(true);
        let without_link = pr_with_url.format_indicator(false);

        // With link should contain OSC 8
        assert!(
            with_link.contains("\x1b]8;;"),
            "include_link=true should contain OSC 8"
        );

        // Without link should NOT contain OSC 8
        assert!(
            !without_link.contains("\x1b]8;;"),
            "include_link=false should not contain OSC 8"
        );

        // Both should contain the indicator
        assert!(with_link.contains("●"), "Should contain indicator");
        assert!(without_link.contains("●"), "Should contain indicator");
    }

    #[test]
    fn test_pr_status_error_constructor() {
        let error = PrStatus::error();
        assert_eq!(error.ci_status, CiStatus::Error);
        assert_eq!(error.source, CiSource::Branch);
        assert!(!error.is_stale);
        assert!(error.url.is_none());
    }

    #[test]
    fn test_pr_status_style_and_format() {
        let status = PrStatus {
            ci_status: CiStatus::Passed,
            source: CiSource::PullRequest,
            is_stale: false,
            url: None,
        };
        // Call format_indicator directly
        let formatted = status.format_indicator(false);
        assert!(formatted.contains("●"));

        // Stale status gets dimmed
        let stale = PrStatus {
            ci_status: CiStatus::Running,
            source: CiSource::Branch,
            is_stale: true,
            url: None,
        };
        let style = stale.style();
        // Just verify it doesn't panic and returns a style
        let _ = format!("{style}test{style:#}");
    }
}
