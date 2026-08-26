//! Remote and URL operations for Repository.
//!
//! # Forge detection
//!
//! Forge-aware operations need three pieces of information:
//!
//! 1. **Forge** — GitHub, GitLab, Gitea, or Azure DevOps
//! 2. **Repository identity** — the owner/namespace and repository path
//! 3. **API hostname** — needed when a CLI cannot infer a self-hosted server
//!
//! [`ForgeKind::from_host`](crate::git::ForgeKind::from_host) is the canonical host
//! classifier. Azure DevOps service domains use domain-suffix matching and
//! take precedence over forge-like labels within those domains. GitHub, GitLab,
//! and Gitea require the forge name as a complete DNS label. Matching ignores
//! case, numeric ports, and a trailing DNS root dot.
//!
//! Repository identity comes from the URL path. This keeps owner/repository
//! extraction valid when an SSH alias replaces the hostname.
//!
//! Remote selection depends on the operation:
//!
//! - [`Repository::ci_platform`] uses `forge.platform`, then the hinted
//!   remote's effective URL, then the primary remote's effective URL.
//! - `wt switch pr:N` honors `forge.platform`, then scans every raw remote
//!   using an explicit provider priority. Its provider owns the final fallback.
//! - Forge API calls extract repository identity from a matching raw remote.
//!   API responses are matched back to local remotes by repository identity.
//!
//! ## Config: `[forge]` section
//!
//! All fields are optional. Standard service hosts and self-hosted names with
//! an exact `github`, `gitlab`, or `gitea` label need no platform override.
//!
//! ```toml
//! [forge]
//! platform = "github"              # forge platform; else detected from URL
//! hostname = "github.example.com"  # API hostname (GHE / self-hosted GitLab)
//! ```
//!
//! `ci.platform` is supported as a deprecated alias for `forge.platform`.
//!
//! ## SSH host aliases
//!
//! Multi-account SSH setups use host aliases (`git@github-personal:owner/repo`)
//! where SSH resolves `github-personal` to `github.com` via `~/.ssh/config`.
//! Git operations work, but the literal hostname affects forge detection.
//!
//! Owner/repo extraction is unaffected because aliases only change the host.
//! An alias with an exact forge label, such as `github.personal`, classifies
//! automatically. Hyphenated and opaque aliases, such as `github-personal` or
//! `work`, need `forge.platform`. A self-hosted alias also needs
//! `forge.hostname` when the forge CLI cannot infer the API server.
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

use super::{GitRemoteUrl, GitRepoInfo, Repository};
use crate::git::error::RefType;
use crate::git::url_path_segments_eq;

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
    /// Resolved from the bulk config map — O(1) once populated.
    ///
    /// [1]: https://git-scm.com/docs/git-config#Documentation/git-config.txt-checkoutdefaultRemote
    pub fn primary_remote(&self) -> anyhow::Result<String> {
        // Check git's checkout.defaultRemote config
        if let Some(default_remote) = self.config_last("checkout.defaultRemote")? {
            let default_remote = default_remote.trim();
            if !default_remote.is_empty() && self.remote_url(default_remote).is_some() {
                return Ok(default_remote.to_string());
            }
        }

        // Fall back to first remote with a configured URL. Filters out
        // phantom remotes from global config (e.g., `remote.origin.prunetags
        // = true` without a URL) by requiring a `remote.<NAME>.url` entry.
        let guard = self.all_config()?.read().unwrap();
        let first_remote = guard.keys().find_map(|k| {
            let rest = k.strip_prefix("remote.")?;
            // `.url` suffix identifies url entries. Remote names can contain
            // dots (e.g., "my.remote"), so rsplit from the right: the last
            // `.url` is always the suffix.
            let name = rest.strip_suffix(".url")?;
            Some(name.to_string())
        });
        first_remote.ok_or_else(|| anyhow::anyhow!("No remotes configured"))
    }

    /// Get the URL for a remote, if configured.
    ///
    /// Returns the raw value from `.git/config` without applying `url.insteadOf`
    /// rewrites. Use [`effective_remote_url`](Self::effective_remote_url) when you
    /// need forge detection to work with `insteadOf` aliases.
    ///
    /// Resolved from the bulk config map — O(1) once populated.
    pub fn remote_url(&self, remote: &str) -> Option<String> {
        self.config_last(&format!("remote.{remote}.url"))
            .ok()
            .flatten()
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
        self.find_remote(|parsed| {
            parsed.owner().eq_ignore_ascii_case(owner)
                && parsed.repo().eq_ignore_ascii_case(repo)
                && host.is_none_or(|h| parsed.host().eq_ignore_ascii_case(h))
        })
    }

    /// Return the first remote whose URL (raw or `insteadOf`-effective) parses
    /// and satisfies `matches`. Each remote is checked against its raw URL and,
    /// when it differs, its effective URL — so a custom hostname rewritten to a
    /// real forge still matches.
    fn find_remote(&self, matches: impl Fn(&GitRemoteUrl) -> bool) -> Option<String> {
        let test = |url: &str| GitRemoteUrl::parse(url).is_some_and(|parsed| matches(&parsed));
        for (remote_name, raw_url) in self.all_remote_urls() {
            if test(&raw_url) {
                return Some(remote_name);
            }
            if let Some(effective_url) = self.effective_remote_url(&remote_name)
                && effective_url != raw_url
                && test(&effective_url)
            {
                return Some(remote_name);
            }
        }

        None
    }

    /// Find a remote that points to the given Azure DevOps project + repo.
    ///
    /// Azure DevOps URLs do not fit the standard `host/owner/repo` shape that
    /// [`find_remote_for_repo`](Self::find_remote_for_repo) assumes — for
    /// `https://dev.azure.com/{org}/{project}/_git/{repo}` the parser stores
    /// `{org}/{project}/_git` in `owner`, and the SSH variant stores `v3/{org}/{project}`.
    /// Match by `azure_organization()` + `azure_project()` + `repo()` instead, so
    /// two projects in the same org with the same repo name don't collide.
    pub fn find_remote_for_azure(
        &self,
        organization: &str,
        project: &str,
        repo_name: &str,
    ) -> Option<String> {
        self.find_remote(|parsed| {
            parsed
                .azure_organization()
                .is_some_and(|o| o.eq_ignore_ascii_case(organization))
                && parsed
                    .azure_project()
                    .is_some_and(|p| url_path_segments_eq(p, project))
                && url_path_segments_eq(parsed.repo(), repo_name)
        })
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
        let Ok(lock) = self.all_config() else {
            return Vec::new();
        };
        let guard = lock.read().unwrap();
        guard
            .iter()
            .filter_map(|(k, values)| {
                let rest = k.strip_prefix("remote.")?;
                let name = rest.strip_suffix(".url")?;
                let url = values.last()?.trim();
                if url.is_empty() {
                    return None;
                }
                Some((name.to_string(), url.to_string()))
            })
            .collect()
    }

    /// Get the URL for the primary remote, if configured.
    ///
    /// Returns the raw config value. Resolves via the bulk config map.
    pub fn primary_remote_url(&self) -> Option<String> {
        self.primary_remote()
            .ok()
            .and_then(|remote| self.remote_url(&remote))
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

    /// Configured forge platform: the repository's `[forge].platform` (or the
    /// deprecated `ci.platform`), else a user-config `[projects."…"].forge`
    /// entry matching this repository.
    ///
    /// Local-only: reads (and may cache) project and user config. Every path
    /// that derives a forge provider resolves it here — `wt list` and its
    /// statusline via `repo_info`, `wt switch pr:` when picking a provider,
    /// and CI-platform detection via `Repository::ci_platform` — so a
    /// self-hosted instance whose host carries no forge brand answers the same
    /// way everywhere instead of resolving in one command and reading
    /// `unknown` in the next.
    ///
    /// The repository's own block wins over a user entry: a
    /// `[projects."git.company.example/*"]` entry states what a host is, and a
    /// repository that names something else knows better.
    pub fn configured_forge_platform(&self) -> Option<String> {
        if let Some(platform) = self
            .project_config()
            .ok()
            .flatten()
            .and_then(|config| config.forge_platform().map(str::to_string))
        {
            return Some(platform);
        }
        let project_id = self.project_identifier().ok()?;
        self.user_config()
            .forge_platform(Some(&project_id))
            .map(str::to_string)
    }

    /// The forge API hostname for GitHub Enterprise or self-hosted GitLab.
    ///
    /// The repository's `[forge].hostname`, else a matching user-config
    /// `[projects."…"].forge.hostname`. Passed to the forge CLI as
    /// `--hostname` when the remote's own host isn't the API server — an SSH
    /// alias, or an API on a different name.
    pub fn forge_hostname(&self) -> Option<String> {
        if let Some(hostname) = self
            .project_config()
            .ok()
            .flatten()
            .and_then(|config| config.forge_hostname().map(str::to_string))
        {
            return Some(hostname);
        }
        let project_id = self.project_identifier().ok()?;
        self.user_config()
            .forge_hostname(Some(&project_id))
            .map(str::to_string)
    }

    /// Repository metadata derived from the primary remote.
    ///
    /// Local-only: built from the cached primary remote URL and the configured
    /// forge platform, with no network access. This may load/cache project and
    /// user config to resolve that platform. The returned
    /// [`GitRepoInfo::remote`] names which local remote was used, so structured
    /// JSON can report it.
    pub fn repo_info(&self) -> Option<GitRepoInfo> {
        let remote = self.primary_remote().ok()?;
        let url = self.remote_url(&remote)?;
        let parsed = GitRemoteUrl::parse(&url)?;
        let provider_override = self.configured_forge_platform();
        let mut info = parsed.repo_info(provider_override.as_deref())?;
        info.remote = Some(remote);
        Some(info)
    }

    /// Parsed URL of the remote that belongs to a particular forge.
    ///
    /// Prefers the primary remote when it matches `is_forge`, then the first
    /// other remote whose URL matches, and finally falls back to the primary
    /// remote's parsed URL even if it doesn't match (so callers preserve their
    /// existing "no forge remote" error path). PR/MR lookup uses this so a
    /// GitHub/Gitea mirror configured as a *non-primary* remote is queried
    /// against its own owner/repo rather than the primary (e.g. GitLab/Azure)
    /// remote's path.
    pub fn forge_remote_parsed_url(
        &self,
        is_forge: impl Fn(&GitRemoteUrl) -> bool,
    ) -> Option<GitRemoteUrl> {
        if let Some(primary) = self.primary_remote_parsed_url()
            && is_forge(&primary)
        {
            return Some(primary);
        }
        self.all_remote_urls()
            .into_iter()
            .filter_map(|(_, url)| GitRemoteUrl::parse(&url))
            .find(&is_forge)
            .or_else(|| self.primary_remote_parsed_url())
    }

    /// Detect the known forge's reference type (PR or MR).
    ///
    /// Classifies the primary remote's effective URL (so `url.insteadOf`
    /// rewrites are respected). Returns `None` when the forge can't be
    /// determined (no remote, opaque host).
    ///
    /// Drives the numeric-branch hint in `BranchNotFound` only — `forge.platform`
    /// is intentionally not consulted here. Honoring it would duplicate
    /// `Repository::ci_platform`'s precedence rules; users who set it but have
    /// an opaque remote host fall through to the both-platforms hint.
    pub fn detect_ref_type(&self) -> Option<RefType> {
        let url = self
            .primary_remote()
            .ok()
            .and_then(|remote| self.effective_remote_url(&remote))?;
        let parsed = GitRemoteUrl::parse(&url)?;
        parsed.forge_kind().map(|forge| forge.ref_type())
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
            .and_then(|config| config.list.url)
    }

    /// Strip the remote prefix from a remote-tracking branch name.
    ///
    /// Given a name like `origin/username/feature-1`, returns `Some("username/feature-1")`
    /// if it's a valid remote-tracking ref. Returns `None` if the name isn't a remote ref
    /// or the remote can't be identified.
    ///
    /// Resolved from the remote-branch inventory — no subprocess calls once it's populated.
    pub fn strip_remote_prefix(&self, ref_name: &str) -> Option<String> {
        self.remote_branches()
            .ok()?
            .iter()
            .find(|r| r.short_name == ref_name)
            .map(|r| r.local_name.clone())
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::TestRepo;

    #[test]
    fn test_find_remote_for_azure_dev_azure() {
        let t = TestRepo::new();
        t.run_git(&[
            "remote",
            "add",
            "origin",
            "https://dev.azure.com/myorg/myproject/_git/myrepo",
        ]);

        // Matching org/project/repo finds the remote.
        assert_eq!(
            t.repo.find_remote_for_azure("myorg", "myproject", "myrepo"),
            Some("origin".to_string())
        );

        // Wrong project — Azure orgs can hold many projects with the same repo name.
        assert_eq!(
            t.repo
                .find_remote_for_azure("myorg", "wrong-project", "myrepo"),
            None
        );

        // Wrong repo.
        assert_eq!(
            t.repo
                .find_remote_for_azure("myorg", "myproject", "wrong-repo"),
            None
        );

        // Wrong org.
        assert_eq!(
            t.repo
                .find_remote_for_azure("other-org", "myproject", "myrepo"),
            None
        );

        // Case-insensitive across all three components.
        assert_eq!(
            t.repo.find_remote_for_azure("MYORG", "MyProject", "MyRepo"),
            Some("origin".to_string())
        );
    }

    #[test]
    fn test_find_remote_for_azure_ssh_form() {
        // SSH form: git@ssh.dev.azure.com:v3/{org}/{project}/{repo}
        let t = TestRepo::new();
        t.run_git(&[
            "remote",
            "add",
            "origin",
            "git@ssh.dev.azure.com:v3/myorg/myproject/myrepo",
        ]);

        assert_eq!(
            t.repo.find_remote_for_azure("myorg", "myproject", "myrepo"),
            Some("origin".to_string())
        );
    }

    #[test]
    fn test_find_remote_for_azure_visualstudio() {
        // Legacy *.visualstudio.com form: org is in the hostname.
        let t = TestRepo::new();
        t.run_git(&[
            "remote",
            "add",
            "origin",
            "https://myorg.visualstudio.com/myproject/_git/myrepo",
        ]);

        assert_eq!(
            t.repo.find_remote_for_azure("myorg", "myproject", "myrepo"),
            Some("origin".to_string())
        );
    }

    #[test]
    fn test_find_remote_for_azure_multiple_remotes() {
        // A repo with both an Azure DevOps fork and a GitHub mirror — the Azure
        // matcher must skip the GitHub remote without false-positive matching.
        let t = TestRepo::new();
        t.run_git(&[
            "remote",
            "add",
            "origin",
            "https://dev.azure.com/myorg/myproject/_git/myrepo",
        ]);
        t.run_git(&["remote", "add", "github", "https://github.com/myorg/myrepo"]);

        assert_eq!(
            t.repo.find_remote_for_azure("myorg", "myproject", "myrepo"),
            Some("origin".to_string())
        );
    }

    #[test]
    fn test_find_remote_for_azure_no_azure_remote() {
        // GitHub-only repo: Azure matcher should find nothing.
        let t = TestRepo::new();
        t.run_git(&["remote", "add", "origin", "https://github.com/myorg/myrepo"]);

        assert_eq!(
            t.repo
                .find_remote_for_azure("myorg", "anyproject", "myrepo"),
            None
        );
    }
}
