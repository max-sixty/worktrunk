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
    /// otherwise falls back to the repository directory name.
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
                    // Fallback for URLs that don't fit host/owner/repo model (e.g., with ports)
                    let url = url.strip_suffix(".git").unwrap_or(url.as_str());
                    // Handle ssh:// format with port: ssh://git@host:port/path -> host/port/path
                    if let Some(ssh_part) = url.strip_prefix("ssh://") {
                        let ssh_part = ssh_part.strip_prefix("git@").unwrap_or(ssh_part);
                        if let Some(colon_pos) = ssh_part.find(':') {
                            let (host, rest) = ssh_part.split_at(colon_pos);
                            return Ok(format!("{}{}", host, rest.replacen(':', "/", 1)));
                        }
                        return Ok(ssh_part.to_string());
                    }
                    return Ok(url.to_string());
                }

                // Fall back to repository name (use worktree base for consistency across all worktrees)
                let repo_root = self.repo_path()?;
                let repo_name = repo_root
                    .file_name()
                    .and_then(|name| name.to_str())
                    .context("Repository directory has no valid name")?;

                Ok(repo_name.to_string())
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
}
