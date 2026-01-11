//! Git config, hints, and marker operations for Repository.

use anyhow::Context;

use crate::config::ProjectConfig;

use super::Repository;

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
