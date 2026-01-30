//! Project-aware configuration accessors.
//!
//! These methods on `UserConfig` return the effective configuration for a given
//! project by merging global settings with project-specific overrides.

use std::collections::HashMap;

use crate::config::HooksConfig;
use crate::config::expansion::expand_template;

use super::UserConfig;
use super::merge::{Merge, merge_optional};
use super::sections::{
    CommitConfig, CommitGenerationConfig, ListConfig, MergeConfig, SelectConfig,
};

/// Default worktree path template
fn default_worktree_path() -> String {
    "{{ repo_path }}/../{{ repo }}.{{ branch | sanitize }}".to_string()
}

impl UserConfig {
    /// Returns the worktree path template, falling back to the default if not set.
    pub fn worktree_path(&self) -> String {
        self.configs
            .worktree_path
            .clone()
            .unwrap_or_else(default_worktree_path)
    }

    /// Returns true if the user has explicitly set a custom worktree-path.
    pub fn has_custom_worktree_path(&self) -> bool {
        self.configs.worktree_path.is_some()
    }

    /// Returns the worktree path template for a specific project.
    ///
    /// Checks project-specific config first, falls back to global worktree-path,
    /// and finally to the default template if neither is set.
    pub fn worktree_path_for_project(&self, project: &str) -> String {
        self.projects
            .get(project)
            .and_then(|p| p.overrides.worktree_path.clone())
            .unwrap_or_else(|| self.worktree_path())
    }

    /// Returns the commit generation config for a specific project.
    ///
    /// Merges project-specific settings with global settings, where project
    /// settings take precedence for fields that are set.
    ///
    /// Checks locations in order of precedence:
    /// 1. `[commit.generation]` (new format)
    /// 2. `[commit-generation]` (deprecated format)
    /// 3. Per-project overrides
    pub fn commit_generation(&self, project: Option<&str>) -> CommitGenerationConfig {
        // Get global config: prefer new location, fall back to deprecated
        let global = self
            .configs
            .commit
            .as_ref()
            .and_then(|c| c.generation.as_ref())
            .or(self.commit_generation.as_ref())
            .cloned()
            .unwrap_or_default();

        // Get project override (also checks both locations)
        let project_config = project.and_then(|p| self.projects.get(p)).and_then(|c| {
            c.overrides
                .commit
                .as_ref()
                .and_then(|cc| cc.generation.as_ref())
                .or(c.commit_generation.as_ref())
        });

        match project_config {
            Some(pc) => global.merge_with(pc),
            None => global,
        }
    }

    /// Returns the list config for a specific project.
    ///
    /// Merges project-specific settings with global settings, where project
    /// settings take precedence for fields that are set.
    pub fn list(&self, project: Option<&str>) -> Option<ListConfig> {
        let project_config = project
            .and_then(|p| self.projects.get(p))
            .and_then(|c| c.overrides.list.as_ref());
        merge_optional(self.configs.list.as_ref(), project_config)
    }

    /// Returns the commit config for a specific project.
    ///
    /// Merges project-specific settings with global settings, where project
    /// settings take precedence for fields that are set.
    pub fn commit(&self, project: Option<&str>) -> Option<CommitConfig> {
        let project_config = project
            .and_then(|p| self.projects.get(p))
            .and_then(|c| c.overrides.commit.as_ref());
        merge_optional(self.configs.commit.as_ref(), project_config)
    }

    /// Returns the merge config for a specific project.
    ///
    /// Merges project-specific settings with global settings, where project
    /// settings take precedence for fields that are set.
    pub fn merge(&self, project: Option<&str>) -> Option<MergeConfig> {
        let project_config = project
            .and_then(|p| self.projects.get(p))
            .and_then(|c| c.overrides.merge.as_ref());
        merge_optional(self.configs.merge.as_ref(), project_config)
    }

    /// Returns the select config for a specific project.
    ///
    /// Merges project-specific settings with global settings, where project
    /// settings take precedence for fields that are set.
    pub fn select(&self, project: Option<&str>) -> Option<SelectConfig> {
        let project_config = project
            .and_then(|p| self.projects.get(p))
            .and_then(|c| c.overrides.select.as_ref());
        merge_optional(self.configs.select.as_ref(), project_config)
    }

    /// Returns effective hooks for a specific project.
    ///
    /// Merges global hooks with per-project hooks using append semantics.
    /// Both global and per-project hooks run (global first, then per-project).
    pub fn hooks(&self, project: Option<&str>) -> HooksConfig {
        let global = &self.configs.hooks;
        let project_hooks = project
            .and_then(|p| self.projects.get(p))
            .map(|c| &c.overrides.hooks);

        match project_hooks {
            Some(ph) => global.merge_with(ph),
            None => global.clone(),
        }
    }

    // ---- Resolved config (concrete types with defaults applied) ----

    /// Returns all resolved config with defaults applied.
    ///
    /// Merges global and per-project settings, applying defaults for any unset fields.
    pub fn resolved(&self, project: Option<&str>) -> super::resolved::ResolvedConfig {
        super::resolved::ResolvedConfig::for_project(self, project)
    }

    /// Format a worktree path using this configuration's template.
    ///
    /// # Arguments
    /// * `main_worktree` - Main worktree directory name (replaces {{ main_worktree }} in template)
    /// * `branch` - Branch name (replaces {{ branch }} in template; use `{{ branch | sanitize }}` for paths)
    /// * `repo` - Repository for template function access
    /// * `project` - Optional project identifier (e.g., "github.com/user/repo") to look up
    ///   project-specific worktree-path template
    pub fn format_path(
        &self,
        main_worktree: &str,
        branch: &str,
        repo: &crate::git::Repository,
        project: Option<&str>,
    ) -> Result<String, String> {
        let template = match project {
            Some(p) => self.worktree_path_for_project(p),
            None => self.worktree_path(),
        };
        // Use native path format (not POSIX) since this is used for filesystem operations
        let repo_path = repo.repo_path().to_string_lossy().to_string();
        let mut vars = HashMap::new();
        vars.insert("main_worktree", main_worktree);
        vars.insert("repo", main_worktree);
        vars.insert("branch", branch);
        vars.insert("repo_path", repo_path.as_str());
        expand_template(&template, &vars, false, repo, "worktree-path")
            .map(|p| shellexpand::tilde(&p).into_owned())
    }
}
