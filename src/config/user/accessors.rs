//! Project-aware configuration accessors.
//!
//! These methods on `UserConfig` return the effective configuration for a given
//! project by merging global settings with project-specific overrides.

use std::collections::{BTreeMap, HashMap};

use crate::config::HooksConfig;
use crate::config::commands::CommandConfig;
use crate::config::expansion::expand_template;

use super::UserConfig;
use super::merge::Merge;
use super::sections::{
    CommitConfig, CommitGenerationConfig, CopyIgnoredConfig, ListConfig, MergeConfig, StepConfig,
    SwitchConfig, SwitchPickerConfig,
};

/// Default worktree path template
fn default_worktree_path() -> String {
    "{{ repo_path }}/../{{ repo }}.{{ branch | sanitize }}".to_string()
}

impl UserConfig {
    fn project_overrides(
        &self,
        project: Option<&str>,
    ) -> Option<&super::sections::UserProjectOverrides> {
        project.and_then(|p| self.projects.get(p))
    }

    fn merged_project_config<T: Merge + Clone>(
        &self,
        project: Option<&str>,
        global: &T,
        project_config: impl FnOnce(&super::sections::UserProjectOverrides) -> &T,
    ) -> T {
        match self.project_overrides(project).map(project_config) {
            Some(proj) => global.merge_with(proj),
            None => global.clone(),
        }
    }

    /// Returns the worktree path template, falling back to the default if not set.
    pub fn worktree_path(&self) -> String {
        self.worktree_path
            .clone()
            .unwrap_or_else(default_worktree_path)
    }

    /// Returns true if the user has explicitly set a custom global worktree-path.
    pub fn has_custom_worktree_path(&self) -> bool {
        self.worktree_path.is_some()
    }

    /// Returns true if the given project has an explicit worktree-path override.
    pub fn has_project_worktree_path(&self, project: &str) -> bool {
        self.projects
            .get(project)
            .and_then(|p| p.worktree_path.as_ref())
            .is_some()
    }

    /// Returns the worktree path template for a specific project.
    ///
    /// Checks project-specific config first, falls back to global worktree-path,
    /// and finally to the default template if neither is set.
    pub fn worktree_path_for_project(&self, project: &str) -> String {
        self.projects
            .get(project)
            .and_then(|p| p.worktree_path.clone())
            .unwrap_or_else(|| self.worktree_path())
    }

    /// Returns the commit generation config for a specific project.
    ///
    /// Merges project-specific settings with global settings, where project
    /// settings take precedence for fields that are set. Deprecated
    /// `[commit-generation]` sections are normalized into `[commit.generation]`
    /// during config loading.
    pub fn commit_generation(&self, project: Option<&str>) -> CommitGenerationConfig {
        let global = self.commit.generation.clone().unwrap_or_default();
        match self
            .project_overrides(project)
            .and_then(|config| config.commit.generation.as_ref())
        {
            Some(proj) => global.merge_with(proj),
            None => global,
        }
    }

    /// Returns the list config for a specific project.
    ///
    /// Merges project-specific settings with global settings, where project
    /// settings take precedence for fields that are set.
    pub fn list(&self, project: Option<&str>) -> ListConfig {
        self.merged_project_config(project, &self.list, |config| &config.list)
    }

    /// Returns the commit config for a specific project.
    ///
    /// Merges project-specific settings with global settings, where project
    /// settings take precedence for fields that are set.
    pub fn commit(&self, project: Option<&str>) -> CommitConfig {
        self.merged_project_config(project, &self.commit, |config| &config.commit)
    }

    /// Returns the merge config for a specific project.
    ///
    /// Merges project-specific settings with global settings, where project
    /// settings take precedence for fields that are set.
    pub fn merge(&self, project: Option<&str>) -> MergeConfig {
        self.merged_project_config(project, &self.merge, |config| &config.merge)
    }

    /// Returns the switch config for a specific project.
    ///
    /// Merges project-specific settings with global settings, where project
    /// settings take precedence for fields that are set.
    pub fn switch(&self, project: Option<&str>) -> SwitchConfig {
        self.merged_project_config(project, &self.switch, |config| &config.switch)
    }

    /// Returns the `wt step` config for a specific project.
    pub fn step(&self, project: Option<&str>) -> StepConfig {
        self.merged_project_config(project, &self.step, |config| &config.step)
    }

    /// Returns the `wt step copy-ignored` config for a specific project.
    pub fn copy_ignored(&self, project: Option<&str>) -> CopyIgnoredConfig {
        self.step(project).copy_ignored.unwrap_or_default()
    }

    /// Returns the switch picker config for a specific project.
    ///
    /// Merges project-specific settings with global settings, where project
    /// settings take precedence for fields that are set. Deprecated `[select]`
    /// sections are normalized into `[switch.picker]` during config loading.
    pub fn switch_picker(&self, project: Option<&str>) -> SwitchPickerConfig {
        let global = self.switch.picker.clone().unwrap_or_default();
        match self
            .project_overrides(project)
            .and_then(|config| config.switch.picker.as_ref())
        {
            Some(proj) => global.merge_with(proj),
            None => global,
        }
    }

    /// Returns effective hooks for a specific project.
    ///
    /// Merges global hooks with per-project hooks using append semantics.
    /// Both global and per-project hooks run (global first, then per-project).
    pub fn hooks(&self, project: Option<&str>) -> HooksConfig {
        let global = &self.hooks;
        let project_hooks = self.project_overrides(project).map(|config| &config.hooks);

        match project_hooks {
            Some(ph) => global.merge_with(ph),
            None => global.clone(),
        }
    }

    /// Returns effective aliases for a specific project.
    ///
    /// Merges global user aliases with per-project user aliases using append
    /// semantics: both run on name collision (global first, then per-project).
    pub fn aliases(&self, project: Option<&str>) -> BTreeMap<String, CommandConfig> {
        let mut result = self.aliases.clone();
        if let Some(proj) = self.project_overrides(project) {
            crate::config::commands::append_aliases(&mut result, &proj.aliases);
        }
        result
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
    /// * remote owner/namespace is available as {{ owner }}
    /// * `project` - Optional project identifier (e.g., "github.com/user/repo") to look up
    ///   project-specific worktree-path template
    pub fn format_path(
        &self,
        main_worktree: &str,
        branch: &str,
        repo: &crate::git::Repository,
        project: Option<&str>,
    ) -> anyhow::Result<String> {
        let template = match project {
            Some(p) => self.worktree_path_for_project(p),
            None => self.worktree_path(),
        };
        // Use native path format (not POSIX) since this is used for filesystem operations
        let repo_path = repo.repo_path()?.to_string_lossy().to_string();
        let mut vars = HashMap::new();
        vars.insert("main_worktree", main_worktree);
        vars.insert("repo", main_worktree);
        vars.insert("branch", branch);
        vars.insert("repo_path", repo_path.as_str());
        let owner = repo
            .primary_remote_parsed_url()
            .map(|parsed_remote| parsed_remote.owner().to_string());
        if let Some(ref owner) = owner {
            vars.insert("owner", owner.as_str());
        }
        Ok(
            expand_template(&template, &vars, false, repo, "worktree-path")
                .map(|p| shellexpand::tilde(&p).into_owned())?,
        )
    }
}
