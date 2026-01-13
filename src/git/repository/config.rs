//! Git config, hints, marker, and default branch operations for Repository.

use anyhow::Context;

use crate::config::ProjectConfig;

use super::{DefaultBranchName, GitError, Repository};

impl Repository {
    /// Get a git config value. Returns None if the key doesn't exist.
    pub fn get_config(&self, key: &str) -> anyhow::Result<Option<String>> {
        match self.run_command(&["config", key]) {
            Ok(value) => Ok(Some(value.trim().to_string())),
            Err(_) => Ok(None), // Config key doesn't exist
        }
    }

    /// Set a git config value.
    pub fn set_config(&self, key: &str, value: &str) -> anyhow::Result<()> {
        self.run_command(&["config", key, value])?;
        Ok(())
    }

    /// Read a user-defined marker from `worktrunk.state.<branch>.marker` in git config.
    ///
    /// Markers are stored as JSON: `{"marker": "text", "set_at": unix_timestamp}`.
    pub fn branch_keyed_marker(&self, branch: &str) -> Option<String> {
        #[derive(serde::Deserialize)]
        struct MarkerValue {
            marker: Option<String>,
        }

        let config_key = format!("worktrunk.state.{branch}.marker");
        let raw = self
            .run_command(&["config", "--get", &config_key])
            .ok()
            .map(|output| output.trim().to_string())
            .filter(|s| !s.is_empty())?;

        let parsed: MarkerValue = serde_json::from_str(&raw).ok()?;
        parsed.marker
    }

    /// Read user-defined branch-keyed marker.
    pub fn user_marker(&self, branch: Option<&str>) -> Option<String> {
        branch.and_then(|branch| self.branch_keyed_marker(branch))
    }

    /// Record the previous branch in worktrunk.history for `wt switch -` support.
    ///
    /// Stores the branch we're switching FROM, so `wt switch -` can return to it.
    pub fn record_switch_previous(&self, previous: Option<&str>) -> anyhow::Result<()> {
        if let Some(prev) = previous {
            self.run_command(&["config", "worktrunk.history", prev])?;
        }
        // If previous is None (detached HEAD), don't update history
        Ok(())
    }

    /// Get the previous branch from worktrunk.history for `wt switch -`.
    ///
    /// Returns the branch we came from, enabling ping-pong switching.
    pub fn get_switch_previous(&self) -> Option<String> {
        self.run_command(&["config", "--get", "worktrunk.history"])
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// Check if a hint has been shown in this repo.
    ///
    /// Hints are stored as `worktrunk.hints.<name> = true`.
    /// TODO: Could move to global git config if we accumulate more global hints.
    pub fn has_shown_hint(&self, name: &str) -> bool {
        self.run_command(&["config", "--get", &format!("worktrunk.hints.{name}")])
            .is_ok()
    }

    /// Mark a hint as shown in this repo.
    pub fn mark_hint_shown(&self, name: &str) -> anyhow::Result<()> {
        self.run_command(&["config", &format!("worktrunk.hints.{name}"), "true"])?;
        Ok(())
    }

    /// Clear a hint so it will show again.
    pub fn clear_hint(&self, name: &str) -> anyhow::Result<bool> {
        match self.run_command(&["config", "--unset", &format!("worktrunk.hints.{name}")]) {
            Ok(_) => Ok(true),
            Err(_) => Ok(false), // Key didn't exist
        }
    }

    /// List all hints that have been shown in this repo.
    pub fn list_shown_hints(&self) -> Vec<String> {
        self.run_command(&["config", "--get-regexp", r"^worktrunk\.hints\."])
            .unwrap_or_default()
            .lines()
            .filter_map(|line| {
                // Format: "worktrunk.hints.worktree-path true"
                line.split_whitespace()
                    .next()
                    .and_then(|key| key.strip_prefix("worktrunk.hints."))
                    .map(String::from)
            })
            .collect()
    }

    /// Clear all hints so they will show again.
    pub fn clear_all_hints(&self) -> anyhow::Result<usize> {
        let hints = self.list_shown_hints();
        let count = hints.len();
        for hint in hints {
            self.clear_hint(&hint)?;
        }
        Ok(count)
    }

    // =========================================================================
    // Default branch detection
    // =========================================================================

    /// Get the default branch name for the repository.
    ///
    /// **Performance note:** This method may trigger a network call on first invocation
    /// if the remote HEAD is not cached locally. The result is then cached in git's
    /// config for subsequent calls. To minimize latency:
    /// - Defer calling this until after fast, local checks (see e497f0f for example)
    /// - Consider passing the result as a parameter if needed multiple times
    /// - For optional operations, provide a fallback (e.g., `.unwrap_or("main")`)
    ///
    /// Uses a hybrid approach:
    /// 1. Check worktrunk cache (`git config worktrunk.default-branch`) — single command
    /// 2. Detect primary remote, try its cache (e.g., `origin/HEAD`)
    /// 3. Query remote (`git ls-remote`) — may take 100ms-2s
    /// 4. Infer from local branches if no remote
    ///
    /// Detection results are cached to `worktrunk.default-branch` for future calls.
    /// Result is cached in the shared repo cache (shared across all worktrees).
    ///
    /// Returns `None` if the default branch cannot be determined.
    pub fn default_branch(&self) -> Option<String> {
        self.cache
            .default_branch
            .get_or_init(|| {
                // Fast path: check worktrunk's persistent cache (git config)
                let configured = self
                    .run_command(&["config", "--get", "worktrunk.default-branch"])
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());

                // If configured, validate it exists locally
                if let Some(ref branch) = configured {
                    if self.local_branch_exists(branch).unwrap_or(false) {
                        // Valid config - no invalid branch to report
                        let _ = self.cache.invalid_default_branch.set(None);
                        return Some(branch.clone());
                    }
                    // Configured branch doesn't exist - cache for warning, return None
                    let _ = self.cache.invalid_default_branch.set(Some(branch.clone()));
                    log::debug!(
                        "Configured default branch '{}' doesn't exist locally",
                        branch
                    );
                    return None;
                }

                // Not configured - no invalid branch to report
                let _ = self.cache.invalid_default_branch.set(None);

                // Not configured - detect and persist to git config
                if let Ok(branch) = self.detect_default_branch() {
                    let _ = self.run_command(&["config", "worktrunk.default-branch", &branch]);
                    return Some(branch);
                }

                None
            })
            .clone()
    }

    /// Check if user configured an invalid default branch.
    ///
    /// Returns `Some(branch_name)` if user set `worktrunk.default-branch` to a branch
    /// that doesn't exist locally. Returns `None` if:
    /// - No branch is configured (detection will be used)
    /// - Configured branch exists locally
    ///
    /// Used to show warnings when the configured branch is invalid.
    ///
    /// **Performance:** Calls `default_branch()` internally to ensure the cache is
    /// populated, but that call is itself cached so subsequent calls are free.
    pub fn invalid_default_branch_config(&self) -> Option<String> {
        // Ensure default_branch() has populated the cache (no-op if already called)
        let _ = self.default_branch();
        self.cache
            .invalid_default_branch
            .get()
            .and_then(|opt| opt.clone())
    }

    /// Detect the default branch without using worktrunk's cache.
    ///
    /// Used by `default_branch()` to populate the cache, and after
    /// `wt config state default-branch clear` to force re-detection.
    pub fn detect_default_branch(&self) -> anyhow::Result<String> {
        // Try to get from the primary remote
        if let Ok(remote) = self.primary_remote() {
            // Try git's cache for this remote (e.g., origin/HEAD)
            if let Ok(branch) = self.get_local_default_branch(&remote) {
                return Ok(branch);
            }

            // Query remote (no caching to git's remote HEAD - we only manage worktrunk's cache)
            if let Ok(branch) = self.query_remote_default_branch(&remote) {
                return Ok(branch);
            }
        }

        // Fallback: No remote or remote query failed, try to infer locally
        // TODO: Show message to user when using inference fallback:
        //   "No remote configured. Using inferred default branch: {branch}"
        //   "To set explicitly, run: wt config state default-branch set <branch>"
        // Problem: git.rs is in lib crate, output module is in binary.
        // Options: (1) Return info about whether fallback was used, let callers show message
        //          (2) Add messages in specific commands (merge.rs, worktree.rs)
        //          (3) Move output abstraction to lib crate
        self.infer_default_branch_locally()
    }

    /// Resolve a target branch from an optional override
    ///
    /// If target is Some, expands special symbols ("@", "-", "^") via `resolve_worktree_name`.
    /// Otherwise, queries the default branch.
    /// This is a common pattern used throughout commands that accept an optional --target flag.
    pub fn resolve_target_branch(&self, target: Option<&str>) -> anyhow::Result<String> {
        match target {
            Some(b) => self.resolve_worktree_name(b),
            None => self.default_branch().ok_or_else(|| {
                GitError::Other {
                    message: "Cannot determine default branch. Specify target explicitly or run 'wt config state default-branch set <branch>'.".into(),
                }
                .into()
            }),
        }
    }

    /// Infer the default branch locally (without remote).
    ///
    /// Uses local heuristics when no remote is available:
    /// 1. If only one local branch exists, use it
    /// 2. Check symbolic-ref HEAD (authoritative for bare repos, works before first commit)
    /// 3. Check user's git config init.defaultBranch (if branch exists)
    /// 4. Look for common branch names (main, master, develop, trunk)
    /// 5. Fail if none of the above work
    fn infer_default_branch_locally(&self) -> anyhow::Result<String> {
        // 1. If there's only one local branch, use it
        let branches = self.local_branches()?;
        if branches.len() == 1 {
            return Ok(branches[0].clone());
        }

        // 2. Check symbolic-ref HEAD - authoritative for bare repos and empty repos
        // - Bare repo directory: HEAD always points to the default branch
        // - Empty repos: No branches exist yet, but HEAD tells us the intended default
        // - Linked worktrees: HEAD points to CURRENT branch, so skip this heuristic
        // - Normal repos: HEAD points to CURRENT branch, so skip this heuristic
        let is_bare = self.is_bare().unwrap_or(false);
        let in_linked_worktree = self.current_worktree().is_linked().unwrap_or(false);
        if ((is_bare && !in_linked_worktree) || branches.is_empty())
            && let Ok(head_ref) = self.run_command(&["symbolic-ref", "HEAD"])
            && let Some(branch) = head_ref.trim().strip_prefix("refs/heads/")
        {
            return Ok(branch.to_string());
        }

        // 3. Check git config init.defaultBranch (if branch exists)
        if let Ok(default) = self.run_command(&["config", "--get", "init.defaultBranch"]) {
            let branch = default.trim().to_string();
            if !branch.is_empty() && branches.contains(&branch) {
                return Ok(branch);
            }
        }

        // 4. Look for common branch names
        for name in ["main", "master", "develop", "trunk"] {
            if branches.contains(&name.to_string()) {
                return Ok(name.to_string());
            }
        }

        // 5. Give up — can't infer
        Err(GitError::Other {
            message:
                "Could not infer default branch. Please specify target branch explicitly or set up a remote."
                    .into(),
        }
        .into())
    }

    // Private helpers for default_branch detection

    fn get_local_default_branch(&self, remote: &str) -> anyhow::Result<String> {
        let stdout =
            self.run_command(&["rev-parse", "--abbrev-ref", &format!("{}/HEAD", remote)])?;
        DefaultBranchName::from_local(remote, &stdout).map(DefaultBranchName::into_string)
    }

    pub(super) fn query_remote_default_branch(&self, remote: &str) -> anyhow::Result<String> {
        let stdout = self.run_command(&["ls-remote", "--symref", remote, "HEAD"])?;
        DefaultBranchName::from_remote(&stdout).map(DefaultBranchName::into_string)
    }

    /// Set the default branch manually.
    ///
    /// This sets worktrunk's cache (`worktrunk.default-branch`). Use `clear` then
    /// `get` to re-detect from remote.
    pub fn set_default_branch(&self, branch: &str) -> anyhow::Result<()> {
        self.run_command(&["config", "worktrunk.default-branch", branch])?;
        Ok(())
    }

    /// Clear the default branch cache.
    ///
    /// Clears worktrunk's cache (`worktrunk.default-branch`). The next call to
    /// `default_branch()` will re-detect (using git's cache or querying remote).
    ///
    /// Returns `true` if cache was cleared, `false` if no cache existed.
    pub fn clear_default_branch_cache(&self) -> anyhow::Result<bool> {
        Ok(self
            .run_command(&["config", "--unset", "worktrunk.default-branch"])
            .is_ok())
    }

    // =========================================================================
    // Project config
    // =========================================================================

    /// Load the project configuration (.config/wt.toml) if it exists.
    ///
    /// Result is cached in the repository's shared cache (same for all clones).
    /// Returns `None` if not in a worktree or if no config file exists.
    pub fn load_project_config(&self) -> anyhow::Result<Option<ProjectConfig>> {
        self.cache
            .project_config
            .get_or_try_init(|| {
                match self.current_worktree().root() {
                    Ok(_) => {
                        ProjectConfig::load(self, true).context("Failed to load project config")
                    }
                    Err(_) => Ok(None), // Not in a worktree, no project config
                }
            })
            .cloned()
    }
}
