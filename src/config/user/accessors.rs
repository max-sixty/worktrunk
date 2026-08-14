//! Project-aware configuration accessors.
//!
//! These methods on `UserConfig` return the effective configuration for a given
//! project by merging global settings with project-specific overrides.

use std::collections::{BTreeMap, HashMap};

use crate::config::HooksConfig;
use crate::config::commands::CommandConfig;
use crate::config::expansion::expand_template;
use crate::shell_exec::ShellEscapeMode;

use super::UserConfig;
use super::merge::Merge;
use super::sections::{
    CommitConfig, CommitGenerationConfig, CopyIgnoredConfig, ListConfig, MergeConfig, RemoveConfig,
    StepConfig, SwitchConfig, SwitchPickerConfig,
};

/// Default worktree path template
fn default_worktree_path() -> String {
    "{{ repo_path }}/../{{ repo }}.{{ branch | sanitize }}".to_string()
}

impl UserConfig {
    /// Every `[projects."…"]` entry applying to `project`, least- to
    /// most-specific.
    ///
    /// A key matches literally or as a `*` pattern, so one entry can carry
    /// settings for a whole host. Callers apply the entries in order, letting
    /// the most specific win — see [`super::project_match`] for the matching
    /// and ordering rules.
    fn project_overrides(
        &self,
        project: Option<&str>,
    ) -> Vec<&super::sections::UserProjectOverrides> {
        project.map_or_else(Vec::new, |p| {
            super::project_match::matching_keys(&self.projects, p)
        })
    }

    fn merged_project_config<T: Merge + Clone>(
        &self,
        project: Option<&str>,
        global: &T,
        project_config: impl Fn(&super::sections::UserProjectOverrides) -> &T,
    ) -> T {
        self.project_overrides(project)
            .into_iter()
            .fold(global.clone(), |merged, overrides| {
                merged.merge_with(project_config(overrides))
            })
    }

    /// The last value `field` yields across the entries applying to `project`,
    /// so the most specific entry that sets it wins.
    fn project_field<'a, T>(
        &'a self,
        project: Option<&str>,
        field: impl Fn(&'a super::sections::UserProjectOverrides) -> Option<T>,
    ) -> Option<T> {
        self.project_overrides(project)
            .into_iter()
            .filter_map(field)
            .next_back()
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
        self.project_field(Some(project), |p| p.worktree_path.as_ref())
            .is_some()
    }

    /// Returns the worktree path template for a specific project.
    ///
    /// Checks project-specific config first, falls back to global worktree-path,
    /// and finally to the default template if neither is set.
    pub fn worktree_path_for_project(&self, project: &str) -> String {
        self.project_field(Some(project), |p| p.worktree_path.clone())
            .unwrap_or_else(|| self.worktree_path())
    }

    /// The forge platform set for a project under `[projects."…"].forge`.
    ///
    /// The user-level counterpart of the repository's own `[forge].platform`:
    /// a `[projects."git.company.example/*"]` entry names the forge for every
    /// repository on a host whose name carries no forge brand, without a
    /// `[forge]` block in each one. Read by
    /// [`Repository::ci_platform`](crate::git::Repository::ci_platform).
    pub fn forge_platform(&self, project: Option<&str>) -> Option<&str> {
        self.project_field(project, |p| p.forge.platform.as_deref())
    }

    /// The forge API hostname set for a project under `[projects."…"].forge`.
    ///
    /// Names the API server for a remote whose host is an SSH alias, or whose
    /// API lives on a different name. Both are facts about the host rather
    /// than the repository, which is why a pattern entry suits them.
    pub fn forge_hostname(&self, project: Option<&str>) -> Option<&str> {
        self.project_field(project, |p| p.forge.hostname.as_deref())
    }

    /// Returns the commit generation config for a specific project.
    ///
    /// Merges project-specific settings with global settings, where project
    /// settings take precedence for fields that are set. Deprecated
    /// `[commit-generation]` sections are normalized into `[commit.generation]`
    /// during config loading.
    pub fn commit_generation(&self, project: Option<&str>) -> CommitGenerationConfig {
        self.project_overrides(project)
            .into_iter()
            .filter_map(|config| config.commit.generation.as_ref())
            .fold(
                self.commit.generation.clone().unwrap_or_default(),
                |merged, proj| merged.merge_with(proj),
            )
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

    /// Returns the remove config for a specific project.
    ///
    /// Merges project-specific settings with global settings, where project
    /// settings take precedence for fields that are set.
    pub fn remove(&self, project: Option<&str>) -> RemoveConfig {
        self.merged_project_config(project, &self.remove, |config| &config.remove)
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
        self.project_overrides(project)
            .into_iter()
            .filter_map(|config| config.switch.picker.as_ref())
            .fold(
                self.switch.picker.clone().unwrap_or_default(),
                |merged, proj| merged.merge_with(proj),
            )
    }

    /// Returns effective hooks for a specific project.
    ///
    /// Merges global hooks with per-project hooks using append semantics.
    /// Both global and per-project hooks run (global first, then per-project).
    pub fn hooks(&self, project: Option<&str>) -> HooksConfig {
        self.project_overrides(project)
            .into_iter()
            .fold(self.hooks.clone(), |merged, config| {
                merged.merge_with(&config.hooks)
            })
    }

    /// Returns effective aliases for a specific project.
    ///
    /// Merges global user aliases with per-project user aliases using append
    /// semantics: both run on name collision (global first, then per-project).
    pub fn aliases(&self, project: Option<&str>) -> BTreeMap<String, CommandConfig> {
        let mut result = self.aliases.clone();
        for proj in self.project_overrides(project) {
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
    /// * `main_worktree` - Main worktree directory name; supplies both
    ///   `{{ main_worktree }}` and `{{ repo }}`
    /// * `branch` - Branch name (replaces {{ branch }} in template; use `{{ branch | sanitize }}` for paths)
    /// * `repo` - Repository, for template function access and for the
    ///   `{{ repo_path }}`, `{{ owner }}`, and {{ remote_repo }} values read off it —
    ///   `{{ owner }}` is absent when the primary remote has no parseable URL
    /// * `project` - Optional project identifier (e.g., "github.com/user/repo") to look up
    ///   project-specific worktree-path template
    ///
    /// The default template uses `{{ repo_path }}`, `{{ repo }}`, and
    /// `{{ branch | sanitize }}`; the full user-facing list is in
    /// [the config docs](https://worktrunk.dev/config/#worktree-path-template).
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
        let parsed_remote = repo.primary_remote_parsed_url();
        if let Some(ref parsed_remote) = parsed_remote {
            vars.insert("owner", parsed_remote.owner());
            vars.insert("remote_repo", parsed_remote.repo());
        }
        Ok(expand_template(
            &template,
            &vars,
            ShellEscapeMode::Literal,
            repo,
            "worktree-path",
        )
        .map(|p| shellexpand::tilde(&p).into_owned())?)
    }
}
