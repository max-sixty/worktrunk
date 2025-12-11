use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::process::Command;
use worktrunk::git::Repository;

/// CI platform detected from remote URL
// TODO: Add a `[ci] platform = "github" | "gitlab"` override in project config
// for cases where URL detection fails or users want to force a specific platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CiPlatform {
    GitHub,
    GitLab,
}

/// Detect the CI platform from a remote URL by searching for "github" or "gitlab".
fn detect_platform_from_url(url: &str) -> Option<CiPlatform> {
    let url_lower = url.to_ascii_lowercase();
    if url_lower.contains("github") {
        Some(CiPlatform::GitHub)
    } else if url_lower.contains("gitlab") {
        Some(CiPlatform::GitLab)
    } else {
        None
    }
}

/// Get the CI platform for a repository by checking its origin remote URL.
fn get_platform_for_repo(repo_root: &str) -> Option<CiPlatform> {
    let output = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(repo_root)
        .output()
        .ok()?;

    if output.status.success() {
        let url = String::from_utf8(output.stdout).ok()?;
        detect_platform_from_url(&url)
    } else {
        None
    }
}

/// Extract owner from a git remote URL.
///
/// Used for client-side filtering of PRs/MRs by source repository. When multiple users
/// have PRs with the same branch name (e.g., everyone has a `feature` branch), we need
/// to identify which PR comes from *our* fork/remote, not just which PR we authored.
///
/// # Why not use `--author`?
///
/// The `gh pr list --author` flag filters by who *created* the PR, not whose fork
/// the PR comes *from*. These are usually the same, but not always:
/// - Maintainers may create PRs from contributor forks
/// - Bots may create PRs on behalf of users
/// - Organization repos: `--author company` doesn't match individual user PRs
///
/// # Why client-side filtering?
///
/// Neither `gh` nor `glab` CLI support server-side filtering by source repository.
/// The `gh pr list --head` flag only accepts branch name, not `owner:branch` format.
/// So we fetch PRs matching the branch name, then filter by `headRepositoryOwner`.
///
/// # Supported URL formats
///
/// - `https://<host>/<owner>/<repo>.git` → `owner`
/// - `git@<host>:<owner>/<repo>.git` → `owner`
/// - `ssh://git@<host>/<owner>/<repo>.git` → `owner`
fn parse_remote_owner(url: &str) -> Option<&str> {
    let url = url.trim();

    let owner = if let Some(rest) = url.strip_prefix("https://") {
        // https://github.com/owner/repo.git -> owner
        rest.split('/').nth(1)
    } else if let Some(rest) = url.strip_prefix("ssh://") {
        // ssh://git@github.com/owner/repo.git -> owner
        // ssh://github.com/owner/repo.git -> owner (no user)
        let without_user = rest.split('@').next_back()?;
        without_user.split('/').nth(1)
    } else if let Some(rest) = url.strip_prefix("git@") {
        // git@github.com:owner/repo.git -> owner
        rest.split(':').nth(1)?.split('/').next()
    } else {
        None
    }?;

    if owner.is_empty() { None } else { Some(owner) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_platform_from_url() {
        // GitHub - various URL formats
        assert_eq!(
            detect_platform_from_url("https://github.com/owner/repo.git"),
            Some(CiPlatform::GitHub)
        );
        assert_eq!(
            detect_platform_from_url("git@github.com:owner/repo.git"),
            Some(CiPlatform::GitHub)
        );
        assert_eq!(
            detect_platform_from_url("ssh://git@github.com/owner/repo.git"),
            Some(CiPlatform::GitHub)
        );

        // GitHub Enterprise
        assert_eq!(
            detect_platform_from_url("https://github.mycompany.com/owner/repo.git"),
            Some(CiPlatform::GitHub)
        );

        // GitLab - various URL formats
        assert_eq!(
            detect_platform_from_url("https://gitlab.com/owner/repo.git"),
            Some(CiPlatform::GitLab)
        );
        assert_eq!(
            detect_platform_from_url("git@gitlab.com:owner/repo.git"),
            Some(CiPlatform::GitLab)
        );

        // Self-hosted GitLab
        assert_eq!(
            detect_platform_from_url("https://gitlab.example.com/owner/repo.git"),
            Some(CiPlatform::GitLab)
        );

        // Legacy schemes (http://, git://) - common on self-hosted installations
        assert_eq!(
            detect_platform_from_url("http://github.com/owner/repo.git"),
            Some(CiPlatform::GitHub)
        );
        assert_eq!(
            detect_platform_from_url("git://github.com/owner/repo.git"),
            Some(CiPlatform::GitHub)
        );
        assert_eq!(
            detect_platform_from_url("http://gitlab.example.com/owner/repo.git"),
            Some(CiPlatform::GitLab)
        );
        assert_eq!(
            detect_platform_from_url("git://gitlab.mycompany.com/owner/repo.git"),
            Some(CiPlatform::GitLab)
        );

        // Unknown platforms
        assert_eq!(
            detect_platform_from_url("https://bitbucket.org/owner/repo.git"),
            None
        );
        assert_eq!(
            detect_platform_from_url("https://codeberg.org/owner/repo.git"),
            None
        );
    }

    /// Test URL parsing for various git remote formats.
    ///
    /// This is critical for PR/MR filtering - if we parse the wrong owner,
    /// we'll show CI status for the wrong PRs.
    #[test]
    fn test_parse_remote_owner() {
        // GitHub HTTPS
        assert_eq!(
            parse_remote_owner("https://github.com/max-sixty/worktrunk.git"),
            Some("max-sixty")
        );
        assert_eq!(
            parse_remote_owner("  https://github.com/owner/repo\n"),
            Some("owner")
        );

        // GitHub SSH (git@ form) - most common for developers
        assert_eq!(
            parse_remote_owner("git@github.com:max-sixty/worktrunk.git"),
            Some("max-sixty")
        );

        // GitHub SSH (ssh:// form)
        assert_eq!(
            parse_remote_owner("ssh://git@github.com/owner/repo.git"),
            Some("owner")
        );
        assert_eq!(
            parse_remote_owner("ssh://github.com/owner/repo.git"),
            Some("owner")
        );

        // GitLab HTTPS
        assert_eq!(
            parse_remote_owner("https://gitlab.com/owner/repo.git"),
            Some("owner")
        );
        assert_eq!(
            parse_remote_owner("https://gitlab.example.com/owner/repo.git"),
            Some("owner")
        );

        // GitLab SSH
        assert_eq!(
            parse_remote_owner("git@gitlab.com:owner/repo.git"),
            Some("owner")
        );

        // Bitbucket
        assert_eq!(
            parse_remote_owner("https://bitbucket.org/owner/repo.git"),
            Some("owner")
        );
        assert_eq!(
            parse_remote_owner("git@bitbucket.org:owner/repo.git"),
            Some("owner")
        );

        // Organization repos - owner is the org, not the user
        assert_eq!(
            parse_remote_owner("https://github.com/company-org/project.git"),
            Some("company-org")
        );

        // Malformed URLs
        assert_eq!(parse_remote_owner("https://github.com/"), None);
        assert_eq!(parse_remote_owner("git@github.com:"), None);
        assert_eq!(parse_remote_owner(""), None);

        // Unsupported protocols
        assert_eq!(parse_remote_owner("http://github.com/owner/repo.git"), None);
    }

    #[test]
    fn test_ttl_jitter_range_and_determinism() {
        // Check range: TTL should be in [30, 60)
        let paths = [
            "/tmp/repo1",
            "/tmp/repo2",
            "/workspace/project",
            "/home/user/code",
        ];
        for path in paths {
            let ttl = CachedCiStatus::ttl_for_repo(path);
            assert!(
                (30..60).contains(&ttl),
                "TTL {} for path {} should be in [30, 60)",
                ttl,
                path
            );
        }

        // Check determinism: same path should always produce same TTL
        let path = "/some/consistent/path";
        let ttl1 = CachedCiStatus::ttl_for_repo(path);
        let ttl2 = CachedCiStatus::ttl_for_repo(path);
        assert_eq!(ttl1, ttl2, "Same path should produce same TTL");

        // Check diversity: different paths should likely produce different TTLs
        let diverse_paths: Vec<_> = (0..20).map(|i| format!("/repo/path{}", i)).collect();
        let ttls: std::collections::HashSet<_> = diverse_paths
            .iter()
            .map(|p| CachedCiStatus::ttl_for_repo(p))
            .collect();
        // With 20 paths mapping to 30 possible values, we expect good diversity
        assert!(
            ttls.len() >= 10,
            "Expected diverse TTLs across paths, got {} unique values",
            ttls.len()
        );
    }

    #[test]
    fn test_escape_branch_round_trip() {
        let cases = [
            "main",
            "feature/test",
            "feature.test",
            "feature-test",
            "feature_test",
            "a.b.c",
            "a/b/c",
            "feature-2Dtest", // Literal -2D in branch name
            "-",
            "--",
            "feat/fix-bug",
            // UTF-8 multi-byte sequences
            "café",     // 2-byte UTF-8 (é = 0xC3 0xA9)
            "日本語",   // 3-byte UTF-8 (CJK)
            "emoji-🎉", // 4-byte UTF-8 (emoji)
        ];

        for branch in cases {
            let escaped = CachedCiStatus::escape_branch(branch);
            let unescaped = CachedCiStatus::unescape_branch(&escaped);
            assert_eq!(
                unescaped, branch,
                "Round-trip failed for '{}': escaped='{}', unescaped='{}'",
                branch, escaped, unescaped
            );
        }
    }

    #[test]
    fn test_escape_branch_git_config_compatible() {
        // Git config keys only allow alphanumeric, `-`, and `.`
        let is_valid_config_char = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '.';

        let cases = [
            "main",
            "feature/test",
            "feature_test",
            "feature-test",
            "a/b/c",
            "user@host",
            "100%",
        ];

        for branch in cases {
            let escaped = CachedCiStatus::escape_branch(branch);
            assert!(
                escaped.chars().all(is_valid_config_char),
                "Escaped '{}' contains invalid git config chars: '{}'",
                branch,
                escaped
            );
        }
    }
}

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

/// Get the owner of the origin remote (for GitHub fork detection).
///
/// Used for client-side filtering of PRs by source repository.
/// See [`parse_remote_owner`] for details on why this is necessary.
fn get_origin_owner(repo_root: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(repo_root)
        .output()
        .ok()?;
    if output.status.success() {
        let url = String::from_utf8(output.stdout).ok()?;
        parse_remote_owner(&url).map(|s| s.to_string())
    } else {
        None
    }
}

/// Get the GitLab project ID for the current repository.
///
/// Used for client-side filtering of MRs by source project.
/// This is the GitLab equivalent of [`get_origin_owner`] for GitHub.
///
/// Returns None if glab is not available or not configured for this repo.
///
/// # Performance Note
///
/// This function is called during GitLab detection regardless of whether
/// the repo is actually GitLab-hosted. If glab is installed but the repo
/// is GitHub, this adds an unnecessary CLI call. A future optimization
/// could check the remote URL first and skip for non-GitLab remotes.
fn get_gitlab_project_id(repo_root: &str) -> Option<u64> {
    // Use glab repo view to get the project info as JSON
    let mut cmd = Command::new("glab");
    cmd.args(["repo", "view", "--output", "json"]);
    cmd.current_dir(repo_root);
    // Disable color/pager to avoid ANSI noise in JSON output
    disable_color_output(&mut cmd);
    cmd.env("PAGER", "cat");

    let output = cmd.output().ok()?;

    if !output.status.success() {
        return None;
    }

    // Parse the JSON to extract the project ID
    #[derive(Deserialize)]
    struct RepoInfo {
        id: u64,
    }

    serde_json::from_slice::<RepoInfo>(&output.stdout)
        .ok()
        .map(|info| info.id)
}

/// Configure command to disable color output
fn disable_color_output(cmd: &mut Command) {
    cmd.env_remove("CLICOLOR_FORCE");
    cmd.env_remove("GH_FORCE_TTY");
    cmd.env("NO_COLOR", "1");
    cmd.env("CLICOLOR", "0");
}

/// Check if a CLI tool is available
///
/// On Windows, this uses `cmd.exe /c` to properly resolve batch files (.cmd/.bat)
/// that may be in PATH, since Rust's Command::new doesn't search PATHEXT.
fn tool_available(tool: &str, args: &[&str]) -> bool {
    #[cfg(windows)]
    {
        // Build command string: "tool arg1 arg2..."
        let mut cmd_str = tool.to_string();
        for arg in args {
            cmd_str.push(' ');
            cmd_str.push_str(arg);
        }
        Command::new("cmd")
            .args(["/c", &cmd_str])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        Command::new(tool)
            .args(args)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
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
    pub fn detect() -> Self {
        let gh_installed = tool_available("gh", &["--version"]);
        let gh_authenticated = gh_installed && tool_available("gh", &["auth", "status"]);
        let glab_installed = tool_available("glab", &["--version"]);
        let glab_authenticated = glab_installed && tool_available("glab", &["auth", "status"]);
        Self {
            gh_installed,
            gh_authenticated,
            glab_installed,
            glab_authenticated,
        }
    }

    /// Returns true if at least one CI tool can fetch status
    pub fn any_available(&self) -> bool {
        self.gh_authenticated || self.glab_authenticated
    }
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

/// CI status from GitHub/GitLab checks
/// Matches the statusline.sh color scheme:
/// - Passed: Green (all checks passed)
/// - Running: Blue (checks in progress)
/// - Failed: Red (checks failed)
/// - Conflicts: Yellow (merge conflicts)
/// - NoCI: Gray (no PR/checks)
/// - Error: Yellow (CI fetch failed, e.g., rate limit)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CiStatus {
    Passed,
    Running,
    Failed,
    Conflicts,
    NoCI,
    /// CI status could not be fetched (rate limit, network error, etc.)
    Error,
}

/// Source of CI status
///
/// TODO: Current visual distinction (● for PR, ○ for branch) means main branch
/// always shows hollow circle when running branch CI. This may not be ideal.
/// Possible improvements:
/// - Use different symbols entirely (e.g., ● vs ◎ double circle, ● vs ⊙ circled dot)
/// - Add a third state for "primary branch" (main/master)
/// - Use different shape families (e.g., ● circle vs ■ square, ● vs ◆ diamond)
/// - Consider directional symbols for branch CI (e.g., ▶ right arrow)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CiSource {
    /// Pull request or merge request
    PullRequest,
    /// Branch workflow/pipeline (no PR/MR)
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

/// Cached CI status stored in git config
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CachedCiStatus {
    /// The cached CI status (None means no CI found for this branch)
    pub status: Option<PrStatus>,
    /// Unix timestamp when the status was fetched
    pub checked_at: u64,
    /// The HEAD commit SHA when the status was fetched
    pub head: String,
}

impl CachedCiStatus {
    /// Base cache TTL in seconds.
    const TTL_BASE_SECS: u64 = 30;

    /// Maximum jitter added to TTL in seconds.
    /// Actual TTL will be BASE + (0..JITTER) based on repo path hash.
    const TTL_JITTER_SECS: u64 = 30;

    /// Compute TTL with jitter based on repo path.
    ///
    /// Different directories get different TTLs [30, 60) seconds, which spreads
    /// out cache expirations when multiple statuslines run concurrently.
    pub(crate) fn ttl_for_repo(repo_root: &str) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        repo_root.hash(&mut hasher);
        let hash = hasher.finish();

        // Map hash to jitter range [0, TTL_JITTER_SECS)
        let jitter = hash % Self::TTL_JITTER_SECS;
        Self::TTL_BASE_SECS + jitter
    }

    /// Escape branch name for use in git config key.
    ///
    /// Git config keys only allow alphanumeric, `-`, and `.` characters.
    /// Branch names commonly contain `/` and `_`, so we encode them as `-XX`
    /// where XX is the uppercase hex value. We also encode `-` itself to
    /// ensure round-trip safety.
    ///
    /// NOTE: This encoding is verbose but necessary — standard percent-encoding
    /// uses `%` which git config doesn't allow, and base64 uses `_`. Open to
    /// simpler approaches if someone finds one.
    pub(crate) fn escape_branch(branch: &str) -> String {
        let mut escaped = String::with_capacity(branch.len());
        for ch in branch.chars() {
            match ch {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '.' => escaped.push(ch),
                '-' => escaped.push_str("-2D"),
                _ => {
                    // Encode as -XX where XX is uppercase hex
                    for byte in ch.to_string().bytes() {
                        escaped.push_str(&format!("-{byte:02X}"));
                    }
                }
            }
        }
        escaped
    }

    /// Unescape branch name from git config key.
    pub(crate) fn unescape_branch(escaped: &str) -> String {
        let mut bytes = Vec::with_capacity(escaped.len());
        let mut chars = escaped.chars().peekable();

        while let Some(ch) = chars.next() {
            if ch == '-' {
                // Try to read two hex digits
                let hex: String = chars.by_ref().take(2).collect();
                if hex.len() == 2
                    && let Ok(byte) = u8::from_str_radix(&hex, 16)
                {
                    bytes.push(byte);
                    continue;
                }
                // Invalid escape sequence, keep as-is
                bytes.push(b'-');
                bytes.extend(hex.bytes());
            } else {
                // Unescaped char - encode as UTF-8 bytes
                bytes.extend(ch.to_string().bytes());
            }
        }

        // Decode collected bytes as UTF-8
        String::from_utf8(bytes)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
    }

    /// Check if the cache is still valid
    fn is_valid(&self, current_head: &str, now_secs: u64, repo_root: &str) -> bool {
        // Cache is valid if:
        // 1. HEAD hasn't changed (same commit)
        // 2. TTL hasn't expired (with deterministic jitter based on repo path)
        let ttl = Self::ttl_for_repo(repo_root);
        self.head == current_head && now_secs.saturating_sub(self.checked_at) < ttl
    }

    /// Read cached CI status from git config
    fn read(branch: &str, repo_root: &str) -> Option<Self> {
        let config_key = format!("worktrunk.ci.{}", Self::escape_branch(branch));
        let output = Command::new("git")
            .args(["config", "--get", &config_key])
            .current_dir(repo_root)
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let json = String::from_utf8(output.stdout).ok()?;
        serde_json::from_str(json.trim()).ok()
    }

    /// Write CI status to git config cache
    fn write(&self, branch: &str, repo_root: &str) {
        let config_key = format!("worktrunk.ci.{}", Self::escape_branch(branch));
        let Ok(json) = serde_json::to_string(self) else {
            log::debug!("Failed to serialize CI cache for {}", branch);
            return;
        };
        if let Err(e) = Command::new("git")
            .args(["config", &config_key, &json])
            .current_dir(repo_root)
            .output()
        {
            log::debug!("Failed to write CI cache for {}: {}", branch, e);
        }
    }

    /// List all cached CI statuses as (branch_name, cached_status) pairs
    pub(crate) fn list_all(repo: &Repository) -> Vec<(String, Self)> {
        let output = repo
            .run_command(&["config", "--get-regexp", r"^worktrunk\.ci\."])
            .unwrap_or_default();

        output
            .lines()
            .filter_map(|line| {
                let (key, json) = line.split_once(' ')?;
                let escaped = key.strip_prefix("worktrunk.ci.")?;
                let branch = Self::unescape_branch(escaped);
                let cached: Self = serde_json::from_str(json).ok()?;
                Some((branch, cached))
            })
            .collect()
    }

    /// Clear all cached CI statuses, returns count cleared
    pub(crate) fn clear_all(repo: &Repository) -> usize {
        let output = repo
            .run_command(&["config", "--get-regexp", r"^worktrunk\.ci\."])
            .unwrap_or_default();

        let mut cleared = 0;
        for line in output.lines() {
            if let Some(key) = line.split_whitespace().next()
                && repo.run_command(&["config", "--unset", key]).is_ok()
            {
                cleared += 1;
            }
        }
        cleared
    }
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
    pub fn color(&self) -> anstyle::AnsiColor {
        use anstyle::AnsiColor;
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
    pub fn style(&self) -> anstyle::Style {
        use anstyle::{Color, Style};
        let style = Style::new().fg_color(Some(Color::Ansi(self.ci_status.color())));
        if self.is_stale { style.dimmed() } else { style }
    }

    /// Get the indicator symbol for this status
    ///
    /// - Error: ⚠ (overrides source indicator)
    /// - PullRequest: ● (filled circle)
    /// - Branch: ○ (hollow circle)
    pub fn indicator(&self) -> &'static str {
        match self.ci_status {
            CiStatus::Error => "⚠",
            _ => match self.source {
                CiSource::PullRequest => "●",
                CiSource::Branch => "○",
            },
        }
    }

    /// Format CI status as a colored indicator for statusline output.
    ///
    /// Returns a string like "●" with appropriate ANSI color.
    pub fn format_indicator(&self) -> String {
        let style = self.style();
        let indicator = self.indicator();
        format!("{style}{indicator}{style:#}")
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
    /// Results (including None) are cached in git config (`worktrunk.ci.{branch}`) for 30-60
    /// seconds to avoid hitting GitHub API rate limits. TTL uses deterministic jitter based on
    /// repo path to spread cache expirations across concurrent statuslines. Invalidated when
    /// HEAD changes.
    ///
    /// # Fork Support
    /// Runs gh commands from the repository directory to enable auto-detection of
    /// upstream repositories for forks. This ensures PRs opened against upstream
    /// repos are properly detected.
    ///
    /// # Arguments
    /// * `repo_path` - Repository root path from `Repository::worktree_root()`
    /// * `has_upstream` - Whether the branch has upstream tracking configured.
    ///   PR/MR detection always runs. Workflow/pipeline fallback only runs if true.
    pub fn detect(
        branch: &str,
        local_head: &str,
        repo_path: &std::path::Path,
        has_upstream: bool,
    ) -> Option<Self> {
        // We run gh/glab commands from the repo directory to let them auto-detect the correct repo
        // (including upstream repos for forks)
        let repo_root = repo_path.to_str()?;

        // Check cache first to avoid hitting API rate limits
        use std::time::{SystemTime, UNIX_EPOCH};
        let now_secs = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();

        if let Some(cached) = CachedCiStatus::read(branch, repo_root) {
            if cached.is_valid(local_head, now_secs, repo_root) {
                log::debug!(
                    "Using cached CI status for {} (age={}s, ttl={}s, status={:?})",
                    branch,
                    now_secs - cached.checked_at,
                    CachedCiStatus::ttl_for_repo(repo_root),
                    cached.status.as_ref().map(|s| &s.ci_status)
                );
                return cached.status;
            }
            log::debug!(
                "Cache expired for {} (age={}s, ttl={}s, head_match={})",
                branch,
                now_secs - cached.checked_at,
                CachedCiStatus::ttl_for_repo(repo_root),
                cached.head == local_head
            );
        }

        // Cache miss or expired - fetch fresh status
        let status = Self::detect_uncached(branch, local_head, repo_root, has_upstream);

        // Cache the result (including None - means no CI found for this branch)
        let cached = CachedCiStatus {
            status: status.clone(),
            checked_at: now_secs,
            head: local_head.to_string(),
        };
        cached.write(branch, repo_root);

        status
    }

    /// Detect CI status without caching (internal implementation)
    ///
    /// Platform is determined by the remote URL (github.com vs gitlab.com).
    /// For unknown platforms (e.g., GitHub Enterprise with custom domains), falls back
    /// to trying both platforms.
    /// PR/MR detection always runs. Workflow/pipeline fallback only runs if `has_upstream`.
    fn detect_uncached(
        branch: &str,
        local_head: &str,
        repo_root: &str,
        has_upstream: bool,
    ) -> Option<Self> {
        // Determine platform from remote URL
        let platform = get_platform_for_repo(repo_root);

        match platform {
            Some(CiPlatform::GitHub) => {
                Self::detect_github_ci(branch, local_head, repo_root, has_upstream)
            }
            Some(CiPlatform::GitLab) => {
                Self::detect_gitlab_ci(branch, local_head, repo_root, has_upstream)
            }
            None => {
                // Unknown platform (e.g., GitHub Enterprise, self-hosted GitLab with custom domain)
                // Fall back to trying both platforms
                log::debug!(
                    "Could not determine CI platform for {}, trying both",
                    repo_root
                );
                Self::detect_github_ci(branch, local_head, repo_root, has_upstream)
                    .or_else(|| Self::detect_gitlab_ci(branch, local_head, repo_root, has_upstream))
            }
        }
    }

    /// Detect GitHub CI status (PR first, then workflow if has_upstream)
    fn detect_github_ci(
        branch: &str,
        local_head: &str,
        repo_root: &str,
        has_upstream: bool,
    ) -> Option<Self> {
        if let Some(status) = Self::detect_github(branch, local_head, repo_root) {
            return Some(status);
        }
        if has_upstream {
            return Self::detect_github_workflow(branch, local_head, repo_root);
        }
        None
    }

    /// Detect GitLab CI status (MR first, then pipeline if has_upstream)
    fn detect_gitlab_ci(
        branch: &str,
        local_head: &str,
        repo_root: &str,
        has_upstream: bool,
    ) -> Option<Self> {
        if let Some(status) = Self::detect_gitlab(branch, local_head, repo_root) {
            return Some(status);
        }
        if has_upstream {
            return Self::detect_gitlab_pipeline(branch, local_head);
        }
        None
    }

    /// Detect GitHub PR CI status for a branch.
    ///
    /// # Filtering Strategy
    ///
    /// We need to find PRs where the head branch comes from *our* fork, not just
    /// PRs we authored. The `--author` flag filters by PR creator, but we want
    /// to filter by source repository.
    ///
    /// Since `gh pr list --head` doesn't support `owner:branch` format, we:
    /// 1. Fetch all open PRs with matching branch name (up to 20)
    /// 2. Include `headRepositoryOwner` in the JSON output
    /// 3. Filter client-side by comparing `headRepositoryOwner.login` to our origin owner
    ///
    /// This correctly handles:
    /// - Fork workflows (PRs from your fork to upstream)
    /// - Organization repos (PRs from org branches)
    /// - Multiple users with same branch name
    fn detect_github(branch: &str, local_head: &str, repo_root: &str) -> Option<Self> {
        // Check if gh is available and authenticated
        let auth = Command::new("gh").args(["auth", "status"]).output();
        match auth {
            Err(e) => {
                log::debug!("gh not available for {}: {}", branch, e);
                return None;
            }
            Ok(o) if !o.status.success() => {
                log::debug!("gh not authenticated for {}", branch);
                return None;
            }
            _ => {}
        }

        // Get origin owner for filtering (see parse_remote_owner docs for why)
        let origin_owner = get_origin_owner(repo_root);
        if origin_owner.is_none() {
            log::debug!("Could not determine origin owner for {}", repo_root);
        }

        // Use `gh pr list --head` instead of `gh pr view` to handle numeric branch names correctly.
        // When branch name is all digits (e.g., "4315"), `gh pr view` interprets it as a PR number,
        // but `gh pr list --head` correctly treats it as a branch name.
        //
        // We fetch up to MAX_PRS_TO_FETCH PRs to handle branch name collisions, then filter
        // client-side by headRepositoryOwner to find PRs from our fork.
        let mut cmd = Command::new("gh");
        cmd.args([
            "pr",
            "list",
            "--head",
            branch,
            "--state",
            "open",
            "--limit",
            &MAX_PRS_TO_FETCH.to_string(),
            "--json",
            "headRefOid,mergeStateStatus,statusCheckRollup,url,headRepositoryOwner",
        ]);

        disable_color_output(&mut cmd);
        cmd.current_dir(repo_root);

        let output = match cmd.output() {
            Ok(output) => output,
            Err(e) => {
                log::warn!("gh pr list failed to execute for branch {}: {}", branch, e);
                return None;
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            log::debug!("gh pr list failed for {}: {}", branch, stderr.trim());
            if is_retriable_error(&stderr) {
                return Some(Self::error());
            }
            return None;
        }

        // gh pr list returns an array - find the first PR from our origin
        let pr_list: Vec<GitHubPrInfo> = parse_json(&output.stdout, "gh pr list", branch)?;

        // Filter to PRs from our origin (case-insensitive comparison for GitHub usernames).
        // If headRepositoryOwner is missing (older GH CLI, Enterprise, or permissions),
        // treat it as a potential match to avoid false negatives.
        let pr_info = if let Some(ref owner) = origin_owner {
            let matched = pr_list.iter().find(|pr| {
                pr.head_repository_owner
                    .as_ref()
                    .map(|h| h.login.eq_ignore_ascii_case(owner))
                    .unwrap_or(true) // Missing owner field = potential match
            });
            if matched.is_none() && !pr_list.is_empty() {
                log::debug!(
                    "Found {} PRs for branch {} but none from origin owner {}",
                    pr_list.len(),
                    branch,
                    owner
                );
            }
            matched
        } else {
            // If we can't determine origin owner, fall back to first open PR
            // This is less accurate but better than nothing
            log::debug!(
                "No origin owner for {}, using first open PR for branch {}",
                repo_root,
                branch
            );
            pr_list.first()
        }?;

        // Determine CI status using priority: conflicts > running > failed > passed > no_ci
        let ci_status = if pr_info.merge_state_status.as_deref() == Some("DIRTY") {
            CiStatus::Conflicts
        } else {
            pr_info.ci_status()
        };

        let is_stale = pr_info
            .head_ref_oid
            .as_ref()
            .map(|pr_head| pr_head != local_head)
            .unwrap_or(false);

        Some(PrStatus {
            ci_status,
            source: CiSource::PullRequest,
            is_stale,
            url: pr_info.url.clone(),
        })
    }

    /// Detect GitLab MR CI status for a branch.
    ///
    /// # Filtering Strategy
    ///
    /// Similar to GitHub (see `detect_github`), we need to find MRs where the
    /// source branch comes from *our* project, not just MRs we authored.
    ///
    /// Since `glab mr list` doesn't support filtering by source project, we:
    /// 1. Get the current project ID via `glab repo view`
    /// 2. Fetch all open MRs with matching branch name (up to 20)
    /// 3. Filter client-side by comparing `source_project_id` to our project ID
    fn detect_gitlab(branch: &str, local_head: &str, repo_root: &str) -> Option<Self> {
        if !tool_available("glab", &["--version"]) {
            return None;
        }

        // Get current project ID for filtering
        let project_id = get_gitlab_project_id(repo_root);
        if project_id.is_none() {
            log::debug!("Could not determine GitLab project ID for {}", repo_root);
        }

        // Fetch MRs with matching source branch.
        // We filter client-side by source_project_id (numeric project ID comparison).
        let mut cmd = Command::new("glab");
        cmd.args([
            "mr",
            "list",
            "--source-branch",
            branch,
            "--state=opened",
            &format!("--per-page={}", MAX_PRS_TO_FETCH),
            "--output",
            "json",
        ]);
        cmd.current_dir(repo_root);

        let output = match cmd.output() {
            Ok(output) => output,
            Err(e) => {
                log::warn!(
                    "glab mr list failed to execute for branch {}: {}",
                    branch,
                    e
                );
                return None;
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            log::debug!("glab mr list failed for {}: {}", branch, stderr.trim());
            // Return error status for retriable failures (rate limit, network) so they
            // surface as warnings instead of being cached as "no CI"
            if is_retriable_error(&stderr) {
                return Some(Self::error());
            }
            return None;
        }

        // glab mr list returns an array - find the first MR from our project
        let mr_list: Vec<GitLabMrInfo> = parse_json(&output.stdout, "glab mr list", branch)?;

        // Filter to MRs from our project (numeric project ID comparison)
        let mr_info = if let Some(proj_id) = project_id {
            let matched = mr_list
                .iter()
                .find(|mr| mr.source_project_id == Some(proj_id));
            if matched.is_none() && !mr_list.is_empty() {
                log::debug!(
                    "Found {} MRs for branch {} but none from project ID {}",
                    mr_list.len(),
                    branch,
                    proj_id
                );
            }
            matched
        } else {
            // If we can't determine project ID, fall back to first MR
            log::debug!(
                "No project ID for {}, using first MR for branch {}",
                repo_root,
                branch
            );
            mr_list.first()
        }?;

        // Determine CI status using priority: conflicts > running > failed > passed > no_ci
        let ci_status = if mr_info.has_conflicts
            || mr_info.detailed_merge_status.as_deref() == Some("conflict")
        {
            CiStatus::Conflicts
        } else if mr_info.detailed_merge_status.as_deref() == Some("ci_still_running") {
            CiStatus::Running
        } else if mr_info.detailed_merge_status.as_deref() == Some("ci_must_pass") {
            CiStatus::Failed
        } else {
            mr_info.ci_status()
        };

        let is_stale = mr_info.sha != local_head;

        Some(PrStatus {
            ci_status,
            source: CiSource::PullRequest,
            is_stale,
            // TODO: Fetch GitLab MR URL from glab output to enable clickable links
            // Currently only GitHub PRs have clickable underlined indicators
            url: None,
        })
    }

    fn detect_github_workflow(branch: &str, local_head: &str, repo_root: &str) -> Option<Self> {
        // Note: We don't log auth failures here since detect_github already logged them
        if !tool_available("gh", &["auth", "status"]) {
            return None;
        }

        // Get most recent workflow run for the branch
        let mut cmd = Command::new("gh");
        cmd.args([
            "run",
            "list",
            "--branch",
            branch,
            "--limit",
            "1",
            "--json",
            "status,conclusion,headSha",
        ]);

        disable_color_output(&mut cmd);
        cmd.current_dir(repo_root);

        let output = match cmd.output() {
            Ok(output) => output,
            Err(e) => {
                log::warn!("gh run list failed to execute for branch {}: {}", branch, e);
                return None;
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            log::debug!("gh run list failed for {}: {}", branch, stderr.trim());
            if is_retriable_error(&stderr) {
                return Some(Self::error());
            }
            return None;
        }

        let runs: Vec<GitHubWorkflowRun> = parse_json(&output.stdout, "gh run list", branch)?;
        let run = runs.first()?;

        // Check if the workflow run matches our local HEAD commit
        let is_stale = run
            .head_sha
            .as_ref()
            .map(|run_sha| run_sha != local_head)
            .unwrap_or(true); // If no SHA, consider it stale

        // Analyze workflow run status
        let ci_status = run.ci_status();

        Some(PrStatus {
            ci_status,
            source: CiSource::Branch,
            is_stale,
            url: None, // Workflow runs don't have a PR URL
        })
    }

    fn detect_gitlab_pipeline(branch: &str, local_head: &str) -> Option<Self> {
        if !tool_available("glab", &["--version"]) {
            return None;
        }

        // Get most recent pipeline for the branch using JSON output
        let output = match Command::new("glab")
            .args(["ci", "list", "--per-page", "1", "--output", "json"])
            .env("BRANCH", branch) // glab ci list uses BRANCH env var
            .output()
        {
            Ok(output) => output,
            Err(e) => {
                log::warn!(
                    "glab ci list failed to execute for branch {}: {}",
                    branch,
                    e
                );
                return None;
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            log::debug!("glab ci list failed for {}: {}", branch, stderr.trim());
            return None;
        }

        let pipelines: Vec<GitLabPipeline> = parse_json(&output.stdout, "glab ci list", branch)?;
        let pipeline = pipelines.first()?;

        // Check if the pipeline matches our local HEAD commit
        let is_stale = pipeline
            .sha
            .as_ref()
            .map(|pipeline_sha| pipeline_sha != local_head)
            .unwrap_or(true); // If no SHA, consider it stale

        let ci_status = pipeline.ci_status();

        Some(PrStatus {
            ci_status,
            source: CiSource::Branch,
            is_stale,
            // TODO: Fetch GitLab pipeline URL to enable clickable links
            url: None,
        })
    }
}

/// GitHub PR info from `gh pr list --json ...`
///
/// Note: We include `headRepositoryOwner` for client-side filtering by source fork.
/// See [`parse_remote_owner`] for why this is necessary.
///
/// Note: We don't include `state` because we already filter with `--state open`.
#[derive(Debug, Deserialize)]
struct GitHubPrInfo {
    #[serde(rename = "headRefOid")]
    head_ref_oid: Option<String>,
    #[serde(rename = "mergeStateStatus")]
    merge_state_status: Option<String>,
    #[serde(rename = "statusCheckRollup")]
    status_check_rollup: Option<Vec<GitHubCheck>>,
    url: Option<String>,
    /// The owner of the repository the PR's head branch comes from.
    /// Used to filter PRs by source fork (see [`parse_remote_owner`]).
    #[serde(rename = "headRepositoryOwner")]
    head_repository_owner: Option<HeadRepositoryOwner>,
}

/// Owner info for the head repository of a PR.
#[derive(Debug, Deserialize)]
struct HeadRepositoryOwner {
    /// The login (username/org name) of the repository owner.
    login: String,
}

/// A single check from `statusCheckRollup`.
///
/// This is a union of two GitHub API types with different field structures:
/// - `CheckRun` (GitHub Actions): has `status` ("COMPLETED", "IN_PROGRESS") and
///   `conclusion` ("SUCCESS", "FAILURE", "CANCELLED", "SKIPPED")
/// - `StatusContext` (external CI like pre-commit.ci): has `state` only
///   ("SUCCESS", "FAILURE", "PENDING", "ERROR")
///
/// We parse all three fields and check whichever is present. An alternative approach would be
/// `gh pr checks <number> --json state` which returns a flat array with unified `state` field,
/// but that requires a separate API call after finding the PR number. Since we also need
/// `gh run list` for branch-based CI (branches without PRs), keeping the single-call approach
/// here is simpler overall.
#[derive(Debug, Deserialize)]
struct GitHubCheck {
    /// CheckRun only: "COMPLETED", "IN_PROGRESS", "QUEUED", etc.
    status: Option<String>,
    /// CheckRun only: "SUCCESS", "FAILURE", "CANCELLED", "SKIPPED", etc.
    conclusion: Option<String>,
    /// StatusContext only: "SUCCESS", "FAILURE", "PENDING", "ERROR"
    state: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GitHubWorkflowRun {
    status: Option<String>,
    conclusion: Option<String>,
    #[serde(rename = "headSha")]
    head_sha: Option<String>,
}

impl GitHubPrInfo {
    fn ci_status(&self) -> CiStatus {
        let Some(checks) = &self.status_check_rollup else {
            return CiStatus::NoCI;
        };

        if checks.is_empty() {
            return CiStatus::NoCI;
        }

        // CheckRun uses `status` for in-progress states
        let has_pending_checkrun = checks.iter().any(|c| {
            matches!(
                c.status.as_deref(),
                Some("IN_PROGRESS" | "QUEUED" | "PENDING" | "EXPECTED")
            )
        });

        // StatusContext uses `state` for pending
        let has_pending_status = checks
            .iter()
            .any(|c| matches!(c.state.as_deref(), Some("PENDING")));

        // CheckRun uses `conclusion` for final result
        let has_failure_checkrun = checks.iter().any(|c| {
            matches!(
                c.conclusion.as_deref(),
                Some("FAILURE" | "ERROR" | "CANCELLED")
            )
        });

        // StatusContext uses `state` for final result
        let has_failure_status = checks
            .iter()
            .any(|c| matches!(c.state.as_deref(), Some("FAILURE" | "ERROR")));

        if has_pending_checkrun || has_pending_status {
            CiStatus::Running
        } else if has_failure_checkrun || has_failure_status {
            CiStatus::Failed
        } else {
            CiStatus::Passed
        }
    }
}

impl GitHubWorkflowRun {
    fn ci_status(&self) -> CiStatus {
        match self.status.as_deref() {
            Some("in_progress" | "queued" | "pending" | "waiting") => CiStatus::Running,
            Some("completed") => match self.conclusion.as_deref() {
                Some("success") => CiStatus::Passed,
                Some("failure" | "cancelled" | "timed_out" | "action_required") => CiStatus::Failed,
                Some("skipped" | "neutral") | None => CiStatus::NoCI,
                _ => CiStatus::NoCI,
            },
            _ => CiStatus::NoCI,
        }
    }
}

/// GitLab MR info from `glab mr list --output json`
///
/// Note: We include `source_project_id` for client-side filtering by source project.
/// See [`parse_remote_owner`] for why we filter by source, not by author.
#[derive(Debug, Deserialize)]
struct GitLabMrInfo {
    sha: String,
    has_conflicts: bool,
    detailed_merge_status: Option<String>,
    head_pipeline: Option<GitLabPipeline>,
    pipeline: Option<GitLabPipeline>,
    /// The source project ID (the project the MR's branch comes from).
    /// Used to filter MRs by source project.
    source_project_id: Option<u64>,
}

impl GitLabMrInfo {
    fn ci_status(&self) -> CiStatus {
        self.head_pipeline
            .as_ref()
            .or(self.pipeline.as_ref())
            .map(GitLabPipeline::ci_status)
            .unwrap_or(CiStatus::NoCI)
    }
}

#[derive(Debug, Deserialize)]
struct GitLabPipeline {
    status: Option<String>,
    /// Only present in `glab ci list` output, not in MR view embedded pipeline
    #[serde(default)]
    sha: Option<String>,
}

fn parse_gitlab_status(status: Option<&str>) -> CiStatus {
    match status {
        Some(
            "running" | "pending" | "preparing" | "waiting_for_resource" | "created" | "scheduled",
        ) => CiStatus::Running,
        Some("failed" | "canceled" | "manual") => CiStatus::Failed,
        Some("success") => CiStatus::Passed,
        Some("skipped") | None => CiStatus::NoCI,
        _ => CiStatus::NoCI,
    }
}

impl GitLabPipeline {
    fn ci_status(&self) -> CiStatus {
        parse_gitlab_status(self.status.as_deref())
    }
}
