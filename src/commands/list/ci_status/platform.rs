//! CI platform detection.
//!
//! Determines whether a repository uses GitHub or GitLab based on
//! project config override or remote URL detection.

use std::sync::OnceLock;

use worktrunk::git::{GitRemoteUrl, Repository};

use super::{CiBranchName, PrStatus, github, gitlab, tool_available};

/// Cached CI tool availability.
static CI_TOOLS: OnceLock<CiToolsAvailable> = OnceLock::new();

/// Cached availability of CI CLI tools (`gh`, `glab`).
///
/// Probed once on first access via `--version` check.
struct CiToolsAvailable {
    gh: bool,
    glab: bool,
}

impl CiToolsAvailable {
    fn get() -> &'static Self {
        CI_TOOLS.get_or_init(|| Self {
            gh: tool_available("gh", &["--version"]),
            glab: tool_available("glab", &["--version"]),
        })
    }
}

/// CI platform detected from project config override or remote URL.
///
/// Platform is determined by:
/// 1. Project config `ci.platform = "github"` or `"gitlab"` (takes precedence)
/// 2. Remote URL detection (searches for "github" or "gitlab" in hostname)
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::Display, strum::EnumString)]
#[strum(serialize_all = "lowercase")]
pub enum CiPlatform {
    GitHub,
    GitLab,
}

impl CiPlatform {
    /// Check if the CLI tool for this platform is available (cached).
    fn is_tool_available(self) -> bool {
        match self {
            Self::GitHub => CiToolsAvailable::get().gh,
            Self::GitLab => CiToolsAvailable::get().glab,
        }
    }

    /// Detect CI status from PR/MR.
    fn detect_pr_mr(
        self,
        repo: &Repository,
        branch: &CiBranchName,
        local_head: &str,
    ) -> Option<PrStatus> {
        match self {
            Self::GitHub => github::detect_github(repo, branch, local_head),
            Self::GitLab => gitlab::detect_gitlab(repo, branch, local_head),
        }
    }

    /// Detect CI status from branch workflow/pipeline (fallback when no PR/MR).
    fn detect_branch(
        self,
        repo: &Repository,
        branch: &CiBranchName,
        local_head: &str,
    ) -> Option<PrStatus> {
        match self {
            Self::GitHub => github::detect_github_commit_checks(repo, local_head),
            // GitLab pipeline uses the bare branch name (not "origin/feature")
            Self::GitLab => gitlab::detect_gitlab_pipeline(&branch.name, local_head),
        }
    }

    /// Detect CI status: PR/MR first, then branch workflow/pipeline if `has_upstream`.
    ///
    /// Returns `None` if the CLI tool is not available or no CI status found.
    pub(super) fn detect_ci(
        self,
        repo: &Repository,
        branch: &CiBranchName,
        local_head: &str,
        has_upstream: bool,
    ) -> Option<PrStatus> {
        if !self.is_tool_available() {
            return None;
        }
        if let Some(status) = self.detect_pr_mr(repo, branch, local_head) {
            return Some(status);
        }
        if has_upstream {
            return self.detect_branch(repo, branch, local_head);
        }
        None
    }
}

/// Detect the CI platform from a remote URL.
///
/// Uses [`GitRemoteUrl`] to parse the URL and check the host for "github" or "gitlab".
pub fn detect_platform_from_url(url: &str) -> Option<CiPlatform> {
    let parsed = GitRemoteUrl::parse(url)?;
    if parsed.is_github() {
        Some(CiPlatform::GitHub)
    } else if parsed.is_gitlab() {
        Some(CiPlatform::GitLab)
    } else {
        None
    }
}

/// Get the CI platform for a repository, optionally prioritizing a specific remote.
///
/// Priority order:
/// 1. Project config `ci.platform` override (if provided)
/// 2. The specific remote's URL (if `remote_hint` is provided)
/// 3. Any remote URL that matches a known platform
///
/// For remote branches, pass the branch's remote as `remote_hint` to ensure
/// the correct platform is detected in mixed-remote repos (e.g., GitHub + GitLab).
pub fn get_platform_for_repo(
    repo: &Repository,
    platform_override: Option<&str>,
    remote_hint: Option<&str>,
) -> Option<CiPlatform> {
    // Config override takes precedence
    if let Some(platform_str) = platform_override {
        if let Ok(platform) = platform_str.parse::<CiPlatform>() {
            log::debug!("Using CI platform from config override: {}", platform);
            return Some(platform);
        }
        log::warn!(
            "Invalid CI platform in config: '{}'. Expected 'github' or 'gitlab'.",
            platform_str
        );
    }

    // If we have a specific remote hint (e.g., from a remote branch), use that first
    if let Some(remote_name) = remote_hint
        && let Some(url) = repo.remote_url(remote_name)
        && let Some(platform) = detect_platform_from_url(&url)
    {
        log::debug!(
            "Detected CI platform {} from remote '{}' (hint)",
            platform,
            remote_name
        );
        return Some(platform);
    }

    // Search all remotes for a supported platform
    for (remote_name, url) in repo.all_remote_urls() {
        if let Some(platform) = detect_platform_from_url(&url) {
            log::debug!(
                "Detected CI platform {} from remote '{}'",
                platform,
                remote_name
            );
            return Some(platform);
        }
    }

    None
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

    #[test]
    fn test_platform_override_github() {
        // Config override should take precedence over URL detection
        assert_eq!(
            "github".parse::<CiPlatform>().ok(),
            Some(CiPlatform::GitHub)
        );
    }

    #[test]
    fn test_platform_override_gitlab() {
        // Config override should take precedence over URL detection
        assert_eq!(
            "gitlab".parse::<CiPlatform>().ok(),
            Some(CiPlatform::GitLab)
        );
    }

    #[test]
    fn test_platform_override_invalid() {
        // Invalid platform strings should not parse
        assert!("invalid".parse::<CiPlatform>().is_err());
        assert!("GITHUB".parse::<CiPlatform>().is_err()); // Case-sensitive
        assert!("GitHub".parse::<CiPlatform>().is_err()); // Case-sensitive
    }
}
