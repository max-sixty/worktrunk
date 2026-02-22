//! Remote and URL operations for Repository.

use anyhow::Context;

use super::{GitRemoteUrl, Repository};

impl Repository {
    /// Get the primary remote name for this repository.
    ///
    /// Returns a consistent value across all worktrees (not branch-specific).
    ///
    /// Uses the following strategy:
    /// 1. Use git's [`checkout.defaultRemote`][1] config if set and has a URL
    /// 2. Otherwise, get the first remote with a configured URL
    /// 3. Return error if no remotes exist
    ///
    /// Result is cached in the shared repo cache (shared across all worktrees).
    ///
    /// [1]: https://git-scm.com/docs/git-config#Documentation/git-config.txt-checkoutdefaultRemote
    pub fn primary_remote(&self) -> anyhow::Result<String> {
        self.cache
            .primary_remote
            .get_or_init(|| {
                // Check git's checkout.defaultRemote config
                if let Ok(default_remote) = self.run_command(&["config", "checkout.defaultRemote"])
                {
                    let default_remote = default_remote.trim();
                    if !default_remote.is_empty() && self.remote_has_url(default_remote) {
                        return Some(default_remote.to_string());
                    }
                }

                // Fall back to first remote with a configured URL
                // Use git config to find remotes with URLs, filtering out phantom remotes
                // from global config (e.g., `remote.origin.prunetags=true` without a URL)
                let output = self
                    .run_command(&["config", "--get-regexp", r"remote\..+\.url"])
                    .unwrap_or_default();
                let first_remote = output.lines().next().and_then(|line| {
                    // Parse "remote.<name>.url <value>" format
                    // Use ".url " as delimiter to handle remote names with dots (e.g., "my.remote")
                    line.strip_prefix("remote.")
                        .and_then(|s| s.split_once(".url "))
                        .map(|(name, _)| name)
                });

                first_remote.map(|s| s.to_string())
            })
            .clone()
            .ok_or_else(|| anyhow::anyhow!("No remotes configured"))
    }

    /// Check if a remote has a URL configured.
    fn remote_has_url(&self, remote: &str) -> bool {
        self.run_command(&["config", &format!("remote.{}.url", remote)])
            .map(|url| !url.trim().is_empty())
            .unwrap_or(false)
    }

    /// Get the URL for a remote, if configured.
    pub fn remote_url(&self, remote: &str) -> Option<String> {
        self.run_command(&["remote", "get-url", remote])
            .ok()
            .map(|url| url.trim().to_string())
            .filter(|url| !url.is_empty())
    }

    /// Find a remote that points to a specific owner/repo.
    ///
    /// Searches all configured remotes and returns the name of the first one
    /// whose URL matches the given owner and repo (case-insensitive).
    ///
    /// When `host` is `Some`, the remote must also match the host. This is
    /// important for multi-host setups (e.g., both github.com and
    /// github.enterprise.com).
    ///
    /// Returns `None` if no matching remote is found.
    pub fn find_remote_for_repo(
        &self,
        host: Option<&str>,
        owner: &str,
        repo: &str,
    ) -> Option<String> {
        // Get all remotes with URLs
        let output = self
            .run_command(&["config", "--get-regexp", r"remote\..+\.url"])
            .ok()?;

        for line in output.lines() {
            // Parse "remote.<name>.url <value>" format
            if let Some(rest) = line.strip_prefix("remote.")
                && let Some((name, url)) = rest.split_once(".url ")
                && let Some(parsed) = GitRemoteUrl::parse(url)
                // Case-insensitive comparison (GitHub owner/repo names are case-insensitive)
                && parsed.owner().eq_ignore_ascii_case(owner)
                && parsed.repo().eq_ignore_ascii_case(repo)
                // If host is specified, it must also match (case-insensitive)
                && host.is_none_or(|h| parsed.host().eq_ignore_ascii_case(h))
            {
                return Some(name.to_string());
            }
        }

        None
    }

    /// Find a remote that points to the same project as the given URL.
    ///
    /// Parses the URL to extract host/owner/repo, then searches configured remotes.
    /// Host matching ensures correct remote selection in multi-host setups
    /// (e.g., both gitlab.com and gitlab.enterprise.com).
    ///
    /// Useful for GitLab MRs where glab provides URLs directly.
    ///
    /// Returns `None` if the URL can't be parsed or no matching remote is found.
    pub fn find_remote_by_url(&self, target_url: &str) -> Option<String> {
        let parsed = GitRemoteUrl::parse(target_url)?;
        self.find_remote_for_repo(Some(parsed.host()), parsed.owner(), parsed.repo())
    }

    /// Get all configured remote URLs.
    ///
    /// Returns a list of (remote_name, url) pairs for all remotes with URLs.
    /// Useful for searching across remotes when the specific remote is unknown.
    pub fn all_remote_urls(&self) -> Vec<(String, String)> {
        let output = match self.run_command(&["config", "--get-regexp", r"remote\..+\.url"]) {
            Ok(output) => output,
            Err(_) => return Vec::new(),
        };

        output
            .lines()
            .filter_map(|line| {
                // Parse "remote.<name>.url <value>" format
                let rest = line.strip_prefix("remote.")?;
                let (name, url) = rest.split_once(".url ")?;
                Some((name.to_string(), url.to_string()))
            })
            .collect()
    }

    /// Get the URL for the primary remote, if configured.
    ///
    /// Result is cached in the repository's shared cache (same for all clones).
    pub fn primary_remote_url(&self) -> Option<String> {
        self.cache
            .primary_remote_url
            .get_or_init(|| {
                self.primary_remote()
                    .ok()
                    .and_then(|remote| self.remote_url(&remote))
            })
            .clone()
    }

    /// Get a project identifier for approval tracking.
    ///
    /// Uses the git remote URL if available (e.g., "github.com/user/repo"),
    /// otherwise falls back to the full canonical path of the repository.
    ///
    /// This identifier is used to track which commands have been approved
    /// for execution in this project.
    ///
    /// Result is cached in the repository's shared cache (same for all clones).
    pub fn project_identifier(&self) -> anyhow::Result<String> {
        self.cache
            .project_identifier
            .get_or_try_init(|| {
                // Try to get the remote URL first (cached)
                if let Some(url) = self.primary_remote_url() {
                    if let Some(parsed) = GitRemoteUrl::parse(url.trim()) {
                        return Ok(parsed.project_identifier());
                    }
                    // Fallback for URLs that don't fit host/owner/repo model
                    let url = url.strip_suffix(".git").unwrap_or(url.as_str());
                    return Ok(url.to_string());
                }

                // Fall back to full canonical path (use worktree base for consistency across all worktrees)
                // Full path avoids collisions across unrelated repos with the same directory name
                let repo_root = self.repo_path();
                let canonical =
                    dunce::canonicalize(repo_root).unwrap_or_else(|_| repo_root.to_path_buf());
                let path_str = canonical
                    .to_str()
                    .context("Repository path is not valid UTF-8")?;

                Ok(path_str.to_string())
            })
            .cloned()
    }

    /// Get the URL template from project config, if configured.
    ///
    /// Convenience method that extracts `list.url` from the project config.
    /// Returns `None` if no config exists or no URL template is configured.
    pub fn url_template(&self) -> Option<String> {
        self.load_project_config()
            .ok()
            .flatten()
            .and_then(|config| config.list)
            .and_then(|list| list.url)
    }

    /// Check if a ref is a remote tracking branch.
    ///
    /// Returns true if the ref exists under `refs/remotes/` (e.g., `origin/main`).
    /// Returns false for local branches, tags, SHAs, and non-existent refs.
    pub fn is_remote_tracking_branch(&self, ref_name: &str) -> bool {
        self.run_command(&[
            "rev-parse",
            "--verify",
            &format!("refs/remotes/{}", ref_name),
        ])
        .is_ok()
    }
}
