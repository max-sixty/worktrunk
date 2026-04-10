//! Remote and URL operations for Repository.
//!
//! # Forge detection
//!
//! Worktrunk needs three pieces of information to talk to GitHub/GitLab:
//!
//! 1. **Platform** — GitHub or GitLab (which CLI to invoke)
//! 2. **Owner/repo** — the project path (for API calls)
//! 3. **API hostname** — which server to talk to (only needed for GHE /
//!    self-hosted GitLab; `gh`/`glab` default to github.com/gitlab.com)
//!
//! ## Design principle
//!
//! Derive owner/repo from URL paths, not hostnames. SSH aliases only
//! corrupt the host component — the path (`owner/repo`) is always real.
//! Only use the hostname for platform detection (substring match), and
//! let `gh`/`glab` default to the right API host unless overridden.
//!
//! **Resolution order:**
//!
//! 1. **Remote** — branch's remote (for remote branches) >
//!    [`primary_remote()`](Repository::primary_remote)
//! 2. **Platform** — `forge.platform` config > hostname substring match
//!    ([`is_github()`](super::GitRemoteUrl::is_github) /
//!    [`is_gitlab()`](super::GitRemoteUrl::is_gitlab)) on effective URL
//! 3. **Owner/repo** — parsed from the chosen remote's URL path (works
//!    regardless of hostname, including SSH aliases)
//! 4. **API hostname** — `forge.hostname` config > omit (let CLI default)
//!
//! For `wt list`, steps 1-4 are sufficient — each branch uses its
//! associated remote.
//!
//! For `wt switch pr:N`, the API call uses owner/repo from the primary
//! remote's raw URL. The API response provides the base repo's identity,
//! and `Repository::find_remote_for_repo` matches it back to a local remote by
//! owner/repo (host is not required to match).
//!
//! ## Where each piece is used
//!
//! | Need | `wt switch pr:N` | `wt list` CI status |
//! |------|------------------|---------------------|
//! | Platform | Implicit (`pr:` = GitHub, `mr:` = GitLab) | `platform_for_repo` |
//! | Owner/repo | `fetch_pr_info` builds API path | `github_owner_repo` for check-runs API |
//! | API hostname | `forge.hostname` config, else omit | `forge.hostname` config, else omit |
//! | Fetch remote | `Repository::find_remote_for_repo` by owner/repo | Not needed |
//!
//! ## Config: `[forge]` section
//!
//! All fields are optional. For most repositories (single remote, hostname
//! contains "github" or "gitlab"), no configuration is needed.
//!
//! ```toml
//! [forge]
//! platform = "github"              # override platform detection
//! hostname = "github.example.com"  # API hostname (GHE / self-hosted GitLab)
//! ```
//!
//! `ci.platform` is supported as a deprecated alias for `forge.platform`.
//!
//! ## SSH host aliases
//!
//! Multi-account SSH setups use host aliases (`git@github-personal:owner/repo`)
//! where SSH resolves `github-personal` → `github.com` via `~/.ssh/config`.
//! Git operations work, but the literal hostname affects forge detection.
//!
//! Owner/repo extraction is unaffected — aliases only change the host, not
//! the path. The impact depends on the alias name:
//!
//! | Alias | Platform detection | API calls | Config needed |
//! |-------|-------------------|-----------|---------------|
//! | `github-personal` | Works ("github" in name) | Works (`gh` defaults to github.com) | None |
//! | `work` (opaque) | Fails | Works (`gh` defaults to github.com) | `forge.platform` |
//! | GHE alias | May work | Needs explicit host | `forge.hostname` (+ `forge.platform` if opaque) |
//!
//! ### `url.insteadOf` (alternative)
//!
//! Git's `url.insteadOf` rewrites URLs before any tool sees them, which
//! solves all detection problems. Trade-off: it also affects SSH, which
//! sees `github.com` instead of the alias and can't select the correct
//! `IdentityFile`. Users must pair it with per-repo `core.sshCommand`.
//! The `[forge]` config avoids this trade-off.
//!
//! ## URL methods
//!
//! - `remote_url` — raw config value, no rewriting (for non-forge uses
//!   like template variables and project identifiers)
//! - `effective_remote_url` — `git remote get-url`, with `insteadOf`
//!   applied (cached; used for platform detection)
//! - `find_remote_for_repo(host, owner, repo)` — match owner/repo across
//!   remotes, host used only as disambiguator

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
                let first_remote = output.lines().find_map(|line| {
                    // Parse "remote.<name>.url <value>" format
                    // Use ".url " as delimiter to handle remote names with dots (e.g., "my.remote")
                    // Use find_map (not next + parse) because the unanchored regex matches
                    // any config key containing "remote.<something>.url" — not just actual
                    // remote entries. For example, includeIf.hasconfig:remote.*.url:... keys
                    // match and can appear before the first real remote URL.
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

    /// Parse the primary remote URL into structured host/owner/repo components.
    ///
    /// Uses the raw configured URL rather than `effective_remote_url()` so owner/namespace
    /// extraction follows the same "path is the source of truth" rule used elsewhere.
    pub fn primary_remote_parsed_url(&self) -> Option<GitRemoteUrl> {
        self.primary_remote_url()
            .as_deref()
            .and_then(GitRemoteUrl::parse)
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
