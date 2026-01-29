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
        /// Source project ID (used for deferred URL fetching).
        source_project_id: u64,
        /// Target project ID (used for deferred URL fetching).
        target_project_id: u64,
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
                    // For GitLab, try to extract namespace from fork_push_url
                    if let Some(url) = &self.fork_push_url
                        && let Some(namespace) = extract_namespace_from_url(url)
                    {
                        return format!("{}:{}", namespace, self.source_branch);
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

/// Extract namespace (owner or group/subgroup) from a git URL.
///
/// Handles both SSH (`git@host:namespace/repo.git`) and HTTPS
/// (`https://host/namespace/repo.git`) formats. Supports GitLab nested
/// namespaces like `group/subgroup/repo.git` → `group/subgroup`.
fn extract_namespace_from_url(url: &str) -> Option<String> {
    // SSH format: git@host:namespace/repo.git
    if let Some(path) = url.strip_prefix("git@").and_then(|s| s.split(':').nth(1)) {
        return extract_namespace_from_path(path);
    }
    // HTTPS format: https://host/namespace/repo.git
    if let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    {
        // Skip the host segment
        let path = rest.split('/').skip(1).collect::<Vec<_>>().join("/");
        return extract_namespace_from_path(&path);
    }
    None
}

/// Extract namespace from a path like `group/subgroup/repo.git`.
///
/// Returns everything except the last segment (repo name).
fn extract_namespace_from_path(path: &str) -> Option<String> {
    let path = path.strip_suffix(".git").unwrap_or(path);
    let segments: Vec<_> = path.split('/').collect();
    if segments.len() < 2 {
        return None;
    }
    // All segments except the last (which is the repo name)
    Some(segments[..segments.len() - 1].join("/"))
}

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
            },
        };
        // GitLab doesn't support prefixed branch names
        assert_eq!(info.prefixed_local_branch_name(), None);
    }

    #[test]
    fn test_extract_namespace_from_url_ssh() {
        assert_eq!(
            extract_namespace_from_url("git@gitlab.com:owner/repo.git"),
            Some("owner".to_string())
        );
        assert_eq!(
            extract_namespace_from_url("git@github.com:contributor/repo.git"),
            Some("contributor".to_string())
        );
    }

    #[test]
    fn test_extract_namespace_from_url_https() {
        assert_eq!(
            extract_namespace_from_url("https://gitlab.com/owner/repo.git"),
            Some("owner".to_string())
        );
        assert_eq!(
            extract_namespace_from_url("http://github.com/owner/repo.git"),
            Some("owner".to_string())
        );
    }

    #[test]
    fn test_extract_namespace_from_url_nested() {
        // GitLab nested namespaces
        assert_eq!(
            extract_namespace_from_url("git@gitlab.com:group/subgroup/repo.git"),
            Some("group/subgroup".to_string())
        );
        assert_eq!(
            extract_namespace_from_url("https://gitlab.com/group/subgroup/repo.git"),
            Some("group/subgroup".to_string())
        );
        // Even deeper nesting
        assert_eq!(
            extract_namespace_from_url("git@gitlab.com:org/team/project/repo.git"),
            Some("org/team/project".to_string())
        );
    }

    #[test]
    fn test_extract_namespace_from_url_invalid() {
        assert_eq!(extract_namespace_from_url("invalid-url"), None);
        assert_eq!(extract_namespace_from_url(""), None);
        // Just a repo name, no namespace
        assert_eq!(extract_namespace_from_url("git@gitlab.com:repo.git"), None);
    }
}
