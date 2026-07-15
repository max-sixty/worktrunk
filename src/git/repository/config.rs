//! Git config, hints, marker, and default branch operations for Repository.

use std::path::PathBuf;

use anyhow::Context;
use color_print::cformat;

use crate::config::ProjectConfig;

use crate::git::CommandError;

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
        let args = ["config", "--unset", key];
        let output = self.run_command_output(&args)?;
        let existed = if output.status.success() {
            true
        } else if output.status.code() == Some(5) {
            // --unset exit code 5 = key didn't exist
            false
        } else {
            return Err(CommandError::from_failed_output("git", &args, &output).into());
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
        let args = ["config", "--get-regexp", pattern];
        let output = self.run_command_output(&args)?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else if output.status.code() == Some(1) {
            // Exit 1 = no keys matched the pattern
            Ok(String::new())
        } else {
            Err(CommandError::from_failed_output("git", &args, &output).into())
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
    /// because hook/alias templates render at execution time and depend on
    /// seeing writes that earlier pipeline steps made via their own
    /// `git config` subprocesses. Those external writes don't round-trip
    /// through our coherent `set_config_value` helper.
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
            let Some((branch, key)) = parse_vars_config_key(config_key) else {
                continue;
            };
            result
                .entry(branch.to_string())
                .or_default()
                .insert(key.to_string(), value.to_string());
        }
        result
    }

    /// Get all vars entries across all branches from the bulk config snapshot.
    ///
    /// Unlike [`Self::all_vars_entries`], this reads the in-memory
    /// `Repository::all_config` map and spawns no subprocess. The snapshot
    /// is loaded once per process, so writes made by other processes after
    /// that load are invisible — coherent for a single read like `wt list`
    /// rendering, wrong for hook/alias pipelines that must observe writes
    /// from earlier steps (those use [`Self::vars_entries`]).
    pub fn all_vars_from_snapshot(
        &self,
    ) -> anyhow::Result<std::collections::HashMap<String, std::collections::BTreeMap<String, String>>>
    {
        self.subsection_map_from_snapshot(parse_vars_config_key)
    }

    /// Get all `branch.<name>.*` git config entries across all branches from
    /// the bulk config snapshot.
    ///
    /// Returns a map of branch → (key → value), reading the in-memory
    /// `Repository::all_config` map with no subprocess (see
    /// [`Self::all_vars_from_snapshot`] for the snapshot's coherence model).
    /// This surfaces the keys a user already stores under `branch.<name>.*` in
    /// git config — both convention keys (`branch.<name>.jira`) and the
    /// git-native `branch.<name>.description` — in `wt list` custom columns via
    /// `{{ git.branch.* }}`, the parallel namespace to `{{ vars.* }}`.
    pub fn all_branch_config_from_snapshot(
        &self,
    ) -> anyhow::Result<std::collections::HashMap<String, std::collections::BTreeMap<String, String>>>
    {
        self.subsection_map_from_snapshot(parse_branch_config_key)
    }

    /// Build a branch → (key → value) map from the bulk config snapshot,
    /// splitting each config key with `parse` (which returns `None` to skip a
    /// key). Backs both [`Self::all_vars_from_snapshot`] and
    /// [`Self::all_branch_config_from_snapshot`]. `parse_config_list_z`
    /// guarantees at least one value per key, so the last one is authoritative.
    fn subsection_map_from_snapshot(
        &self,
        parse: fn(&str) -> Option<(&str, &str)>,
    ) -> anyhow::Result<std::collections::HashMap<String, std::collections::BTreeMap<String, String>>>
    {
        let guard = self.all_config()?.read().unwrap();
        let mut result: std::collections::HashMap<
            String,
            std::collections::BTreeMap<String, String>,
        > = std::collections::HashMap::new();
        for (config_key, values) in guard.iter() {
            let Some((branch, key)) = parse(config_key) else {
                continue;
            };
            let value = values.last().cloned().unwrap_or_default();
            result
                .entry(branch.to_string())
                .or_default()
                .insert(key.to_string(), value);
        }
        Ok(result)
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
    /// Hints are stored as `worktrunk.hints.<name> = <count>` (an integer
    /// representing how many times the hint has been shown). The presence of
    /// the key — regardless of value — means the hint has been shown at least
    /// once.
    /// TODO: Could move to global git config if we accumulate more global hints.
    pub fn has_shown_hint(&self, name: &str) -> bool {
        self.config_last(&format!("worktrunk.hints.{name}"))
            .ok()
            .flatten()
            .is_some()
    }

    /// Return how many times a hint has been shown in this repo.
    ///
    /// Returns `0` when the key is missing. Legacy `"true"` values (written
    /// before the counter migration) parse as `0`, so the next
    /// [`mark_hint_shown`] increments them to `1` — the same as a fresh hint.
    /// This loses the "user has seen this before" signal for legacy entries,
    /// which is acceptable: the only consumer of the count is the
    /// shell-integration escalation at 5+, and a legacy user merely starts
    /// their counter from zero. `has_shown_hint` still returns `true` so any
    /// "first time" branches still skip.
    ///
    /// [`mark_hint_shown`]: Self::mark_hint_shown
    pub fn hint_count(&self, name: &str) -> u32 {
        self.config_last(&format!("worktrunk.hints.{name}"))
            .ok()
            .flatten()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0)
    }

    /// Mark a hint as shown in this repo, incrementing its display counter.
    ///
    /// The first call writes `1`; subsequent calls increment. Legacy `"true"`
    /// values (written before the counter migration) and any unparsable
    /// content are treated as `0`, so the next call resets to `1` — see
    /// [`hint_count`] for the rationale.
    ///
    /// [`hint_count`]: Self::hint_count
    pub fn mark_hint_shown(&self, name: &str) -> anyhow::Result<()> {
        let next = self.hint_count(name).saturating_add(1);
        self.set_config_value(&format!("worktrunk.hints.{name}"), &next.to_string())
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
    /// **Network contract — this is the sole "look it up if missing" helper
    /// in worktrunk that is allowed to fall through to the wire.** The first
    /// call per repo may run `git ls-remote` (100 ms–2 s); the result is
    /// then persisted to `worktrunk.default-branch` and every subsequent
    /// call is a local cache hit. No other detection helper may add a
    /// similar fallback. See `CLAUDE.md` → "Network Access" for the policy.
    ///
    /// Detection strategy:
    /// 1. Check worktrunk cache (`git config worktrunk.default-branch`)
    /// 2. Try primary remote's local cache (e.g., `origin/HEAD`)
    /// 3. Query remote (`git ls-remote`) — may take 100 ms–2 s (sole wire fallback)
    /// 4. Infer from local branches if no remote
    ///
    /// Detection results are cached to `worktrunk.default-branch` for future
    /// calls. Result is also cached in the shared repo cache (shared across
    /// all worktrees).
    ///
    /// To minimize latency on the rare cold-clone case:
    /// - Defer calling this until after fast, local checks (see e497f0f for an example).
    /// - Consider passing the result as a parameter if needed multiple times.
    /// - For optional operations, provide a fallback (e.g., `.unwrap_or("main")`).
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
                        .inspect_err(|e| tracing::debug!(error = %e, "Local inference failed: {e}"))
                        .ok()
                });

                // Cache detected result to git config for future runs
                if let Some(ref branch) = detected
                    && let Err(e) = self.set_config_value("worktrunk.default-branch", branch)
                {
                    tracing::debug!(error = %e, "Failed to persist default-branch cache: {e}");
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
                if self.is_unborn_branch(&branch) {
                    return Err(GitError::UnbornDefaultBranch { branch }.into());
                }
                return Err(GitError::StaleDefaultBranch { branch }.into());
            }
            return Err(GitError::BranchNotFound {
                branch,
                show_create_hint: true,
                last_fetch_ago: None,
                pr_mr_platform: self.detect_ref_type(),
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
                if self.is_unborn_branch(&reference) {
                    return Err(GitError::UnbornDefaultBranch { branch: reference }.into());
                }
                return Err(GitError::StaleDefaultBranch { branch: reference }.into());
            }
            return Err(GitError::ReferenceNotFound { reference }.into());
        }
        Ok(reference)
    }

    /// True when `branch` is checked out as an unborn HEAD (no commits yet)
    /// in some worktree — e.g. a freshly `git init`'d repo before its first
    /// commit. Such a branch has no `refs/heads/<branch>` ref, so the
    /// `require_target_*` existence checks fail even though the cached
    /// default branch is correct. Used to surface
    /// [`GitError::UnbornDefaultBranch`] (no cache-reset hint) instead of
    /// [`GitError::StaleDefaultBranch`] (which would wrongly suggest the
    /// cache is bad).
    fn is_unborn_branch(&self, branch: &str) -> bool {
        self.list_worktrees().is_ok_and(|worktrees| {
            worktrees
                .iter()
                .any(|wt| wt.branch.as_deref() == Some(branch) && !wt.has_commits())
        })
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

    /// Read the persisted default-branch cache without detecting or writing.
    ///
    /// Unlike `default_branch()`, this never falls through to detection
    /// (`origin/HEAD`, `git ls-remote`, local inference) and never persists a
    /// result. It reports only what is currently stored in
    /// `worktrunk.default-branch`, so state-inspection commands
    /// (`wt config state`) can display the cache without the act of inspecting
    /// it repopulating the very value being cleared.
    ///
    /// Returns `None` when nothing is cached.
    pub fn cached_default_branch(&self) -> Option<String> {
        self.config_value("worktrunk.default-branch")
            .ok()
            .flatten()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// Read the primary remote's locally-cached HEAD without touching the
    /// network, as `(remote_name, branch)` — the branch `<remote>/HEAD`
    /// resolves to (e.g. `("origin", "main")`).
    ///
    /// Unlike [`default_branch`](Self::default_branch), this never queries the
    /// remote (`git ls-remote`), never infers from local branches, and never
    /// persists a result: it reports only git's own `<remote>/HEAD` symref.
    /// Returns `None` when there is no primary remote or its HEAD is not set
    /// locally (e.g. `git remote set-head` was never run).
    ///
    /// State inspection (`wt config state`) uses this to flag when the
    /// persisted `worktrunk.default-branch` cache has drifted from
    /// `origin/HEAD` — e.g. after a default-branch rename followed by
    /// `git remote set-head origin -a`, which the fast-path cache in
    /// `default_branch()` won't otherwise notice.
    pub fn remote_head(&self) -> Option<(String, String)> {
        let remote = self.primary_remote().ok()?;
        let branch = self.local_default_branch(&remote).ok()?;
        Some((remote, branch))
    }

    // =========================================================================
    // Project config
    // =========================================================================

    /// Return the path for the project config file.
    ///
    /// If `WORKTRUNK_PROJECT_CONFIG_PATH` is set, returns that path (used for
    /// test isolation so the spawned `wt` does not pick up this repo's
    /// `.config/wt.toml`). An empty value means no project config. A relative
    /// value resolves against the same worktree root the default
    /// `.config/wt.toml` is anchored to — never the process cwd, which would
    /// make the override silently depend on the invocation directory — and
    /// errors when no worktree root exists to anchor it. A missing file at the
    /// resulting path still resolves to `Ok(None)` via `ProjectConfig::load`,
    /// matching the no-config case.
    ///
    /// Without the override: uses the current worktree when inside one (both
    /// normal and bare repos). For bare repos at the bare root (outside any
    /// worktree), falls back to the primary worktree. When the default branch
    /// is checked out in no worktree (so `primary_worktree()` is `None`),
    /// there is no on-disk path to return here — `ProjectConfig::load` reads
    /// the committed default-branch config from the object store via
    /// [`default_branch_project_config_content`](Self::default_branch_project_config_content)
    /// so project config (and every project hook) isn't silently dropped while
    /// the primary is parked on another branch (#3461). That fallback is for
    /// the no-override case only: an override always names the config source
    /// outright.
    ///
    /// "The current worktree" is whatever this `Repository` was rooted at, so
    /// the answer to "which `.config/wt.toml` does a hook read" is decided by
    /// the caller's choice of root. That policy — which worktree each hook type
    /// is resolved against — is the spec in the `commands::hooks` module docs
    /// (`src/commands/hooks.rs`).
    pub fn project_config_path(&self) -> anyhow::Result<Option<PathBuf>> {
        let override_path = std::env::var_os("WORKTRUNK_PROJECT_CONFIG_PATH").map(PathBuf::from);
        if let Some(path) = &override_path {
            if path.as_os_str().is_empty() {
                // An empty override means no project config, matching a
                // missing file at the override path.
                return Ok(None);
            }
            if path.is_absolute() {
                return Ok(Some(path.clone()));
            }
            // Windows-only forms that are neither absolute nor purely
            // relative — drive-relative (`C:cfg`) or rooted without a drive
            // (`\cfg`, `/tmp/x`) — would resolve against the process drive or
            // replace the anchor under `Path::join`; reject them rather than
            // silently keep the cwd dependence this resolution exists to
            // eliminate.
            #[cfg(windows)]
            if path.has_root()
                || matches!(
                    path.components().next(),
                    Some(std::path::Component::Prefix(_))
                )
            {
                anyhow::bail!(
                    "WORKTRUNK_PROJECT_CONFIG_PATH ({}) is neither fully absolute nor relative; use an absolute path including the drive",
                    path.display()
                );
            }
        }

        // Batched rev-parse: asks `--is-inside-work-tree` and also pre-warms
        // the worktree root / git-dir / branch caches, sparing three later
        // forks on the typical alias path.
        let info = self.current_worktree().prewarm_info().unwrap_or_default();

        // Inside a worktree — use it (normal repo or linked worktree in bare
        // repo; `root` is `Some` iff the batch saw us inside a work tree). At
        // the bare root, fall back to the primary worktree (the one holding
        // the default branch); when the default branch is checked out in no
        // worktree, there is no root and no on-disk path — `ProjectConfig::load`
        // then reads the committed default-branch config from the object store
        // via `default_branch_project_config_content` (#3461).
        let root = match info.root {
            Some(root) => Some(root),
            None if self.is_bare().unwrap_or(false) => self.primary_worktree()?,
            None => None,
        };

        let Some(relative) = override_path else {
            return Ok(root.map(|root| root.join(".config").join("wt.toml")));
        };
        let Some(root) = root else {
            anyhow::bail!(
                "WORKTRUNK_PROJECT_CONFIG_PATH is relative ({}) but there is no worktree root to resolve it against; use an absolute path",
                relative.display()
            );
        };
        Ok(Some(root.join(relative)))
    }

    /// Content of the default branch's committed `.config/wt.toml`, read from
    /// the object store via `git show`, for the one state where the on-disk
    /// path can't supply it: a bare repo whose default branch is checked out in
    /// no worktree.
    ///
    /// In a bare layout the primary worktree normally holds the default branch,
    /// so its on-disk `.config/wt.toml` *is* the default branch's project
    /// config and [`project_config_path`](Self::project_config_path) resolves
    /// it directly. When the primary worktree is transiently parked on another
    /// branch (a common agent-driven workflow), no worktree exposes the default
    /// branch's config on disk; returning nothing there would silently drop the
    /// entire project config and every project hook (#3461). Reading the
    /// committed copy from the object store restores it without depending on
    /// which branch is checked out where — and, unlike scanning worktrees for
    /// any `.config/wt.toml`, always reads the *default branch's* config rather
    /// than whatever branch a worktree happens to be parked on.
    ///
    /// Returns `None` cheaply — before touching the object store — for every
    /// other repo shape (non-bare repos, and bare repos whose default branch is
    /// checked out somewhere), so the common load path never pays for the extra
    /// `git show`. Also returns `None` when the default branch ships no project
    /// config (`git show` exits non-zero for a path absent from the tree),
    /// matching the no-config case.
    ///
    /// This is a best-effort resolver: the `is_bare` / `primary_worktree`
    /// checks re-run calls that `project_config_path` already made
    /// successfully on this path, and a `git show` failure degrades to "no
    /// project config" (no hooks) rather than surfacing — the same
    /// error-swallowing shape `alias.rs` uses for config resolution, and safe
    /// because the fallback only ever adds hooks, never risks data.
    ///
    /// The read resolves the default branch **by name**
    /// (`<default-branch>:.config/wt.toml`), not via `HEAD`. `git show` resolves
    /// `HEAD` against the invocation cwd's per-worktree HEAD, and this runs with
    /// `discovery_path` as cwd — a linked worktree parked on some other branch
    /// when `wt` is invoked from inside one (the common agent case). Reading
    /// `HEAD` there would read that worktree's branch, dropping or mis-sourcing
    /// the default branch's config; an absolute branch ref is cwd-independent.
    /// The returned `PathBuf` is a display-only label of the form
    /// `<default-branch>:.config/wt.toml` — a git revision spec, not a
    /// filesystem path. Nothing is read from or written to it; it only
    /// annotates diagnostics (e.g. a parse error) with the object-store source.
    pub fn default_branch_project_config_content(&self) -> Option<(String, PathBuf)> {
        // An explicit WORKTRUNK_PROJECT_CONFIG_PATH override names the config
        // source outright — an empty value or a missing file at the override
        // path means no project config — so the committed fallback must not
        // supersede it (the override exists for test isolation, where reading
        // the repo's own committed config is exactly the leak being prevented).
        if std::env::var_os("WORKTRUNK_PROJECT_CONFIG_PATH").is_some() {
            return None;
        }
        if !self.is_bare().unwrap_or(false) {
            return None;
        }
        // Only the "default branch checked out nowhere" state needs this; when
        // the default branch is checked out somewhere, its on-disk path already
        // resolved (and stays authoritative — e.g. a deletion there wins).
        if self.primary_worktree().ok().flatten().is_some() {
            return None;
        }

        let spec = format!("{}:.config/wt.toml", self.default_branch()?);
        match self.run_command_output(&["show", &spec]) {
            Ok(output) if output.status.success() => Some((
                String::from_utf8_lossy(&output.stdout).into_owned(),
                PathBuf::from(&spec),
            )),
            // A non-zero exit (typically 128, path absent from the tree) or a
            // rare spawn failure: treat as "no project config", the same result
            // as an absent file on disk.
            _ => None,
        }
    }

    /// Load the project configuration (.config/wt.toml) if it exists.
    ///
    /// Result is cached in the repository's shared cache (same for all clones).
    /// Returns `None` if not in a worktree or if no config file exists.
    ///
    /// Returns an owned clone — use [`project_config`](Self::project_config)
    /// when a borrow suffices, to avoid the clone.
    pub fn load_project_config(&self) -> anyhow::Result<Option<ProjectConfig>> {
        Ok(self.project_config()?.cloned())
    }

    /// Borrow the cached project configuration.
    ///
    /// Same caching semantics as [`load_project_config`](Self::load_project_config),
    /// but returns a reference into the cache. Mirrors
    /// [`user_config`](Self::user_config) so callers that consume both can
    /// pull them through the same borrow-from-cache shape.
    ///
    /// Unlike `user_config`, this does **not** participate in
    /// [`Repository::prewarm`] — `.config/wt.toml` lives inside the
    /// worktree, so the read can't fire until git discovery finishes.
    /// Callers pay a few-tens-of-µs sequential file read on first access,
    /// and deprecation warnings (if any) emit on that first call.
    ///
    /// User and project configs are kept distinct rather than merged
    /// because downstream consumers (alias loading, hook resolution,
    /// approval policy) apply per-source rules.
    pub fn project_config(&self) -> anyhow::Result<Option<&ProjectConfig>> {
        self.cache
            .project_config
            .get_or_try_init(|| match self.current_worktree().root() {
                Ok(_) => ProjectConfig::load(self, true).context("Failed to load project config"),
                Err(_) => Ok(None), // Not in a worktree, no project config
            })
            .map(Option::as_ref)
    }
}

/// Split a `worktrunk.state.<branch>.vars.<key>` config key into `(branch, key)`.
///
/// Uses `rsplit_once`: var keys cannot contain dots (validated by
/// `validate_vars_key`), so the last `.vars.` is always the real separator.
/// `split_once` would misparse branch names containing `.vars.`.
fn parse_vars_config_key(config_key: &str) -> Option<(&str, &str)> {
    config_key
        .strip_prefix("worktrunk.state.")?
        .rsplit_once(".vars.")
}

/// Split a `branch.<name>.<key>` config key into `(branch, key)`.
///
/// Uses `rsplit_once`: git config variable names cannot contain dots, so the
/// last `.` always separates the branch subsection (which may itself contain
/// dots or slashes, e.g. `feature.foo`, `feature/bar`) from the variable name.
/// Git's own section-level branch settings have no subsection and so flatten to
/// two segments (`branch.sort`, `branch.autoSetupMerge`); after stripping the
/// prefix they hold no `.`, so `rsplit_once` yields `None` and they're skipped.
fn parse_branch_config_key(config_key: &str) -> Option<(&str, &str)> {
    config_key.strip_prefix("branch.")?.rsplit_once('.')
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
    fn test_get_config_regexp_failure_is_command_error() {
        // A real failure (invalid pattern, exit 6) must surface as a typed
        // `CommandError` — unlike exit 1, which means "no keys matched".
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        let err = repo.get_config_regexp("(").unwrap_err();
        let cmd_err = CommandError::find_in(&err).expect("error should carry a CommandError");
        assert_eq!(cmd_err.command_string(), "git config --get-regexp (");
    }

    #[test]
    fn test_unset_config_failure_is_command_error() {
        // A real failure (invalid key, exit 1) must surface as a typed
        // `CommandError` — unlike exit 5, which means "key didn't exist".
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        let err = repo.unset_config("inva lid.key").unwrap_err();
        let cmd_err = CommandError::find_in(&err).expect("error should carry a CommandError");
        assert_eq!(cmd_err.command_string(), "git config --unset inva lid.key");
    }

    #[test]
    fn test_config_read_failure_is_command_error() {
        // Corrupting the config after the repository is open (the bulk map
        // populates lazily) makes `git config --list -z` fail — the failure
        // must surface as a typed `CommandError`.
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();
        std::fs::write(test.root_path().join(".git/config"), "[bad\n").unwrap();

        let err = repo.config_value("user.name").unwrap_err();
        let cmd_err = CommandError::find_in(&err).expect("error should carry a CommandError");
        assert_eq!(cmd_err.command_string(), "git config --list -z");
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

    /// `mark_hint_shown` writes 1 on first call and increments on each
    /// subsequent call. `hint_count` returns 0 for missing keys and the
    /// current count otherwise; `clear_hint` resets the counter back to 0.
    #[test]
    fn test_hint_count_increments_and_clears() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        // Missing key -> 0, and "shown" predicate is false.
        assert_eq!(repo.hint_count("escalating"), 0);
        assert!(!repo.has_shown_hint("escalating"));

        // First mark records 1.
        repo.mark_hint_shown("escalating").unwrap();
        assert_eq!(repo.hint_count("escalating"), 1);
        assert!(repo.has_shown_hint("escalating"));

        // Subsequent marks increment.
        repo.mark_hint_shown("escalating").unwrap();
        assert_eq!(repo.hint_count("escalating"), 2);
        repo.mark_hint_shown("escalating").unwrap();
        assert_eq!(repo.hint_count("escalating"), 3);

        // Clearing resets the counter (no `(--unset)` survivors poisoning future reads).
        assert!(repo.clear_hint("escalating").unwrap());
        assert_eq!(repo.hint_count("escalating"), 0);
        assert!(!repo.has_shown_hint("escalating"));
    }

    /// Legacy `worktrunk.hints.<name> = "true"` values (written before the
    /// counter migration) parse as 0, so `mark_hint_shown` resets them to 1.
    /// This is the documented migration choice — a legacy user starts their
    /// counter from zero, but `has_shown_hint` continues to report `true` so
    /// any "first-time only" branches still treat them as having seen it.
    #[test]
    fn test_hint_count_legacy_true_resets_to_one() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        // Simulate a pre-migration repo by writing the literal "true" value.
        repo.set_config("worktrunk.hints.legacy", "true").unwrap();
        assert!(repo.has_shown_hint("legacy"));
        assert_eq!(repo.hint_count("legacy"), 0);

        // Next mark normalises to 1 (not 2 — legacy "shown once" is dropped).
        repo.mark_hint_shown("legacy").unwrap();
        assert_eq!(repo.hint_count("legacy"), 1);

        // From there, normal increment behaviour resumes.
        repo.mark_hint_shown("legacy").unwrap();
        assert_eq!(repo.hint_count("legacy"), 2);
    }

    /// The snapshot read must parse the same entries as the subprocess read,
    /// including branch names that themselves contain `.vars.`.
    #[test]
    fn test_all_vars_from_snapshot_matches_subprocess_read() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        repo.set_config("worktrunk.state.feature.vars.ticket", "JIRA-1")
            .unwrap();
        repo.set_config("worktrunk.state.feature.vars.note", "a note")
            .unwrap();
        repo.set_config("worktrunk.state.weird.vars.branch.vars.key", "v")
            .unwrap();

        let snapshot = repo.all_vars_from_snapshot().unwrap();
        assert_eq!(snapshot, repo.all_vars_entries());
        assert_eq!(snapshot["feature"]["ticket"], "JIRA-1");
        assert_eq!(snapshot["feature"]["note"], "a note");
        assert_eq!(snapshot["weird.vars.branch"]["key"], "v");
    }

    /// The branch-config snapshot read parses `branch.<name>.*` keys, including
    /// branch names containing dots or slashes, lowercases the variable name as
    /// git does, and skips git's own section-level branch settings
    /// (`branch.sort`), which have no subsection.
    #[test]
    fn test_all_branch_config_from_snapshot() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        repo.set_config("branch.feature.jira", "PROJ-1").unwrap();
        repo.set_config("branch.feature.foo.jira", "PROJ-2")
            .unwrap();
        repo.set_config("branch.feature/bar.jira", "PROJ-3")
            .unwrap();
        // Git lowercases variable names; a mixed-case key reads back lowered.
        repo.set_config("branch.feature.nvciShelf", "64645277")
            .unwrap();
        // A section-level branch setting has no subsection — must be skipped.
        repo.set_config("branch.sort", "-committerdate").unwrap();

        let snapshot = repo.all_branch_config_from_snapshot().unwrap();
        assert_eq!(snapshot["feature"]["jira"], "PROJ-1");
        assert_eq!(snapshot["feature"]["nvcishelf"], "64645277");
        // Dotted and slashed branch names keep their full subsection.
        assert_eq!(snapshot["feature.foo"]["jira"], "PROJ-2");
        assert_eq!(snapshot["feature/bar"]["jira"], "PROJ-3");
        // `branch.sort` is git's own key, not a per-branch entry.
        assert!(!snapshot.contains_key("sort"));
    }
}
