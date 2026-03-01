//! Git remote URL parsing.
//!
//! Parses git remote URLs into structured components (host, owner, repo).
//! Supports HTTPS, SSH, and git@ URL formats.

/// Parsed git remote URL with host, owner (namespace), and repository components.
///
/// # Supported URL formats
///
/// - `https://<host>/<namespace>/<repo>.git`
/// - `http://<host>/<namespace>/<repo>.git`
/// - `git://<host>/<namespace>/<repo>.git`
/// - `git@<host>:<namespace>/<repo>.git`
/// - `ssh://git@<host>/<namespace>/<repo>.git`
/// - `ssh://git@<host>:<port>/<namespace>/<repo>.git`
/// - `ssh://<host>/<namespace>/<repo>.git`
/// - `ssh://<host>:<port>/<namespace>/<repo>.git`
///
/// # Nested groups (GitLab subgroups)
///
/// GitLab supports arbitrary nesting depth: `gitlab.com/group/subgroup/subsubgroup/repo`
/// The parser treats everything before the last path segment as the namespace:
/// - `owner()` returns `"group/subgroup/subsubgroup"`
/// - `repo()` returns `"repo"`
/// - `project_identifier()` returns `"gitlab.com/group/subgroup/subsubgroup/repo"`
///
/// This ensures unique project identifiers for approval tracking security.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRemoteUrl {
    host: String,
    /// Full namespace path (may include `/` for nested groups)
    owner: String,
    repo: String,
}

/// Split a path into namespace and repo components.
///
/// Takes everything before the last segment as namespace, last segment as repo.
/// Handles trailing `.git` suffix and empty segments.
///
/// Returns `None` if there aren't at least 2 non-empty path segments.
fn split_namespace_repo(path: &str) -> Option<(String, String)> {
    // Filter out empty segments (handles trailing slashes, double slashes)
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    if segments.len() < 2 {
        return None;
    }

    // Last segment is repo (possibly with .git suffix)
    let repo_with_suffix = segments.last()?;
    let repo = repo_with_suffix
        .strip_suffix(".git")
        .unwrap_or(repo_with_suffix);

    // Everything else is the namespace
    let namespace = segments[..segments.len() - 1].join("/");

    if namespace.is_empty() || repo.is_empty() {
        return None;
    }

    Some((namespace, repo.to_string()))
}

impl GitRemoteUrl {
    /// Parse a git remote URL into structured components.
    ///
    /// Returns `None` for malformed URLs or unsupported formats.
    ///
    /// Handles GitLab nested groups by treating all path segments except the last
    /// as the namespace. This ensures unique project identifiers for approval security.
    pub fn parse(url: &str) -> Option<Self> {
        let url = url.trim();

        let (host, namespace, repo) = if let Some(rest) = url.strip_prefix("https://") {
            // https://github.com/owner/repo.git
            // https://gitlab.com/group/subgroup/repo.git
            let (host, path) = rest.split_once('/')?;
            let (namespace, repo) = split_namespace_repo(path)?;
            (host, namespace, repo)
        } else if let Some(rest) = url.strip_prefix("http://") {
            // http://github.com/owner/repo.git
            let (host, path) = rest.split_once('/')?;
            let (namespace, repo) = split_namespace_repo(path)?;
            (host, namespace, repo)
        } else if let Some(rest) = url.strip_prefix("git://") {
            // git://github.com/owner/repo.git
            let (host, path) = rest.split_once('/')?;
            let (namespace, repo) = split_namespace_repo(path)?;
            (host, namespace, repo)
        } else if let Some(rest) = url.strip_prefix("ssh://") {
            // ssh://git@github.com/owner/repo.git or ssh://github.com/owner/repo.git
            // ssh://git@host:port/owner/repo.git (port is stripped — irrelevant to project identity)
            let without_user = rest.split('@').next_back()?;
            let (host_with_port, path) = without_user.split_once('/')?;
            // Strip port from host (e.g., "gitlab.internal:2222" → "gitlab.internal")
            let host = host_with_port.split(':').next().unwrap_or(host_with_port);
            let (namespace, repo) = split_namespace_repo(path)?;
            (host, namespace, repo)
        } else if let Some(rest) = url.strip_prefix("git@") {
            // git@github.com:owner/repo.git
            // git@gitlab.com:group/subgroup/repo.git
            let (host, path) = rest.split_once(':')?;
            let (namespace, repo) = split_namespace_repo(path)?;
            (host, namespace, repo)
        } else {
            return None;
        };

        // Validate non-empty host
        if host.is_empty() {
            return None;
        }

        Some(Self {
            host: host.to_string(),
            owner: namespace,
            repo,
        })
    }

    /// The host (e.g., "github.com", "gitlab.example.com").
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The repository owner, organization, or namespace path.
    ///
    /// For nested GitLab groups, returns the full namespace (e.g., "group/subgroup/team").
    /// For standard repos, returns the owner (e.g., "owner", "company-org").
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

    /// Check if this URL points to a GitHub host.
    ///
    /// Matches github.com and GitHub Enterprise hosts (e.g., github.mycompany.com).
    pub fn is_github(&self) -> bool {
        self.host.to_ascii_lowercase().contains("github")
    }

    /// Check if this URL points to a GitLab host.
    ///
    /// Matches gitlab.com and self-hosted GitLab instances (e.g., gitlab.example.com).
    pub fn is_gitlab(&self) -> bool {
        self.host.to_ascii_lowercase().contains("gitlab")
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
        assert_eq!(
            url.project_identifier(),
            "gitlab.internal.company.com/owner/repo"
        );
    }

    #[test]
    fn test_git_at_urls() {
        let url = GitRemoteUrl::parse("git@github.com:owner/repo.git").unwrap();
        assert_eq!(url.project_identifier(), "github.com/owner/repo");

        // Without .git suffix
        let url = GitRemoteUrl::parse("git@github.com:owner/repo").unwrap();
        assert_eq!(url.repo(), "repo");

        // GitLab
        let url = GitRemoteUrl::parse("git@gitlab.example.com:owner/repo.git").unwrap();
        assert!(url.project_identifier().starts_with("gitlab.example.com/"));

        // Bitbucket
        let url = GitRemoteUrl::parse("git@bitbucket.org:owner/repo.git").unwrap();
        assert!(url.project_identifier().starts_with("bitbucket.org/"));
    }

    #[test]
    fn test_ssh_urls() {
        // With git@ user
        let url = GitRemoteUrl::parse("ssh://git@github.com/owner/repo.git").unwrap();
        assert_eq!(url.project_identifier(), "github.com/owner/repo");

        // Without user
        let url = GitRemoteUrl::parse("ssh://github.com/owner/repo.git").unwrap();
        assert!(url.project_identifier().starts_with("github.com/"));
        assert_eq!(url.owner(), "owner");
    }

    #[test]
    fn test_ssh_urls_with_ports() {
        // Standard SSH with port
        let url = GitRemoteUrl::parse("ssh://git@host:22/owner/repo.git").unwrap();
        assert_eq!(url.host(), "host");
        assert_eq!(url.owner(), "owner");
        assert_eq!(url.repo(), "repo");
        assert_eq!(url.project_identifier(), "host/owner/repo");

        // Without user
        let url = GitRemoteUrl::parse("ssh://host:2222/owner/repo.git").unwrap();
        assert_eq!(url.host(), "host");
        assert_eq!(url.owner(), "owner");
        assert_eq!(url.repo(), "repo");

        // Nested groups with port
        let url =
            GitRemoteUrl::parse("ssh://git@gitlab.internal:2222/group/subgroup/repo.git").unwrap();
        assert_eq!(url.host(), "gitlab.internal");
        assert_eq!(url.owner(), "group/subgroup");
        assert_eq!(url.repo(), "repo");
        assert_eq!(
            url.project_identifier(),
            "gitlab.internal/group/subgroup/repo"
        );

        // Port is stripped — same project identity as without port
        let with_port = GitRemoteUrl::parse("ssh://git@host:2222/owner/repo.git").unwrap();
        let without_port = GitRemoteUrl::parse("ssh://git@host/owner/repo.git").unwrap();
        assert_eq!(
            with_port.project_identifier(),
            without_port.project_identifier(),
            "Port is a transport detail — same project identity"
        );
    }

    #[test]
    fn test_git_protocol_urls() {
        let url = GitRemoteUrl::parse("git://github.com/owner/repo.git").unwrap();
        assert_eq!(url.project_identifier(), "github.com/owner/repo");
        assert!(url.is_github());

        let url = GitRemoteUrl::parse("git://gitlab.example.com/owner/repo.git").unwrap();
        assert!(url.is_gitlab());
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

    #[test]
    fn test_is_github() {
        // GitHub.com
        assert!(
            GitRemoteUrl::parse("https://github.com/owner/repo.git")
                .unwrap()
                .is_github()
        );
        assert!(
            GitRemoteUrl::parse("git@github.com:owner/repo.git")
                .unwrap()
                .is_github()
        );
        assert!(
            GitRemoteUrl::parse("ssh://git@github.com/owner/repo.git")
                .unwrap()
                .is_github()
        );

        // GitHub Enterprise
        assert!(
            GitRemoteUrl::parse("https://github.mycompany.com/owner/repo.git")
                .unwrap()
                .is_github()
        );

        // Not GitHub
        assert!(
            !GitRemoteUrl::parse("https://gitlab.com/owner/repo.git")
                .unwrap()
                .is_github()
        );
        assert!(
            !GitRemoteUrl::parse("https://bitbucket.org/owner/repo.git")
                .unwrap()
                .is_github()
        );
    }

    #[test]
    fn test_is_gitlab() {
        // GitLab.com
        assert!(
            GitRemoteUrl::parse("https://gitlab.com/owner/repo.git")
                .unwrap()
                .is_gitlab()
        );
        assert!(
            GitRemoteUrl::parse("git@gitlab.com:owner/repo.git")
                .unwrap()
                .is_gitlab()
        );

        // Self-hosted GitLab
        assert!(
            GitRemoteUrl::parse("https://gitlab.example.com/owner/repo.git")
                .unwrap()
                .is_gitlab()
        );

        // Not GitLab
        assert!(
            !GitRemoteUrl::parse("https://github.com/owner/repo.git")
                .unwrap()
                .is_gitlab()
        );
        assert!(
            !GitRemoteUrl::parse("https://bitbucket.org/owner/repo.git")
                .unwrap()
                .is_gitlab()
        );
    }

    // Security-critical tests for nested GitLab groups.
    //
    // GitLab supports nested groups (subgroups) with arbitrary depth:
    // https://docs.gitlab.com/ee/user/group/subgroups/
    //
    // For approval security, project_identifier() MUST be unique per repo.
    // Two repos in the same parent group must have different identifiers:
    // - gitlab.com/group/subgroup/repo1 ≠ gitlab.com/group/subgroup/repo2
    //
    // If parsing fails or truncates the path, approvals for one repo
    // could apply to another, bypassing security.

    #[test]
    fn test_nested_gitlab_groups_https() {
        // Single subgroup
        let url = GitRemoteUrl::parse("https://gitlab.com/group/subgroup/repo.git").unwrap();
        assert_eq!(url.host(), "gitlab.com");
        assert_eq!(url.owner(), "group/subgroup");
        assert_eq!(url.repo(), "repo");
        assert_eq!(
            url.project_identifier(),
            "gitlab.com/group/subgroup/repo",
            "Security: nested group must be fully preserved in identifier"
        );

        // Multiple levels of nesting
        let url =
            GitRemoteUrl::parse("https://gitlab.com/org/team/project/subproject/repo.git").unwrap();
        assert_eq!(url.host(), "gitlab.com");
        assert_eq!(url.owner(), "org/team/project/subproject");
        assert_eq!(url.repo(), "repo");
        assert_eq!(
            url.project_identifier(),
            "gitlab.com/org/team/project/subproject/repo"
        );

        // Without .git suffix
        let url = GitRemoteUrl::parse("https://gitlab.com/group/subgroup/repo").unwrap();
        assert_eq!(url.repo(), "repo");
        assert_eq!(url.owner(), "group/subgroup");
    }

    #[test]
    fn test_nested_gitlab_groups_ssh() {
        // git@ format with subgroup
        let url = GitRemoteUrl::parse("git@gitlab.com:group/subgroup/repo.git").unwrap();
        assert_eq!(url.host(), "gitlab.com");
        assert_eq!(url.owner(), "group/subgroup");
        assert_eq!(url.repo(), "repo");
        assert_eq!(
            url.project_identifier(),
            "gitlab.com/group/subgroup/repo",
            "Security: SSH URLs must handle nested groups identically to HTTPS"
        );

        // ssh:// format with subgroup
        let url = GitRemoteUrl::parse("ssh://git@gitlab.com/group/subgroup/repo.git").unwrap();
        assert_eq!(url.owner(), "group/subgroup");
        assert_eq!(url.repo(), "repo");

        // Deeply nested
        let url = GitRemoteUrl::parse("git@gitlab.com:a/b/c/d/repo.git").unwrap();
        assert_eq!(url.owner(), "a/b/c/d");
        assert_eq!(url.repo(), "repo");
    }

    #[test]
    fn test_nested_groups_self_hosted() {
        // Self-hosted GitLab with subgroups
        let url =
            GitRemoteUrl::parse("https://gitlab.mycompany.com/team/frontend/repo.git").unwrap();
        assert_eq!(url.host(), "gitlab.mycompany.com");
        assert_eq!(url.owner(), "team/frontend");
        assert_eq!(url.repo(), "repo");

        let url = GitRemoteUrl::parse("git@gitlab.internal:org/dept/project/repo.git").unwrap();
        assert_eq!(url.owner(), "org/dept/project");
        assert_eq!(url.repo(), "repo");
    }

    #[test]
    fn test_nested_groups_security_uniqueness() {
        // CRITICAL: Two repos in the same parent group must have different identifiers
        let repo1 = GitRemoteUrl::parse("https://gitlab.com/company/team/repo-a.git").unwrap();
        let repo2 = GitRemoteUrl::parse("https://gitlab.com/company/team/repo-b.git").unwrap();

        assert_ne!(
            repo1.project_identifier(),
            repo2.project_identifier(),
            "Security: Different repos MUST have different project identifiers"
        );

        // The parent path alone is not sufficient
        assert_eq!(repo1.owner(), "company/team");
        assert_eq!(repo2.owner(), "company/team");
        assert_ne!(repo1.repo(), repo2.repo());
    }

    #[test]
    fn test_parse_remote_owner_nested() {
        // parse_remote_owner should return the full namespace
        assert_eq!(
            parse_remote_owner("https://gitlab.com/group/subgroup/repo.git"),
            Some("group/subgroup".to_string())
        );
        assert_eq!(
            parse_remote_owner("git@gitlab.com:org/team/project/repo.git"),
            Some("org/team/project".to_string())
        );
    }

    #[test]
    fn test_parse_owner_repo_nested() {
        assert_eq!(
            parse_owner_repo("https://gitlab.com/group/subgroup/repo.git"),
            Some(("group/subgroup".to_string(), "repo".to_string()))
        );
        assert_eq!(
            parse_owner_repo("git@gitlab.com:a/b/c/repo.git"),
            Some(("a/b/c".to_string(), "repo".to_string()))
        );
    }

    // Additional security edge cases for nested groups

    #[test]
    fn test_nested_groups_edge_cases() {
        // Maximum reasonable nesting depth
        let url = GitRemoteUrl::parse("https://gitlab.com/a/b/c/d/e/f/g/repo.git").unwrap();
        assert_eq!(url.owner(), "a/b/c/d/e/f/g");
        assert_eq!(url.repo(), "repo");
        assert_eq!(url.project_identifier(), "gitlab.com/a/b/c/d/e/f/g/repo");

        // Repo name with dots (valid GitLab repo names)
        let url = GitRemoteUrl::parse("https://gitlab.com/group/repo.name.git").unwrap();
        assert_eq!(url.owner(), "group");
        assert_eq!(url.repo(), "repo.name");

        // Repo name with hyphens and underscores
        let url =
            GitRemoteUrl::parse("https://gitlab.com/my-group/sub_group/my-repo_v2.git").unwrap();
        assert_eq!(url.owner(), "my-group/sub_group");
        assert_eq!(url.repo(), "my-repo_v2");
    }

    #[test]
    fn test_nested_groups_similar_paths_are_distinct() {
        // Security: Paths that look similar must have distinct identifiers
        // This tests against potential truncation or normalization bugs

        let cases = [
            // Sibling repos in nested group
            (
                "https://gitlab.com/org/team/repo-a.git",
                "gitlab.com/org/team/repo-a",
            ),
            (
                "https://gitlab.com/org/team/repo-b.git",
                "gitlab.com/org/team/repo-b",
            ),
            // Different nesting levels with similar names
            ("https://gitlab.com/org/repo.git", "gitlab.com/org/repo"),
            (
                "https://gitlab.com/org/team/repo.git",
                "gitlab.com/org/team/repo",
            ),
            (
                "https://gitlab.com/org/team/sub/repo.git",
                "gitlab.com/org/team/sub/repo",
            ),
            // Group name matches repo name at different level
            (
                "https://gitlab.com/project/repo.git",
                "gitlab.com/project/repo",
            ),
            (
                "https://gitlab.com/repo/project.git",
                "gitlab.com/repo/project",
            ),
        ];

        let identifiers: Vec<_> = cases
            .iter()
            .map(|(url, _)| GitRemoteUrl::parse(url).unwrap().project_identifier())
            .collect();

        // All identifiers must be unique
        for (i, id) in identifiers.iter().enumerate() {
            assert_eq!(
                id, cases[i].1,
                "URL {} should produce identifier {}",
                cases[i].0, cases[i].1
            );
        }

        // Verify no duplicates
        let mut sorted = identifiers.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            identifiers.len(),
            sorted.len(),
            "All project identifiers must be unique"
        );
    }

    #[test]
    fn test_nested_groups_malformed_paths() {
        // These should fail to parse (security: don't accept garbage)

        // Missing repo (only namespace)
        assert!(GitRemoteUrl::parse("https://gitlab.com/group/").is_none());
        assert!(GitRemoteUrl::parse("git@gitlab.com:group/").is_none());

        // Just host
        assert!(GitRemoteUrl::parse("https://gitlab.com/").is_none());
        assert!(GitRemoteUrl::parse("git@gitlab.com:").is_none());

        // Double slashes shouldn't create empty segments
        let url = GitRemoteUrl::parse("https://gitlab.com/group//subgroup/repo.git");
        // Should either fail or treat as group/subgroup/repo (no empty segment)
        if let Some(parsed) = url {
            assert!(!parsed.owner().contains("//"));
            assert!(!parsed.owner().is_empty());
        }

        // Repo named exactly ".git" - stripping suffix produces empty string
        // This should fail to parse (repo would be empty)
        assert!(GitRemoteUrl::parse("https://gitlab.com/group/.git").is_none());

        // But a repo named ".git.git" strips to ".git" which is valid (unusual but possible)
        let url = GitRemoteUrl::parse("https://gitlab.com/group/.git.git").unwrap();
        assert_eq!(url.repo(), ".git");
    }

    #[test]
    fn test_all_url_formats_handle_nested_groups_identically() {
        // Security: All URL formats for the same repo must produce identical identifiers
        let formats = [
            "https://gitlab.com/group/subgroup/repo.git",
            "https://gitlab.com/group/subgroup/repo",
            "git@gitlab.com:group/subgroup/repo.git",
            "git@gitlab.com:group/subgroup/repo",
            "ssh://git@gitlab.com/group/subgroup/repo.git",
            "ssh://gitlab.com/group/subgroup/repo.git",
            "git://gitlab.com/group/subgroup/repo.git",
            "http://gitlab.com/group/subgroup/repo.git",
        ];

        let expected_identifier = "gitlab.com/group/subgroup/repo";

        for url in formats {
            let parsed =
                GitRemoteUrl::parse(url).unwrap_or_else(|| panic!("Failed to parse URL: {url}"));
            assert_eq!(
                parsed.project_identifier(),
                expected_identifier,
                "URL format '{url}' must produce consistent identifier"
            );
            assert_eq!(parsed.owner(), "group/subgroup");
            assert_eq!(parsed.repo(), "repo");
        }
    }

    // =========================================================================
    // ADVERSARIAL SECURITY TESTS: Identifier Collision Attacks
    // =========================================================================
    //
    // These tests verify that an attacker cannot craft a URL that produces
    // the same project_identifier as a different repository they don't control.
    //
    // Attack model: Attacker controls repo A, wants approvals from repo A to
    // apply to repo B (which they don't control).

    #[test]
    fn test_adversarial_different_nesting_levels_no_collision() {
        // Attack: Can two repos at different nesting levels collide?
        //
        // Scenario: Attacker controls gitlab.com/a-b/c/repo
        // Target victim: gitlab.com/a/b/c/repo
        // These should NEVER collide.

        let attacker = GitRemoteUrl::parse("https://gitlab.com/a-b/c/repo.git").unwrap();
        let victim = GitRemoteUrl::parse("https://gitlab.com/a/b/c/repo.git").unwrap();

        assert_ne!(
            attacker.project_identifier(),
            victim.project_identifier(),
            "CRITICAL: Different group structures must have different identifiers"
        );

        // Verify the actual identifiers
        assert_eq!(attacker.project_identifier(), "gitlab.com/a-b/c/repo");
        assert_eq!(victim.project_identifier(), "gitlab.com/a/b/c/repo");
    }

    #[test]
    fn test_adversarial_host_spoofing_no_collision() {
        // Attack: Use a subdomain that looks like a different host

        // gitlab.com.evil.com/owner/repo vs gitlab.com/owner/repo
        let evil_host = GitRemoteUrl::parse("https://gitlab.com.evil.com/owner/repo.git").unwrap();
        let real_host = GitRemoteUrl::parse("https://gitlab.com/owner/repo.git").unwrap();

        assert_ne!(
            evil_host.project_identifier(),
            real_host.project_identifier(),
            "Different hosts must produce different identifiers"
        );

        assert_eq!(evil_host.host(), "gitlab.com.evil.com");
        assert_eq!(real_host.host(), "gitlab.com");
    }

    #[test]
    fn test_adversarial_case_sensitivity() {
        // Attack: Use different casing to create "different" repos that might
        // collide after normalization.

        // gitlab.com/Owner/Repo vs gitlab.com/owner/repo
        let uppercase = GitRemoteUrl::parse("https://gitlab.com/Owner/Repo.git").unwrap();
        let lowercase = GitRemoteUrl::parse("https://gitlab.com/owner/repo.git").unwrap();

        // These SHOULD be different identifiers (case-sensitive)
        // GitLab/GitHub treat these as different repos
        assert_ne!(
            uppercase.project_identifier(),
            lowercase.project_identifier(),
            "Case differences must produce different identifiers"
        );
    }

    #[test]
    fn test_adversarial_git_suffix_manipulation() {
        // Attack: Use .git.git or other suffix manipulations

        let double_git = GitRemoteUrl::parse("https://gitlab.com/owner/repo.git.git").unwrap();
        let single_git = GitRemoteUrl::parse("https://gitlab.com/owner/repo.git").unwrap();
        let no_git = GitRemoteUrl::parse("https://gitlab.com/owner/repo").unwrap();

        // .git.git -> strip ONE .git -> repo is "repo.git"
        assert_eq!(double_git.repo(), "repo.git");
        assert_eq!(single_git.repo(), "repo");
        assert_eq!(no_git.repo(), "repo");

        // single_git and no_git should match (same repo)
        assert_eq!(single_git.project_identifier(), no_git.project_identifier());

        // double_git is actually a different repo (named "repo.git")
        assert_ne!(
            double_git.project_identifier(),
            single_git.project_identifier()
        );
    }

    #[test]
    fn test_adversarial_ssh_user_injection() {
        // CRITICAL: Attack via SSH user field with @ character
        //
        // ssh://user@legitimate.com@attacker.com/owner/repo.git
        //
        // The parser uses: rest.split('@').next_back()
        // This takes EVERYTHING after the LAST @
        //
        // Input: "user@legitimate.com@attacker.com/owner/repo.git"
        // Split by @: ["user", "legitimate.com", "attacker.com/owner/repo.git"]
        // next_back(): "attacker.com/owner/repo.git"
        // Host becomes: "attacker.com"
        //
        // This means ssh://git@victim.com@attacker.com/owner/repo.git
        // produces host = "attacker.com", not "victim.com"!

        // The URL parses successfully - last @ wins for user/host separation
        let parsed =
            GitRemoteUrl::parse("ssh://git@legitimate.com@attacker.com/owner/repo.git").unwrap();

        // The parser extracts host from AFTER the last @
        // So the host is "attacker.com", not "legitimate.com"
        // This is consistent behavior - the URL is malformed but parseable
        assert_eq!(
            parsed.host(),
            "attacker.com",
            "SSH URLs with multiple @ signs: last @ determines host"
        );

        // The identifier correctly reflects attacker.com
        assert!(parsed.project_identifier().starts_with("attacker.com/"));
    }

    #[test]
    fn test_adversarial_ssh_at_in_path() {
        // What if @ appears in the path (namespace)?
        // ssh://git@host.com/org@company/repo.git
        //
        // The parser uses split('@').next_back() which takes everything after
        // the LAST @. So "git@host.com/org@company/repo.git" splits as:
        // ["git", "host.com/org", "company/repo.git"]
        // next_back() returns "company/repo.git"
        // split_once('/') gives host="company", path="repo.git"
        // split_namespace_repo("repo.git") has only 1 segment, returns None
        //
        // This URL is rejected - @ in namespace breaks ssh:// parsing

        assert!(
            GitRemoteUrl::parse("ssh://git@host.com/org@company/repo.git").is_none(),
            "SSH URLs with @ in path after host are rejected (ambiguous parsing)"
        );

        // However, https:// handles @ in namespace correctly (no user@ prefix)
        let https_with_at = GitRemoteUrl::parse("https://host.com/org@company/repo.git").unwrap();
        assert_eq!(https_with_at.owner(), "org@company");
        assert_eq!(https_with_at.repo(), "repo");
    }

    #[test]
    fn test_adversarial_empty_user_ssh() {
        // ssh://user@/owner/repo.git - empty host after user@
        // After split('@').next_back(): "/owner/repo.git"
        // split_once('/'): host="", path="owner/repo.git"
        // Empty host is rejected
        assert!(
            GitRemoteUrl::parse("ssh://user@/owner/repo.git").is_none(),
            "Empty host should be rejected"
        );

        // ssh://@host.com/owner/repo.git - empty user (@ with nothing before it)
        // After split('@').next_back(): "host.com/owner/repo.git"
        // This parses correctly - the empty user is effectively ignored
        let parsed = GitRemoteUrl::parse("ssh://@host.com/owner/repo.git").unwrap();
        assert_eq!(parsed.host(), "host.com");
        assert_eq!(parsed.owner(), "owner");
        assert_eq!(parsed.repo(), "repo");
    }

    #[test]
    fn test_adversarial_empty_segment_normalization() {
        // Attack: Use empty segments to shift parsing
        // gitlab.com/a//b/repo (double slash)

        let with_double_slash = GitRemoteUrl::parse("https://gitlab.com/a//b/repo.git").unwrap();
        let normal = GitRemoteUrl::parse("https://gitlab.com/a/b/repo.git").unwrap();

        // Empty segments are filtered out, so these produce the same identifier
        // This is SAFE because it's the same logical repo
        assert_eq!(
            with_double_slash.project_identifier(),
            normal.project_identifier(),
            "Empty segment normalization should produce consistent identifiers"
        );

        // Verify no empty segments in owner
        assert!(!with_double_slash.owner().contains("//"));
    }

    #[test]
    fn test_adversarial_dot_segments() {
        // Attack: Use . or .. segments to manipulate path
        // gitlab.com/owner/./repo vs gitlab.com/owner/repo
        //
        // The parser treats "." as a literal path segment (no special handling).
        // This is safe because it produces a DIFFERENT identifier.

        let with_dot = GitRemoteUrl::parse("https://gitlab.com/owner/./repo.git").unwrap();
        let normal = GitRemoteUrl::parse("https://gitlab.com/owner/repo.git").unwrap();

        // "." is preserved as literal segment - different identifier, no collision
        assert_eq!(with_dot.owner(), "owner/.");
        assert_eq!(with_dot.repo(), "repo");
        assert_ne!(
            with_dot.project_identifier(),
            normal.project_identifier(),
            "Literal . segment produces different identifier (no collision)"
        );
    }

    #[test]
    fn test_adversarial_parent_traversal() {
        // Attack: Use .. to escape namespace
        // gitlab.com/owner/../victim/repo -> should NOT resolve to gitlab.com/victim/repo
        //
        // The parser treats ".." as a literal path segment (no directory traversal).
        // This is SAFE because it produces a different identifier than the "escaped" path.

        let with_dotdot =
            GitRemoteUrl::parse("https://gitlab.com/owner/../victim/repo.git").unwrap();
        let victim = GitRemoteUrl::parse("https://gitlab.com/victim/repo.git").unwrap();

        // ".." is treated literally, not as parent directory
        assert_eq!(with_dotdot.owner(), "owner/../victim");
        assert!(
            with_dotdot.project_identifier().contains(".."),
            "Parent traversal (..) must be treated literally"
        );

        // No collision with the "target" path
        assert_ne!(
            with_dotdot.project_identifier(),
            victim.project_identifier(),
            "Path traversal attack must not collide with target"
        );
    }

    #[test]
    fn test_adversarial_unicode_lookalikes() {
        // Attack: Use Unicode characters that look like ASCII

        let normal = GitRemoteUrl::parse("https://gitlab.com/owner/repo.git").unwrap();

        // Using Greek omicron (\u{03BF}) instead of ASCII 'o'
        let with_greek_o = GitRemoteUrl::parse("https://gitlab.com/\u{03BF}wner/repo.git").unwrap();

        assert_ne!(
            normal.project_identifier(),
            with_greek_o.project_identifier(),
            "Unicode lookalikes must produce different identifiers"
        );
    }

    #[test]
    fn test_adversarial_url_encoded_slash() {
        // Attack: Can a repo name containing "/" (URL-encoded as %2F) collide
        // with a nested group path?
        //
        // Note: GitLab does NOT allow "/" in repo names.
        // But test parser behavior with URL-encoded content.
        //
        // The parser treats %2F literally (doesn't decode it).
        // This is the SAFE behavior - no collision possible.

        let parsed = GitRemoteUrl::parse("https://gitlab.com/attacker/evil%2Frepo.git").unwrap();

        // The %2F stays in the repo name, so no collision with nested paths
        assert_eq!(parsed.owner(), "attacker");
        assert_eq!(parsed.repo(), "evil%2Frepo");

        // No collision with what the attacker might want to target
        let target = GitRemoteUrl::parse("https://gitlab.com/attacker/evil/repo.git").unwrap();
        assert_ne!(
            parsed.project_identifier(),
            target.project_identifier(),
            "URL-encoded slash must not collide with actual nested path"
        );
    }

    #[test]
    fn test_adversarial_comprehensive_uniqueness() {
        // Exhaustive test: Many URLs that should all have DIFFERENT identifiers

        let urls = [
            "https://gitlab.com/a/repo.git",
            "https://gitlab.com/a/b/repo.git",
            "https://gitlab.com/a/b/c/repo.git",
            "https://gitlab.com/a-b/repo.git",
            "https://gitlab.com/a/b-repo.git",
            "https://gitlab.com/A/repo.git", // case difference
            "https://gitlab.com/a/Repo.git", // case difference
            "https://github.com/a/repo.git", // different host
            "https://gitlab.example.com/a/repo.git", // different host
        ];

        let identifiers: Vec<String> = urls
            .iter()
            .filter_map(|u| GitRemoteUrl::parse(u).map(|p| p.project_identifier()))
            .collect();

        // All should be unique
        let mut unique = identifiers.clone();
        unique.sort();
        unique.dedup();

        assert_eq!(
            identifiers.len(),
            unique.len(),
            "All URLs must produce unique identifiers. Got duplicates in: {:?}",
            identifiers
        );
    }
}
