//! Git remote URL parsing.
//!
//! Parses git remote URLs into structured components (host, owner, repo).
//! Supports HTTPS, SSH, and git@ URL formats.

/// Parsed git remote URL with host, owner, and repository components.
///
/// # Supported URL formats
///
/// - `https://<host>/<owner>/<repo>.git`
/// - `http://<host>/<owner>/<repo>.git`
/// - `git@<host>:<owner>/<repo>.git`
/// - `ssh://git@<host>/<owner>/<repo>.git`
/// - `ssh://<host>/<owner>/<repo>.git`
///
/// # Example
///
/// ```
/// use worktrunk::git::GitRemoteUrl;
///
/// let url = GitRemoteUrl::parse("git@github.com:owner/repo.git").unwrap();
/// assert_eq!(url.host(), "github.com");
/// assert_eq!(url.owner(), "owner");
/// assert_eq!(url.repo(), "repo");
/// assert_eq!(url.project_identifier(), "github.com/owner/repo");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRemoteUrl {
    host: String,
    owner: String,
    repo: String,
}

impl GitRemoteUrl {
    /// Parse a git remote URL into structured components.
    ///
    /// Returns `None` for malformed URLs or unsupported formats.
    pub fn parse(url: &str) -> Option<Self> {
        let url = url.trim();

        let (host, owner, repo_with_suffix) = if let Some(rest) = url.strip_prefix("https://") {
            // https://github.com/owner/repo.git
            let mut parts = rest.split('/');
            let host = parts.next()?;
            let owner = parts.next()?;
            let repo = parts.next()?;
            (host, owner, repo)
        } else if let Some(rest) = url.strip_prefix("http://") {
            // http://github.com/owner/repo.git
            let mut parts = rest.split('/');
            let host = parts.next()?;
            let owner = parts.next()?;
            let repo = parts.next()?;
            (host, owner, repo)
        } else if let Some(rest) = url.strip_prefix("ssh://") {
            // ssh://git@github.com/owner/repo.git or ssh://github.com/owner/repo.git
            // Note: URLs with ports (ssh://host:2222/...) are not supported here
            // as they don't fit the host/owner/repo model. They should be handled
            // as raw strings (project_identifier fallback).
            let without_user = rest.split('@').next_back()?;
            let mut parts = without_user.split('/');
            let host = parts.next()?;
            // If host contains a colon (port), this URL doesn't fit our model
            if host.contains(':') {
                return None;
            }
            let owner = parts.next()?;
            let repo = parts.next()?;
            (host, owner, repo)
        } else if let Some(rest) = url.strip_prefix("git@") {
            // git@github.com:owner/repo.git
            let (host, path) = rest.split_once(':')?;
            let mut parts = path.split('/');
            let owner = parts.next()?;
            let repo = parts.next()?;
            (host, owner, repo)
        } else {
            return None;
        };

        // Strip .git suffix from repo if present
        let repo = repo_with_suffix
            .strip_suffix(".git")
            .unwrap_or(repo_with_suffix);

        // Validate non-empty
        if host.is_empty() || owner.is_empty() || repo.is_empty() {
            return None;
        }

        Some(Self {
            host: host.to_string(),
            owner: owner.to_string(),
            repo: repo.to_string(),
        })
    }

    /// The hostname (e.g., "github.com", "gitlab.example.com").
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The repository owner or organization (e.g., "owner", "company-org").
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// The repository name without .git suffix (e.g., "repo").
    pub fn repo(&self) -> &str {
        &self.repo
    }

    /// Project identifier in "host/owner/repo" format.
    ///
    /// Used for tracking approved commands per project.
    pub fn project_identifier(&self) -> String {
        format!("{}/{}/{}", self.host, self.owner, self.repo)
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
pub fn parse_remote_owner(url: &str) -> Option<String> {
    GitRemoteUrl::parse(url).map(|u| u.owner().to_string())
}

/// Extract owner and repository name from a git remote URL.
pub fn parse_owner_repo(url: &str) -> Option<(String, String)> {
    GitRemoteUrl::parse(url).map(|u| (u.owner().to_string(), u.repo().to_string()))
}

/// Extract hostname from a git remote URL.
pub fn parse_remote_host(url: &str) -> Option<String> {
    GitRemoteUrl::parse(url).map(|u| u.host().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_https_urls() {
        let url = GitRemoteUrl::parse("https://github.com/owner/repo.git").unwrap();
        assert_eq!(url.host(), "github.com");
        assert_eq!(url.owner(), "owner");
        assert_eq!(url.repo(), "repo");
        assert_eq!(url.project_identifier(), "github.com/owner/repo");

        // Without .git suffix
        let url = GitRemoteUrl::parse("https://github.com/owner/repo").unwrap();
        assert_eq!(url.repo(), "repo");

        // With whitespace
        let url = GitRemoteUrl::parse("  https://github.com/owner/repo.git\n").unwrap();
        assert_eq!(url.owner(), "owner");
    }

    #[test]
    fn test_http_urls() {
        let url = GitRemoteUrl::parse("http://gitlab.internal.company.com/owner/repo.git").unwrap();
        assert_eq!(url.host(), "gitlab.internal.company.com");
        assert_eq!(url.owner(), "owner");
        assert_eq!(url.repo(), "repo");
    }

    #[test]
    fn test_git_at_urls() {
        let url = GitRemoteUrl::parse("git@github.com:owner/repo.git").unwrap();
        assert_eq!(url.host(), "github.com");
        assert_eq!(url.owner(), "owner");
        assert_eq!(url.repo(), "repo");

        // Without .git suffix
        let url = GitRemoteUrl::parse("git@github.com:owner/repo").unwrap();
        assert_eq!(url.repo(), "repo");

        // GitLab
        let url = GitRemoteUrl::parse("git@gitlab.example.com:owner/repo.git").unwrap();
        assert_eq!(url.host(), "gitlab.example.com");

        // Bitbucket
        let url = GitRemoteUrl::parse("git@bitbucket.org:owner/repo.git").unwrap();
        assert_eq!(url.host(), "bitbucket.org");
    }

    #[test]
    fn test_ssh_urls() {
        // With git@ user
        let url = GitRemoteUrl::parse("ssh://git@github.com/owner/repo.git").unwrap();
        assert_eq!(url.host(), "github.com");
        assert_eq!(url.owner(), "owner");
        assert_eq!(url.repo(), "repo");

        // Without user
        let url = GitRemoteUrl::parse("ssh://github.com/owner/repo.git").unwrap();
        assert_eq!(url.host(), "github.com");
        assert_eq!(url.owner(), "owner");
    }

    #[test]
    fn test_malformed_urls() {
        assert!(GitRemoteUrl::parse("").is_none());
        assert!(GitRemoteUrl::parse("https://github.com/").is_none());
        assert!(GitRemoteUrl::parse("https://github.com/owner/").is_none());
        assert!(GitRemoteUrl::parse("git@github.com:").is_none());
        assert!(GitRemoteUrl::parse("git@github.com:owner/").is_none());
        assert!(GitRemoteUrl::parse("ftp://github.com/owner/repo.git").is_none());
    }

    #[test]
    fn test_org_repos() {
        let url = GitRemoteUrl::parse("https://github.com/company-org/project.git").unwrap();
        assert_eq!(url.owner(), "company-org");
        assert_eq!(url.repo(), "project");
    }

    #[test]
    fn test_parse_remote_owner() {
        assert_eq!(
            parse_remote_owner("https://github.com/max-sixty/worktrunk.git"),
            Some("max-sixty".to_string())
        );
        assert_eq!(
            parse_remote_owner("  https://github.com/owner/repo\n"),
            Some("owner".to_string())
        );
        assert_eq!(
            parse_remote_owner("git@github.com:max-sixty/worktrunk.git"),
            Some("max-sixty".to_string())
        );
        assert_eq!(
            parse_remote_owner("ssh://git@github.com/owner/repo.git"),
            Some("owner".to_string())
        );
        assert_eq!(
            parse_remote_owner("ssh://github.com/owner/repo.git"),
            Some("owner".to_string())
        );
        assert_eq!(
            parse_remote_owner("https://gitlab.com/owner/repo.git"),
            Some("owner".to_string())
        );
        assert_eq!(
            parse_remote_owner("https://gitlab.example.com/owner/repo.git"),
            Some("owner".to_string())
        );
        assert_eq!(
            parse_remote_owner("git@gitlab.com:owner/repo.git"),
            Some("owner".to_string())
        );
        assert_eq!(
            parse_remote_owner("https://bitbucket.org/owner/repo.git"),
            Some("owner".to_string())
        );
        assert_eq!(
            parse_remote_owner("git@bitbucket.org:owner/repo.git"),
            Some("owner".to_string())
        );
        assert_eq!(
            parse_remote_owner("https://github.com/company-org/project.git"),
            Some("company-org".to_string())
        );
        assert_eq!(
            parse_remote_owner("http://github.com/owner/repo.git"),
            Some("owner".to_string())
        );
        assert_eq!(parse_remote_owner("https://github.com/"), None);
        assert_eq!(parse_remote_owner("git@github.com:"), None);
        assert_eq!(parse_remote_owner(""), None);
    }

    #[test]
    fn test_parse_remote_host() {
        assert_eq!(
            parse_remote_host("https://gitlab.com/owner/repo.git"),
            Some("gitlab.com".to_string())
        );
        assert_eq!(
            parse_remote_host("https://gitlab.example.com/owner/repo.git"),
            Some("gitlab.example.com".to_string())
        );
        assert_eq!(
            parse_remote_host("  https://github.com/owner/repo\n"),
            Some("github.com".to_string())
        );
        assert_eq!(
            parse_remote_host("http://gitlab.internal.company.com/owner/repo.git"),
            Some("gitlab.internal.company.com".to_string())
        );
        assert_eq!(
            parse_remote_host("git@gitlab.com:owner/repo.git"),
            Some("gitlab.com".to_string())
        );
        assert_eq!(
            parse_remote_host("git@gitlab.example.com:owner/repo.git"),
            Some("gitlab.example.com".to_string())
        );
        assert_eq!(
            parse_remote_host("ssh://git@gitlab.example.com/owner/repo.git"),
            Some("gitlab.example.com".to_string())
        );
        assert_eq!(
            parse_remote_host("ssh://gitlab.example.com/owner/repo.git"),
            Some("gitlab.example.com".to_string())
        );
        assert_eq!(parse_remote_host("https://"), None);
        assert_eq!(parse_remote_host("git@"), None);
        assert_eq!(parse_remote_host(""), None);
    }

    #[test]
    fn test_parse_owner_repo() {
        assert_eq!(
            parse_owner_repo("https://github.com/owner/repo.git"),
            Some(("owner".to_string(), "repo".to_string()))
        );
        assert_eq!(
            parse_owner_repo("https://github.com/owner/repo"),
            Some(("owner".to_string(), "repo".to_string()))
        );
        assert_eq!(
            parse_owner_repo("  https://github.com/owner/repo.git\n"),
            Some(("owner".to_string(), "repo".to_string()))
        );
        assert_eq!(
            parse_owner_repo("git@github.com:owner/repo.git"),
            Some(("owner".to_string(), "repo".to_string()))
        );
        assert_eq!(
            parse_owner_repo("git@github.com:owner/repo"),
            Some(("owner".to_string(), "repo".to_string()))
        );
        assert_eq!(
            parse_owner_repo("ssh://git@github.com/owner/repo.git"),
            Some(("owner".to_string(), "repo".to_string()))
        );
        assert_eq!(
            parse_owner_repo("https://gitlab.com/owner/repo.git"),
            Some(("owner".to_string(), "repo".to_string()))
        );
        assert_eq!(parse_owner_repo("https://github.com/owner/"), None);
        assert_eq!(parse_owner_repo("git@github.com:owner/"), None);
        assert_eq!(parse_owner_repo(""), None);
    }

    #[test]
    fn test_project_identifier() {
        let cases = [
            (
                "https://github.com/max-sixty/worktrunk.git",
                "github.com/max-sixty/worktrunk",
            ),
            ("git@github.com:owner/repo.git", "github.com/owner/repo"),
            (
                "ssh://git@gitlab.example.com/org/project.git",
                "gitlab.example.com/org/project",
            ),
        ];

        for (input, expected) in cases {
            let url = GitRemoteUrl::parse(input).unwrap();
            assert_eq!(url.project_identifier(), expected, "input: {input}");
        }
    }
}
