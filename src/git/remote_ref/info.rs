//! Remote ref info types.
//!
//! Provides unified types for PR/MR metadata across platforms.

use crate::git::{RefContext, RefType};

/// Platform-specific data for a remote ref.
///
/// Contains fields that differ between GitHub and GitLab.
#[derive(Debug, Clone)]
pub enum PlatformData {
    /// GitHub-specific data.
    GitHub {
        /// GitHub host (e.g., "github.com", "github.enterprise.com").
        host: String,
        /// Owner of the head (source) repository.
        head_owner: String,
        /// Name of the head (source) repository.
        head_repo: String,
        /// Owner of the base (target) repository.
        base_owner: String,
        /// Name of the base (target) repository.
        base_repo: String,
    },
    /// GitLab-specific data.
    GitLab {
        /// Source project ID.
        source_project_id: u64,
        /// Target project ID.
        target_project_id: u64,
        /// Target project SSH URL (where MR refs live).
        target_ssh_url: Option<String>,
        /// Target project HTTP URL (where MR refs live).
        target_http_url: Option<String>,
    },
}

/// Unified information about a PR or MR.
///
/// This struct contains all the data needed to create a local branch
/// for a PR/MR, regardless of platform.
#[derive(Debug, Clone)]
pub struct RemoteRefInfo {
    /// The reference type (PR or MR).
    pub ref_type: RefType,
    /// The PR/MR number.
    pub number: u32,
    /// The PR/MR title.
    pub title: String,
    /// The PR/MR author's username.
    pub author: String,
    /// The PR/MR state ("open", "closed", "merged", etc.).
    pub state: String,
    /// Whether this is a draft PR/MR.
    pub draft: bool,
    /// The branch name in the source repository.
    pub source_branch: String,
    /// Whether this is a cross-repository (fork) PR/MR.
    pub is_cross_repo: bool,
    /// The PR/MR web URL.
    pub url: String,
    /// URL to push to for fork PRs/MRs, or `None` if push isn't supported.
    pub fork_push_url: Option<String>,
    /// Platform-specific data.
    pub platform_data: PlatformData,
}

impl RefContext for RemoteRefInfo {
    fn ref_type(&self) -> RefType {
        self.ref_type
    }

    fn number(&self) -> u32 {
        self.number
    }

    fn title(&self) -> &str {
        &self.title
    }

    fn author(&self) -> &str {
        &self.author
    }

    fn state(&self) -> &str {
        &self.state
    }

    fn draft(&self) -> bool {
        self.draft
    }

    fn url(&self) -> &str {
        &self.url
    }

    fn source_ref(&self) -> String {
        if self.is_cross_repo {
            // Try to extract owner for display
            match &self.platform_data {
                PlatformData::GitHub { head_owner, .. } => {
                    format!("{}:{}", head_owner, self.source_branch)
                }
                PlatformData::GitLab { .. } => {
                    // For GitLab, try to extract owner from fork_push_url
                    if let Some(url) = &self.fork_push_url
                        && let Some(owner) = extract_owner_from_url(url)
                    {
                        return format!("{}:{}", owner, self.source_branch);
                    }
                    self.source_branch.clone()
                }
            }
        } else {
            self.source_branch.clone()
        }
    }
}

impl RemoteRefInfo {
    /// Get the target remote URL (where refs live) for GitLab fork MRs.
    ///
    /// For GitHub, use `find_remote_for_repo` instead.
    /// Returns `None` for same-repo refs or if URL isn't available.
    ///
    /// TODO(hidden-io): This accessor calls `glab config get git_protocol` which spawns
    /// a subprocess. Consider moving protocol choice into `GitLabProvider::fetch_info`
    /// and storing the chosen URL in `RemoteRefInfo` to avoid hidden I/O.
    pub fn target_remote_url(&self) -> Option<String> {
        match &self.platform_data {
            PlatformData::GitHub { .. } => None,
            PlatformData::GitLab {
                target_ssh_url,
                target_http_url,
                ..
            } => {
                let use_ssh = get_git_protocol() == "ssh";
                if use_ssh {
                    target_ssh_url.clone().or_else(|| target_http_url.clone())
                } else {
                    target_http_url.clone().or_else(|| target_ssh_url.clone())
                }
            }
        }
    }

    /// Generate a prefixed local branch name for when the unprefixed name conflicts.
    ///
    /// Returns `<owner>/<branch>` (e.g., `contributor/main`).
    /// Only meaningful for GitHub fork PRs; GitLab doesn't support this pattern.
    pub fn prefixed_local_branch_name(&self) -> Option<String> {
        match &self.platform_data {
            PlatformData::GitHub { head_owner, .. } => {
                Some(format!("{}/{}", head_owner, self.source_branch))
            }
            PlatformData::GitLab { .. } => None,
        }
    }
}

/// Extract owner from a git URL.
///
/// Handles both SSH (`git@host:owner/repo.git`) and HTTPS
/// (`https://host/owner/repo.git`) formats.
///
/// TODO(nested-namespaces): Only extracts the first path segment. GitLab nested
/// namespaces (`group/subgroup/repo`) will display as `group:<branch>`, losing
/// the subgroup context. Consider extracting the full namespace path.
fn extract_owner_from_url(url: &str) -> Option<String> {
    // SSH format: git@host:owner/repo.git
    if let Some(path) = url.strip_prefix("git@").and_then(|s| s.split(':').nth(1)) {
        return path.split('/').next().map(|s| s.to_string());
    }
    // HTTPS format: https://host/owner/repo.git
    if let Some(path) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    {
        return path.split('/').nth(1).map(|s| s.to_string());
    }
    None
}

use super::gitlab::get_git_protocol;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_source_ref_same_repo() {
        let info = RemoteRefInfo {
            ref_type: RefType::Pr,
            number: 101,
            title: "Fix bug".to_string(),
            author: "alice".to_string(),
            state: "open".to_string(),
            draft: false,
            source_branch: "feature-auth".to_string(),
            is_cross_repo: false,
            url: "https://github.com/owner/repo/pull/101".to_string(),
            fork_push_url: None,
            platform_data: PlatformData::GitHub {
                host: "github.com".to_string(),
                head_owner: "owner".to_string(),
                head_repo: "repo".to_string(),
                base_owner: "owner".to_string(),
                base_repo: "repo".to_string(),
            },
        };
        assert_eq!(info.source_ref(), "feature-auth");
    }

    #[test]
    fn test_source_ref_fork_github() {
        let info = RemoteRefInfo {
            ref_type: RefType::Pr,
            number: 42,
            title: "Add feature".to_string(),
            author: "contributor".to_string(),
            state: "open".to_string(),
            draft: false,
            source_branch: "feature-fix".to_string(),
            is_cross_repo: true,
            url: "https://github.com/owner/repo/pull/42".to_string(),
            fork_push_url: Some("git@github.com:contributor/repo.git".to_string()),
            platform_data: PlatformData::GitHub {
                host: "github.com".to_string(),
                head_owner: "contributor".to_string(),
                head_repo: "repo".to_string(),
                base_owner: "owner".to_string(),
                base_repo: "repo".to_string(),
            },
        };
        assert_eq!(info.source_ref(), "contributor:feature-fix");
    }

    #[test]
    fn test_source_ref_fork_gitlab() {
        let info = RemoteRefInfo {
            ref_type: RefType::Mr,
            number: 101,
            title: "Fix bug".to_string(),
            author: "contributor".to_string(),
            state: "opened".to_string(),
            draft: false,
            source_branch: "feature-fix".to_string(),
            is_cross_repo: true,
            url: "https://gitlab.com/owner/repo/-/merge_requests/101".to_string(),
            fork_push_url: Some("git@gitlab.com:contributor/repo.git".to_string()),
            platform_data: PlatformData::GitLab {
                source_project_id: 456,
                target_project_id: 123,
                target_ssh_url: Some("git@gitlab.com:owner/repo.git".to_string()),
                target_http_url: Some("https://gitlab.com/owner/repo.git".to_string()),
            },
        };
        assert_eq!(info.source_ref(), "contributor:feature-fix");
    }

    #[test]
    fn test_prefixed_local_branch_name_github() {
        let info = RemoteRefInfo {
            ref_type: RefType::Pr,
            number: 101,
            title: "Test".to_string(),
            author: "contributor".to_string(),
            state: "open".to_string(),
            draft: false,
            source_branch: "main".to_string(),
            is_cross_repo: true,
            url: "https://github.com/owner/repo/pull/101".to_string(),
            fork_push_url: Some("git@github.com:contributor/repo.git".to_string()),
            platform_data: PlatformData::GitHub {
                host: "github.com".to_string(),
                head_owner: "contributor".to_string(),
                head_repo: "repo".to_string(),
                base_owner: "owner".to_string(),
                base_repo: "repo".to_string(),
            },
        };
        assert_eq!(
            info.prefixed_local_branch_name(),
            Some("contributor/main".to_string())
        );
    }

    #[test]
    fn test_prefixed_local_branch_name_gitlab() {
        let info = RemoteRefInfo {
            ref_type: RefType::Mr,
            number: 101,
            title: "Test".to_string(),
            author: "contributor".to_string(),
            state: "opened".to_string(),
            draft: false,
            source_branch: "main".to_string(),
            is_cross_repo: true,
            url: "https://gitlab.com/owner/repo/-/merge_requests/101".to_string(),
            fork_push_url: Some("git@gitlab.com:contributor/repo.git".to_string()),
            platform_data: PlatformData::GitLab {
                source_project_id: 456,
                target_project_id: 123,
                target_ssh_url: None,
                target_http_url: None,
            },
        };
        // GitLab doesn't support prefixed branch names
        assert_eq!(info.prefixed_local_branch_name(), None);
    }

    #[test]
    fn test_extract_owner_from_url_ssh() {
        assert_eq!(
            extract_owner_from_url("git@gitlab.com:owner/repo.git"),
            Some("owner".to_string())
        );
        assert_eq!(
            extract_owner_from_url("git@github.com:contributor/repo.git"),
            Some("contributor".to_string())
        );
    }

    #[test]
    fn test_extract_owner_from_url_https() {
        assert_eq!(
            extract_owner_from_url("https://gitlab.com/owner/repo.git"),
            Some("owner".to_string())
        );
        assert_eq!(
            extract_owner_from_url("http://github.com/owner/repo.git"),
            Some("owner".to_string())
        );
    }

    #[test]
    fn test_extract_owner_from_url_invalid() {
        assert_eq!(extract_owner_from_url("invalid-url"), None);
        assert_eq!(extract_owner_from_url(""), None);
    }
}
