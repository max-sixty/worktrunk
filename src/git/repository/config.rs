//! Git config, hints, marker, and default branch operations for Repository.

use std::path::PathBuf;

use anyhow::Context;
use color_print::cformat;

use crate::config::ProjectConfig;

use super::{DefaultBranchName, GitError, Repository};

impl Repository {
    /// Get a git config value. Returns None if the key doesn't exist.
    ///
    /// Reads from the bulk config map populated by the private
    /// `all_config()` accessor. Returns an error only if the bulk read
    /// itself fails (corrupt config, git subprocess failure).
    pub fn config_value(&self, key: &str) -> anyhow::Result<Option<String>> {
        self.config_last(key)
    }

    /// Set a git config value.
    ///
    /// Writes to the on-disk config AND updates the bulk config map if
    /// populated, so subsequent in-process reads see the new value.
    pub fn set_config(&self, key: &str, value: &str) -> anyhow::Result<()> {
        self.set_config_value(key, value)
    }

    /// Unset a git config value.
    ///
    /// Returns `true` if the key was cleared, `false` if it didn't exist.
    /// Removes the key from the bulk config map if populated.
    pub fn unset_config(&self, key: &str) -> anyhow::Result<bool> {
        self.unset_config_value(key)
    }

    /// Write a config value and keep the bulk config map coherent.
    ///
    /// Every writer in the codebase routes through this helper so that a
    /// `set` followed by a `get` in the same process sees the new value —
    /// the bulk map is updated in-memory when populated, in addition to the
    /// on-disk `git config` write.
    ///
    /// Map keys are canonicalized to match git's emitted form (section +
    /// variable lowercased, subsection preserved) so writes with mixed-case
    /// variable names (e.g. `branch.<name>.pushRemote`) land under the same
    /// key `config_last` looks up. Without this, a later `--list -z`
    /// populate would emit the canonical form while this insert would leave
    /// behind a stale duplicate under the literal form.
    pub(super) fn set_config_value(&self, key: &str, value: &str) -> anyhow::Result<()> {
        self.run_command(&["config", key, value])?;
        if let Some(lock) = self.cache.all_config.get() {
            let canonical = super::canonical_config_key(key);
            lock.write()
                .unwrap()
                .insert(canonical, vec![value.to_string()]);
        }
        Ok(())
    }

    /// Unset a config key and keep the bulk config map coherent.
    ///
    /// Returns `true` if the key was cleared, `false` if it didn't exist.
    /// Propagates actual git config errors (corrupt config, permission denied).
    ///
    /// Removes the canonical form (matching what git emits) to stay in
    /// sync with `set_config_value` and `config_last`.
    pub(super) fn unset_config_value(&self, key: &str) -> anyhow::Result<bool> {
        let output = self.run_command_output(&["config", "--unset", key])?;
        let existed = if output.status.success() {
            true
        } else if output.status.code() == Some(5) {
            // --unset exit code 5 = key didn't exist
            false
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git config --unset {}: {}", key, stderr.trim());
        };
        if let Some(lock) = self.cache.all_config.get() {
            // `shift_remove` preserves remaining order (swap_remove would
            // reorder); order matters for `primary_remote` which picks the
            // first remote with a configured URL.
            let canonical = super::canonical_config_key(key);
            lock.write().unwrap().shift_remove(&canonical);
        }
        Ok(existed)
    }

    /// Run `git config --get-regexp <pattern>` and return stdout.
    ///
    /// Distinguishes exit 1 (no matching keys — expected, returns empty
    /// string) from real config errors (corrupt config, permission denied —
    /// surfaced as `Err`). Use this instead of `run_command` + `.unwrap_or_default()`,
    /// which conflates the two.
    pub fn get_config_regexp(&self, pattern: &str) -> anyhow::Result<String> {
        let output = self.run_command_output(&["config", "--get-regexp", pattern])?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else if output.status.code() == Some(1) {
            // Exit 1 = no keys matched the pattern
            Ok(String::new())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git config --get-regexp {}: {}", pattern, stderr.trim());
        }
    }

    /// Read a user-defined marker from `worktrunk.state.<branch>.marker` in git config.
    ///
    /// Markers are stored as JSON: `{"marker": "text", "set_at": unix_timestamp}`.
    pub fn branch_marker(&self, branch: &str) -> Option<String> {
        #[derive(serde::Deserialize)]
        struct MarkerValue {
            marker: Option<String>,
        }

        let raw = self
            .config_last(&format!("worktrunk.state.{branch}.marker"))
            .ok()
            .flatten()
            .filter(|s| !s.is_empty())?;
        let parsed: MarkerValue = serde_json::from_str(&raw).ok()?;
        parsed.marker
    }

    /// Read user-defined branch-keyed marker.
    pub fn user_marker(&self, branch: Option<&str>) -> Option<String> {
        branch.and_then(|branch| self.branch_marker(branch))
    }

    /// Get all vars entries for a branch, sorted by key name.
    ///
    /// Returns a `BTreeMap` so it serializes to a minijinja object for template access
    /// via `{{ vars.key }}`.
    ///
    /// Reads git config directly — **not** via the bulk `all_config` cache —
    /// because lazy template expansion in hook/alias pipelines depends on
    /// seeing writes that earlier steps made via their own `git config`
    /// subprocesses. Those external writes don't round-trip through our
    /// coherent `set_config_value` helper.
    pub fn vars_entries(&self, branch: &str) -> std::collections::BTreeMap<String, String> {
        let escaped = regex::escape(branch);
        let pattern = format!(r"^worktrunk\.state\.{escaped}\.vars\.");
        let output = self.get_config_regexp(&pattern).unwrap_or_default();

        let prefix = format!("worktrunk.state.{branch}.vars.");
        output
            .lines()
            .filter_map(|line| {
                let (config_key, value) = line.split_once(' ')?;
                let key = config_key.strip_prefix(&prefix)?;
                Some((key.to_string(), value.to_string()))
            })
            .collect()
    }

    /// Get all vars entries across all branches in a single git call.
    ///
    /// Returns a map of branch → (key → value). Reads git config directly
    /// (see [`Self::vars_entries`] for rationale) but still uses one
    /// `git config --get-regexp` rather than N per-branch calls.
    pub fn all_vars_entries(
        &self,
    ) -> std::collections::HashMap<String, std::collections::BTreeMap<String, String>> {
        let output = self
            .get_config_regexp(r"^worktrunk\.state\..+\.vars\.")
            .unwrap_or_default();

        let mut result: std::collections::HashMap<
            String,
            std::collections::BTreeMap<String, String>,
        > = std::collections::HashMap::new();
        for line in output.lines() {
            let Some((config_key, value)) = line.split_once(' ') else {
                continue;
            };
            let Some(rest) = config_key.strip_prefix("worktrunk.state.") else {
                continue;
            };
            // Use rsplit_once: var keys cannot contain dots (validated by
            // validate_vars_key), so the last `.vars.` is always the real separator.
            // split_once would misparse branch names containing `.vars.`.
            let Some((branch, key)) = rest.rsplit_once(".vars.") else {
                continue;
            };
            result
                .entry(branch.to_string())
                .or_default()
                .insert(key.to_string(), value.to_string());
        }
        result
    }

    /// Set the previous branch in worktrunk.history for `wt switch -` support.
    ///
    /// Stores the branch we're switching FROM, so `wt switch -` can return to it.
    pub fn set_switch_previous(&self, previous: Option<&str>) -> anyhow::Result<()> {
        if let Some(prev) = previous {
            self.set_config_value("worktrunk.history", prev)?;
        }
        // If previous is None (detached HEAD), don't update history
        Ok(())
    }

    /// Get the previous branch from worktrunk.history for `wt switch -`.
    ///
    /// Returns the branch we came from, enabling ping-pong switching.
    pub fn switch_previous(&self) -> Option<String> {
        self.config_last("worktrunk.history")
            .ok()
            .flatten()
            .filter(|s| !s.is_empty())
    }

    /// Check if a hint has been shown in this repo.
    ///
    /// Hints are stored as `worktrunk.hints.<name> = true`.
    /// TODO: Could move to global git config if we accumulate more global hints.
    pub fn has_shown_hint(&self, name: &str) -> bool {
        self.config_last(&format!("worktrunk.hints.{name}"))
            .ok()
            .flatten()
            .is_some()
    }

    /// Mark a hint as shown in this repo.
    pub fn mark_hint_shown(&self, name: &str) -> anyhow::Result<()> {
        self.set_config_value(&format!("worktrunk.hints.{name}"), "true")
    }

    /// Clear a hint so it will show again.
    ///
    /// Returns `true` if the hint was cleared, `false` if it didn't exist.
    /// Propagates actual git config errors (corrupt config, permission denied).
    pub fn clear_hint(&self, name: &str) -> anyhow::Result<bool> {
        self.unset_config_value(&format!("worktrunk.hints.{name}"))
    }

    /// List all hints that have been shown in this repo.
    ///
    /// Output is sorted alphabetically so hints render in a predictable
    /// user-facing order regardless of git's config file layout.
    pub fn list_shown_hints(&self) -> Vec<String> {
        let Ok(lock) = self.all_config() else {
            return Vec::new();
        };
        let guard = lock.read().unwrap();
        let mut hints: Vec<String> = guard
            .keys()
            .filter_map(|k| k.strip_prefix("worktrunk.hints.").map(String::from))
            .collect();
        hints.sort();
        hints
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
    /// Detection strategy:
    /// 1. Check worktrunk cache (`git config worktrunk.default-branch`)
    /// 2. Try primary remote's local cache (e.g., `origin/HEAD`)
    /// 3. Query remote (`git ls-remote`) — may take 100ms-2s
    /// 4. Infer from local branches if no remote
    ///
    /// Detection results are cached to `worktrunk.default-branch` for future calls.
    /// Result is also cached in the shared repo cache (shared across all worktrees).
    ///
    /// Returns `None` if the default branch cannot be determined.
    pub fn default_branch(&self) -> Option<String> {
        self.cache
            .default_branch
            .get_or_init(|| {
                // Fast path: trust the persisted value without re-validating
                // that the branch still resolves locally. This avoids an
                // extra fork on every command, at the cost of surfacing a
                // clearer error downstream (see GitError::StaleDefaultBranch)
                // when the configured branch was deleted externally.
                let configured = self
                    .config_last("worktrunk.default-branch")
                    .ok()
                    .flatten()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                if let Some(branch) = configured {
                    return Some(branch);
                }

                // Detect: try remote, then local inference
                let detected = self.detect_from_remote().or_else(|| {
                    self.infer_default_branch_locally()
                        .inspect_err(|e| log::debug!("Local inference failed: {e}"))
                        .ok()
                });

                // Cache detected result to git config for future runs
                if let Some(ref branch) = detected
                    && let Err(e) = self.set_config_value("worktrunk.default-branch", branch)
                {
                    log::debug!("Failed to persist default-branch cache: {e}");
                }

                detected
            })
            .clone()
    }

    /// Try to detect default branch from remote.
    fn detect_from_remote(&self) -> Option<String> {
        let remote = self.primary_remote().ok()?;

        // Try git's local cache for this remote (e.g., origin/HEAD)
        if let Ok(branch) = self.local_default_branch(&remote) {
            return Some(branch);
        }

        // Query remote directly (may be slow)
        self.query_remote_default_branch(&remote).ok()
    }

    /// Resolve a target branch from an optional override
    ///
    /// If target is Some, expands special symbols ("@", "-", "^") via `resolve_worktree_name`.
    /// Otherwise, queries the default branch.
    /// This is a common pattern used throughout commands that accept an optional --target flag.
    ///
    /// Note: This does not validate that the target exists. Use `require_target_branch` or
    /// `require_target_ref` for validation before approval prompts.
    pub fn resolve_target_branch(&self, target: Option<&str>) -> anyhow::Result<String> {
        match target {
            Some(b) => self.resolve_worktree_name(b),
            None => self.default_branch().ok_or_else(|| {
                GitError::Other {
                    message: cformat!(
                        "Cannot determine default branch. Specify target explicitly or run <bold>wt config state default-branch set BRANCH</>"
                    ),
                }
                .into()
            }),
        }
    }

    /// Resolve and validate a target that must be a branch.
    ///
    /// Use this for commands that update a branch ref (merge, push).
    /// Validates before approval prompts to avoid wasting user time.
    ///
    /// When `target` is `None` (resolving via the cached default branch)
    /// and the resolved branch doesn't exist, surfaces
    /// [`GitError::StaleDefaultBranch`] with cache-reset hints rather than
    /// the generic "branch not found" — the user didn't type that name,
    /// the persisted cache did.
    pub fn require_target_branch(&self, target: Option<&str>) -> anyhow::Result<String> {
        let branch = self.resolve_target_branch(target)?;
        if !self.branch(&branch).exists()? {
            if target.is_none() {
                return Err(GitError::StaleDefaultBranch { branch }.into());
            }
            return Err(GitError::BranchNotFound {
                branch,
                show_create_hint: true,
                last_fetch_ago: None,
            }
            .into());
        }
        Ok(branch)
    }

    /// Resolve and validate a target that can be any commit-ish.
    ///
    /// Use this for commands that reference a commit (rebase, squash).
    /// Validates before approval prompts to avoid wasting user time.
    ///
    /// When `target` is `None` (resolving via the cached default branch)
    /// and the resolved reference doesn't exist, surfaces
    /// [`GitError::StaleDefaultBranch`] with cache-reset hints rather than
    /// the generic "reference not found".
    pub fn require_target_ref(&self, target: Option<&str>) -> anyhow::Result<String> {
        let reference = self.resolve_target_branch(target)?;
        if !self.ref_exists(&reference)? {
            if target.is_none() {
                return Err(GitError::StaleDefaultBranch { branch: reference }.into());
            }
            return Err(GitError::ReferenceNotFound { reference }.into());
        }
        Ok(reference)
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
        let branches = self.all_branches()?;
        if branches.len() == 1 {
            return Ok(branches[0].clone());
        }

        // 2. Check symbolic-ref HEAD - authoritative for bare repos and empty repos
        // - Bare repo directory: HEAD always points to the default branch
        // - Empty repos: No branches exist yet, but HEAD tells us the intended default
        // - Linked worktrees: HEAD points to CURRENT branch, so skip this heuristic
        // - Normal repos: HEAD points to CURRENT branch, so skip this heuristic
        let is_bare = self.is_bare()?;
        let in_linked_worktree = self.current_worktree().is_linked()?;
        if ((is_bare && !in_linked_worktree) || branches.is_empty())
            && let Ok(head_ref) = self.run_command(&["symbolic-ref", "HEAD"])
            && let Some(branch) = head_ref.trim().strip_prefix("refs/heads/")
        {
            return Ok(branch.to_string());
        }

        // 3. Check git config init.defaultBranch (if branch exists)
        if let Ok(Some(default)) = self.config_last("init.defaultBranch") {
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

    fn local_default_branch(&self, remote: &str) -> anyhow::Result<String> {
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
        self.set_config_value("worktrunk.default-branch", branch)
    }

    /// Clear the default branch cache.
    ///
    /// Clears worktrunk's cache (`worktrunk.default-branch`). The next call to
    /// `default_branch()` will re-detect (using git's cache or querying remote).
    ///
    /// Returns `true` if cache was cleared, `false` if no cache existed.
    /// Propagates actual git config errors (corrupt config, permission denied).
    pub fn clear_default_branch_cache(&self) -> anyhow::Result<bool> {
        self.unset_config_value("worktrunk.default-branch")
    }

    // =========================================================================
    // Project config
    // =========================================================================

    /// Return the path for the project config file.
    ///
    /// If `WORKTRUNK_PROJECT_CONFIG_PATH` is set, returns that path (used for
    /// test isolation so the spawned `wt` does not pick up this repo's
    /// `.config/wt.toml`). A missing file at that path still resolves to
    /// `Ok(None)` via `ProjectConfig::load`, matching the no-config case.
    ///
    /// Otherwise: uses the current worktree when inside one (both normal and
    /// bare repos). For bare repos at the bare root (outside any worktree),
    /// falls back to the primary worktree. Returns `None` when no worktree can
    /// be determined (bare repo with no linked worktrees).
    pub fn project_config_path(&self) -> anyhow::Result<Option<PathBuf>> {
        if let Ok(path) = std::env::var("WORKTRUNK_PROJECT_CONFIG_PATH") {
            return Ok(Some(PathBuf::from(path)));
        }

        // Batched rev-parse: asks `--is-inside-work-tree` and also pre-warms
        // the worktree root / git-dir / branch / HEAD-SHA caches, sparing
        // four later forks on the typical alias path.
        let info = self.current_worktree().prewarm_info().unwrap_or_default();

        if let Some(root) = info.root {
            // Inside a worktree — use it (normal repo or linked worktree in
            // bare repo). `root` is `Some` iff the batch saw us inside a work
            // tree, so no separate `is_inside` check.
            return Ok(Some(root.join(".config").join("wt.toml")));
        }

        if self.is_bare().unwrap_or(false) {
            // At bare repo root — use primary worktree
            return Ok(self
                .primary_worktree()?
                .map(|p| p.join(".config").join("wt.toml")));
        }

        Ok(None)
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestRepo;

    #[test]
    fn test_get_config_regexp_no_match_returns_empty() {
        // Exit 1 from git config --get-regexp means "no keys matched" — must
        // surface as Ok("") rather than an error so callers don't conflate
        // no-matches with real config failures.
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        let output = repo
            .get_config_regexp(r"^worktrunk\.state\..+\.marker$")
            .unwrap();
        assert_eq!(output, "");
    }

    #[test]
    fn test_get_config_regexp_returns_matches() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        repo.set_config("worktrunk.state.feature.marker", r#"{"marker":"wip"}"#)
            .unwrap();
        repo.set_config("worktrunk.state.bugfix.marker", r#"{"marker":"fix"}"#)
            .unwrap();

        let output = repo
            .get_config_regexp(r"^worktrunk\.state\..+\.marker$")
            .unwrap();
        assert!(output.contains("worktrunk.state.feature.marker"));
        assert!(output.contains("worktrunk.state.bugfix.marker"));
    }
}
