//! Remote and URL operations for Repository.
//!
//! # Forge detection
//!
//! Worktrunk needs three pieces of information from remote URLs:
//!
//! 1. **Platform** — GitHub or GitLab (determines which CLI/API to use)
//! 2. **Hostname** — which server to talk to (for `gh api --hostname`,
//!    `glab auth status --hostname`)
//! 3. **Owner/repo** — the project path (for API calls and remote matching)
//!
//! All three are extracted from the remote URL's hostname via substring
//! matching: [`is_github()`](super::GitRemoteUrl::is_github) checks if the
//! host contains `"github"`, [`is_gitlab()`](super::GitRemoteUrl::is_gitlab)
//! checks for `"gitlab"`. This covers `github.com`, GitHub Enterprise
//! (`github.mycompany.com`), `gitlab.com`, and self-hosted GitLab
//! (`gitlab.example.com`).
//!
//! ## Where each piece is resolved
//!
//! | Need | `wt switch pr:N` | `wt list` CI status |
//! |------|------------------|---------------------|
//! | Platform | Implicit (user typed `pr:` or `mr:`) | [`platform_for_repo`] in `ci_status::platform` |
//! | Hostname | [`fetch_pr_info`] extracts from effective URL | `gh`/`glab` infer from `current_dir` |
//! | Owner/repo | API response provides it | [`github_owner_repo`] / [`find_forge_remote`] |
//! | Remote matching | [`find_remote_for_repo`] matches API response back to local remote | Not needed |
//!
//! [`platform_for_repo`]: crate::commands::list::ci_status::platform::platform_for_repo
//! [`fetch_pr_info`]: crate::git::remote_ref::github
//! [`github_owner_repo`]: crate::commands::list::ci_status::github
//!
//! ## SSH host aliases
//!
//! Multi-account SSH setups use host aliases (`git@github-personal:owner/repo`)
//! where SSH resolves `github-personal` → `github.com` via `~/.ssh/config`.
//! Git operations work, but the literal hostname breaks forge detection:
//!
//! - **Platform detection** fails if the alias doesn't contain `"github"` or
//!   `"gitlab"` (e.g., `work`). Override with `ci.platform` in project config.
//! - **`gh api --hostname`** receives the alias instead of the real hostname.
//!   If the alias contains `"github"` (e.g., `github-personal`), `gh` gets
//!   `--hostname github-personal` and fails. If not (e.g., `work`), we fall
//!   back to `github.com` which works for non-Enterprise repos.
//! - **Remote matching** fails when comparing `github.com` (from API response)
//!   against `github-personal` (from local remote URL).
//!
//! ### `url.insteadOf` (current solution)
//!
//! Git's `url.insteadOf` rewrites URLs before any tool sees them:
//!
//! ```text
//! git config url."git@github.com:".insteadOf "git@github-personal:"
//! ```
//!
//! After this, [`effective_remote_url`](Repository::effective_remote_url)
//! (which runs `git remote get-url`) returns the rewritten URL with the real
//! hostname. All forge detection, API calls, and remote matching work.
//!
//! Trade-off: `url.insteadOf` also affects SSH, which sees `github.com`
//! instead of the alias and can't select the correct `IdentityFile`. Users
//! must pair it with per-repo `core.sshCommand` to restore key routing.
//!
//! ### Known gap
//!
//! SSH host aliases without `url.insteadOf` are partially unsupported. The
//! hostname and remote matching steps fail because `effective_remote_url`
//! returns the alias unchanged. Owner/repo extraction is unaffected (aliases
//! only change the host, not the path). A future fix could resolve aliases
//! via `ssh -G <host>` or a config option; see issue #1790.
//!
//! ## URL methods
//!
//! - `remote_url` — raw config value, no rewriting (for non-forge uses like
//!   template variables and project identifiers)
//! - `effective_remote_url` — `git remote get-url`, with `insteadOf` applied
//!   (cached; use for all forge detection)
//! - `find_forge_remote(pred)` — search all remotes using effective URLs
//! - `find_remote_for_repo(host, owner, repo)` — match API response back to
//!   local remote (checks both raw and effective URLs)

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
    ///
    /// Returns the raw value from `.git/config` without applying `url.insteadOf`
    /// rewrites. Use [`effective_remote_url`](Self::effective_remote_url) when you
    /// need forge detection to work with `insteadOf` aliases.
    pub fn remote_url(&self, remote: &str) -> Option<String> {
        self.run_command(&["config", &format!("remote.{}.url", remote)])
            .ok()
            .map(|url| url.trim().to_string())
            .filter(|url| !url.is_empty())
    }

    /// Get the effective URL for a remote, with `url.insteadOf` rewrites applied.
    ///
    /// Uses `git remote get-url` which applies `url.insteadOf` rewrites. When no
    /// rewrite rules are configured, returns the same value as [`remote_url`](Self::remote_url).
    ///
    /// Results are cached per-remote in the shared repo cache.
    ///
    /// Returns `None` if the remote doesn't exist or has no URL.
    pub fn effective_remote_url(&self, remote: &str) -> Option<String> {
        self.cache
            .effective_remote_urls
            .entry(remote.to_string())
            .or_insert_with(|| {
                self.run_command(&["remote", "get-url", remote])
                    .ok()
                    .map(|url| url.trim().to_string())
                    .filter(|url| !url.is_empty())
            })
            .clone()
    }

    /// Find a remote that points to a specific owner/repo.
    ///
    /// Searches all configured remotes and returns the name of the first one
    /// whose URL matches the given owner and repo (case-insensitive). Checks
    /// both the raw config URL and the effective URL (with `url.insteadOf`
    /// rewrites applied), so matches work in both directions: when the raw URL
    /// contains a real forge hostname, and when `insteadOf` rewrites a custom
    /// hostname to a real forge.
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
        let matches = |url: &str| -> bool {
            let Some(parsed) = GitRemoteUrl::parse(url) else {
                return false;
            };
            parsed.owner().eq_ignore_ascii_case(owner)
                && parsed.repo().eq_ignore_ascii_case(repo)
                && host.is_none_or(|h| parsed.host().eq_ignore_ascii_case(h))
        };

        for (remote_name, raw_url) in self.all_remote_urls() {
            if matches(&raw_url) {
                return Some(remote_name);
            }
            if let Some(effective_url) = self.effective_remote_url(&remote_name)
                && effective_url != raw_url
                && matches(&effective_url)
            {
                return Some(remote_name);
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
    /// Returns the raw config value. Result is cached in the shared repo cache.
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

    /// Search all remotes for a URL matching a predicate.
    ///
    /// Uses effective URLs (with `url.insteadOf` rewrites applied) so that
    /// custom SSH hostnames resolve to real forge hostnames.
    ///
    /// Returns the first matching `(remote_name, url)` pair, or `None`.
    pub fn find_forge_remote<F>(&self, predicate: F) -> Option<(String, String)>
    where
        F: Fn(&GitRemoteUrl) -> bool,
    {
        for (remote_name, _) in self.all_remote_urls() {
            if let Some(url) = self.effective_remote_url(&remote_name)
                && let Some(parsed) = GitRemoteUrl::parse(&url)
                && predicate(&parsed)
            {
                return Some((remote_name, url));
            }
        }
        None
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
                let repo_root = self.repo_path()?;
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

    /// Strip the remote prefix from a remote-tracking branch name.
    ///
    /// Given a name like `origin/username/feature-1`, returns `Some("username/feature-1")`
    /// if it's a valid remote-tracking ref. Returns `None` if the name isn't a remote ref
    /// or the remote can't be identified.
    ///
    /// This handles remote names that don't contain `/` (the common case). It lists
    /// all configured remotes and finds the one that matches the prefix.
    ///
    /// TODO: A cleaner approach would be to strip the prefix upstream — either have
    /// `list_remote_branches()` return `(remote, local_branch, sha)` tuples, or track
    /// `is_remote` on `ListItem` so the picker outputs just the local branch name.
    /// Either would eliminate this runtime `git remote` call. See #1260.
    pub fn strip_remote_prefix(&self, ref_name: &str) -> Option<String> {
        // Quick check: is this actually a remote-tracking ref?
        if !self.is_remote_tracking_branch(ref_name) {
            return None;
        }

        // List all remotes and find the one that is a prefix of ref_name
        let output = self.run_command(&["remote"]).ok()?;
        output.lines().find_map(|remote| {
            let prefix = format!("{}/", remote.trim());
            ref_name
                .strip_prefix(&prefix)
                .filter(|branch| !branch.is_empty())
                .map(|branch| branch.to_string())
        })
    }
}
