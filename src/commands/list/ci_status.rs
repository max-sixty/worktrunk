use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::process::Command;
use worktrunk::git::Repository;

/// Extract owner from a git remote URL (works for GitHub, GitLab, Bitbucket, etc.)
///
/// Supports formats:
/// - `https://<host>/<owner>/<repo>.git`
/// - `git@<host>:<owner>/<repo>.git`
/// - `ssh://git@<host>/<owner>/<repo>.git`
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
    fn test_parse_remote_owner() {
        // GitHub HTTPS
        assert_eq!(
            parse_remote_owner("https://github.com/owner/repo.git"),
            Some("owner")
        );
        assert_eq!(
            parse_remote_owner("  https://github.com/owner/repo\n"),
            Some("owner")
        );

        // GitHub SSH (git@ form)
        assert_eq!(
            parse_remote_owner("git@github.com:owner/repo.git"),
            Some("owner")
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

/// Get the owner of the origin remote (for fork detection)
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

/// Configure command to disable color output
fn disable_color_output(cmd: &mut Command) {
    cmd.env_remove("CLICOLOR_FORCE");
    cmd.env_remove("GH_FORCE_TTY");
    cmd.env("NO_COLOR", "1");
    cmd.env("CLICOLOR", "0");
}

/// Check if a CLI tool is available
fn tool_available(tool: &str, args: &[&str]) -> bool {
    Command::new(tool)
        .args(args)
        .output()
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

    /// Compute TTL with deterministic jitter based on repo path.
    ///
    /// Different directories get different TTLs [30, 60) seconds, which spreads
    /// out cache expirations when multiple statuslines run concurrently.
    /// The jitter is deterministic so the same directory always gets the same TTL.
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
    pub fn detect(branch: &str, local_head: &str, repo_path: &std::path::Path) -> Option<Self> {
        // We run gh/glab commands from the repo directory to let them auto-detect the correct repo
        // (including upstream repos for forks)
        let repo_root = repo_path.to_str().expect("repo path is not valid UTF-8");

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
        let status = Self::detect_uncached(branch, local_head, repo_root);

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
    fn detect_uncached(branch: &str, local_head: &str, repo_root: &str) -> Option<Self> {
        // Try GitHub PR first
        if let Some(status) = Self::detect_github(branch, local_head, repo_root) {
            return Some(status);
        }

        // Try GitHub workflow runs (for branches without PRs)
        if let Some(status) = Self::detect_github_workflow(branch, local_head, repo_root) {
            return Some(status);
        }

        // Try GitLab MR
        if let Some(status) = Self::detect_gitlab(branch, local_head, repo_root) {
            return Some(status);
        }

        // Fall back to GitLab pipeline (for branches without MRs)
        Self::detect_gitlab_pipeline(branch, local_head)
    }

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

        // Use `gh pr list --head` instead of `gh pr view` to handle numeric branch names correctly.
        // When branch name is all digits (e.g., "4315"), `gh pr view` interprets it as a PR number,
        // but `gh pr list --head` correctly treats it as a branch name.
        //
        // Use --author to filter to PRs from the origin remote owner, avoiding false matches
        // with other forks that have branches with the same name (e.g., everyone's fork has "master")
        let mut cmd = Command::new("gh");
        cmd.args([
            "pr",
            "list",
            "--head",
            branch,
            "--limit",
            "1",
            "--json",
            "state,headRefOid,mergeStateStatus,statusCheckRollup,url",
        ]);
        if let Some(origin_owner) = get_origin_owner(repo_root) {
            cmd.args(["--author", &origin_owner]);
        }

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

        // gh pr list returns an array, take the first (and only) item
        let pr_list: Vec<GitHubPrInfo> = parse_json(&output.stdout, "gh pr list", branch)?;
        let pr_info = pr_list.first()?;

        // Only process open PRs
        if pr_info.state != "OPEN" {
            return None;
        }

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

    fn detect_gitlab(branch: &str, local_head: &str, repo_root: &str) -> Option<Self> {
        if !tool_available("glab", &["--version"]) {
            return None;
        }

        // Use glab mr list with --source-branch and --author to filter to MRs from the origin
        // remote owner, avoiding false matches with other forks that have branches with the
        // same name (similar to the GitHub --author fix)
        let mut cmd = Command::new("glab");
        cmd.args([
            "mr",
            "list",
            "--source-branch",
            branch,
            "--state=opened",
            "--per-page=1",
            "--output",
            "json",
        ]);
        if let Some(origin_owner) = get_origin_owner(repo_root) {
            cmd.args(["--author", &origin_owner]);
        }
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
            return None;
        }

        // glab mr list returns an array, take the first item
        let mr_list: Vec<GitLabMrInfo> = parse_json(&output.stdout, "glab mr list", branch)?;
        let mr_info = mr_list.first()?;

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

#[derive(Debug, Deserialize)]
struct GitHubPrInfo {
    state: String,
    #[serde(rename = "headRefOid")]
    head_ref_oid: Option<String>,
    #[serde(rename = "mergeStateStatus")]
    merge_state_status: Option<String>,
    #[serde(rename = "statusCheckRollup")]
    status_check_rollup: Option<Vec<GitHubCheck>>,
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GitHubCheck {
    status: Option<String>,
    conclusion: Option<String>,
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

        let has_pending = checks.iter().any(|c| {
            matches!(
                c.status.as_deref(),
                Some("IN_PROGRESS" | "QUEUED" | "PENDING" | "EXPECTED")
            )
        });

        let has_failure = checks.iter().any(|c| {
            matches!(
                c.conclusion.as_deref(),
                Some("FAILURE" | "ERROR" | "CANCELLED")
            )
        });

        if has_pending {
            CiStatus::Running
        } else if has_failure {
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

#[derive(Debug, Deserialize)]
struct GitLabMrInfo {
    sha: String,
    has_conflicts: bool,
    detailed_merge_status: Option<String>,
    head_pipeline: Option<GitLabPipeline>,
    pipeline: Option<GitLabPipeline>,
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
