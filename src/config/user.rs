//! User-level configuration
//!
//! Personal preferences and per-project approved commands, not checked into git.

use std::path::PathBuf;
use std::sync::OnceLock;

use config::{Case, Config, ConfigError, File};
use fs2::FileExt;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::HooksConfig;

/// Trait for merging configuration structs.
///
/// Project-specific config fields override global fields when set.
/// Fields that are `None` in the override fall back to the base value.
pub trait Merge {
    /// Merge with another config, where `other` takes precedence for set fields.
    fn merge_with(&self, other: &Self) -> Self;
}

/// Merge optional global and project configs, returning the effective config.
///
/// - Both set: merge (project takes precedence for set fields)
/// - Only project set: clone project
/// - Only global set: clone global
/// - Neither set: None
fn merge_optional<T: Merge + Clone>(global: Option<&T>, project: Option<&T>) -> Option<T> {
    match (global, project) {
        (Some(g), Some(p)) => Some(g.merge_with(p)),
        (None, Some(p)) => Some(p.clone()),
        (Some(g), None) => Some(g.clone()),
        (None, None) => None,
    }
}

/// Acquire an exclusive lock on the config file for read-modify-write operations.
///
/// Uses a `.lock` file alongside the config file to coordinate between processes.
/// The lock is released when the returned guard is dropped.
fn acquire_config_lock(config_path: &std::path::Path) -> Result<std::fs::File, ConfigError> {
    let lock_path = config_path.with_extension("toml.lock");

    // Create parent directory if needed
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ConfigError::Message(format!("Failed to create config directory: {e}")))?;
    }

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| ConfigError::Message(format!("Failed to open lock file: {e}")))?;

    file.lock_exclusive()
        .map_err(|e| ConfigError::Message(format!("Failed to acquire config lock: {e}")))?;

    Ok(file)
}

#[cfg(not(test))]
use etcetera::base_strategy::{BaseStrategy, choose_base_strategy};

/// Override for user config path, set via --config CLI flag
static CONFIG_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Set the user config path override (called from CLI --config flag)
pub fn set_config_path(path: PathBuf) {
    CONFIG_PATH.set(path).ok();
}

use super::expansion::expand_template;

/// What to stage before committing
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    clap::ValueEnum,
    serde::Serialize,
    serde::Deserialize,
    JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum StageMode {
    /// Stage everything: untracked files + unstaged tracked changes
    #[default]
    All,
    /// Stage tracked changes only (like `git add -u`)
    Tracked,
    /// Stage nothing, commit only what's already in the index
    None,
}

/// User-level configuration for worktree path formatting and LLM integration.
///
/// This config is stored at `~/.config/worktrunk/config.toml` (or platform equivalent)
/// and is NOT checked into git. Each developer maintains their own user config.
///
/// The `worktree-path` template is relative to the repository root.
/// Supported variables:
/// - `{{ repo }}` - Repository directory name (e.g., `myproject`)
/// - `{{ branch }}` - Raw branch name (e.g., `feature/auth`)
/// - `{{ branch | sanitize }}` - Branch name with `/` and `\` replaced by `-`
///
/// # Examples
///
/// ```toml
/// # Default - parent directory siblings
/// worktree-path = "../{{ repo }}.{{ branch | sanitize }}"
///
/// # Inside repo (clean, no redundant directory)
/// worktree-path = ".worktrees/{{ branch | sanitize }}"
///
/// # Repository-namespaced (useful for shared directories with multiple repos)
/// worktree-path = "../worktrees/{{ repo }}/{{ branch | sanitize }}"
///
/// # Commit generation configuration
/// [commit.generation]
/// command = "llm -m claude-haiku-4.5"  # Shell command for generating commit messages
///
/// # Per-project configuration
/// [projects."github.com/user/repo"]
/// approved-commands = ["npm install", "npm test"]
/// ```
///
/// Config file location:
/// - Linux: `$XDG_CONFIG_HOME/worktrunk/config.toml` or `~/.config/worktrunk/config.toml`
/// - macOS: `$XDG_CONFIG_HOME/worktrunk/config.toml` or `~/.config/worktrunk/config.toml`
/// - Windows: `%APPDATA%\worktrunk\config.toml`
///
/// Environment variables can override config file settings using `WORKTRUNK_` prefix with
/// `__` separator for nested fields (e.g., `WORKTRUNK_COMMIT__GENERATION__COMMAND`).
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct UserConfig {
    /// **DEPRECATED**: Use `[commit.generation]` instead.
    ///
    /// This field is kept for backward compatibility. When both are set,
    /// `commit.generation` takes precedence.
    #[serde(
        default,
        rename = "commit-generation",
        skip_serializing_if = "Option::is_none"
    )]
    pub commit_generation: Option<CommitGenerationConfig>,

    /// Per-project configuration (approved commands, etc.)
    /// Uses BTreeMap for deterministic serialization order and better diff readability
    #[serde(default)]
    pub projects: std::collections::BTreeMap<String, UserProjectOverrides>,

    /// Settings that can be overridden per-project (worktree-path, list, commit, merge, select, hooks)
    #[serde(flatten, default)]
    pub configs: OverridableConfig,

    /// Skip the first-run shell integration prompt
    #[serde(
        default,
        rename = "skip-shell-integration-prompt",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub skip_shell_integration_prompt: bool,
}

/// Configuration for commit message generation
///
/// The command is a shell string executed via `sh -c`. Environment variables
/// can be set inline (e.g., `MAX_THINKING_TOKENS=0 claude -p ...`).
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq, JsonSchema)]
pub struct CommitGenerationConfig {
    /// Shell command to invoke for generating commit messages
    ///
    /// Examples:
    /// - `"llm -m claude-haiku-4.5"`
    /// - `"MAX_THINKING_TOKENS=0 claude -p --model haiku"`
    ///
    /// The command receives the prompt via stdin and should output the commit message.
    #[serde(default)]
    pub command: Option<String>,

    /// Inline template for commit message prompt
    /// Available variables: {{ git_diff }}, {{ branch }}, {{ recent_commits }}, {{ repo }}
    #[serde(default)]
    pub template: Option<String>,

    /// Path to template file (mutually exclusive with template)
    /// Supports tilde expansion (e.g., "~/.config/worktrunk/commit-template.txt")
    #[serde(default, rename = "template-file")]
    pub template_file: Option<String>,

    /// Inline template for squash commit message prompt
    /// Available variables: {{ commits }}, {{ target_branch }}, {{ branch }}, {{ repo }}
    #[serde(default, rename = "squash-template")]
    pub squash_template: Option<String>,

    /// Path to squash template file (mutually exclusive with squash-template)
    /// Supports tilde expansion (e.g., "~/.config/worktrunk/squash-template.txt")
    #[serde(default, rename = "squash-template-file")]
    pub squash_template_file: Option<String>,
}

impl CommitGenerationConfig {
    /// Returns true if an LLM command is configured
    pub fn is_configured(&self) -> bool {
        self.command
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
    }
}

impl Merge for CommitGenerationConfig {
    fn merge_with(&self, other: &Self) -> Self {
        // For template/template_file pairs: if project sets one, it clears the other
        // This prevents violating mutual exclusivity when global has one and project has the other
        let (template, template_file) = if other.template.is_some() {
            (other.template.clone(), None)
        } else if other.template_file.is_some() {
            (None, other.template_file.clone())
        } else {
            (self.template.clone(), self.template_file.clone())
        };

        let (squash_template, squash_template_file) = if other.squash_template.is_some() {
            (other.squash_template.clone(), None)
        } else if other.squash_template_file.is_some() {
            (None, other.squash_template_file.clone())
        } else {
            (
                self.squash_template.clone(),
                self.squash_template_file.clone(),
            )
        };

        Self {
            command: other.command.clone().or_else(|| self.command.clone()),
            template,
            template_file,
            squash_template,
            squash_template_file,
        }
    }
}

/// Per-project overrides in the user's config file
///
/// Stored under `[projects."project-id"]` in the user's config.
/// These are user preferences (not checked into git) that override
/// the corresponding global settings when set.
///
/// # TOML Format
/// ```toml
/// [projects."github.com/user/repo"]
/// worktree-path = ".worktrees/{{ branch | sanitize }}"
/// approved-commands = ["npm install", "npm test"]
///
/// [projects."github.com/user/repo".commit.generation]
/// command = "llm -m gpt-4"
///
/// [projects."github.com/user/repo".list]
/// full = true
///
/// [projects."github.com/user/repo".merge]
/// squash = false
/// ```
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default, JsonSchema)]
pub struct UserProjectOverrides {
    /// Commands that have been approved for automatic execution in this project
    #[serde(
        default,
        rename = "approved-commands",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub approved_commands: Vec<String>,

    /// **DEPRECATED**: Use `commit.generation` instead.
    ///
    /// Per-project commit generation settings (overrides global `[commit.generation]`)
    #[serde(
        default,
        rename = "commit-generation",
        skip_serializing_if = "Option::is_none"
    )]
    pub commit_generation: Option<CommitGenerationConfig>,

    /// Per-project overrides (worktree-path, list, commit, merge, select)
    #[serde(flatten, default)]
    pub overrides: OverridableConfig,
}

impl UserProjectOverrides {
    /// Returns true if all fields are empty/None (no settings configured).
    ///
    /// Used to determine if a project entry can be removed from config after
    /// clearing approvals.
    pub fn is_empty(&self) -> bool {
        self.approved_commands.is_empty()
            && self.commit_generation.is_none()
            && self.overrides.is_empty()
    }
}

/// Configuration for the `wt list` command
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default, JsonSchema)]
pub struct ListConfig {
    /// Show CI and `main` diffstat by default
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full: Option<bool>,

    /// Include branches without worktrees by default
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branches: Option<bool>,

    /// Include remote branches by default
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remotes: Option<bool>,

    /// (Experimental) Per-task timeout in milliseconds.
    /// When set to a positive value, git operations that exceed this timeout are terminated.
    /// Timed-out tasks show defaults in the table. Set to 0 to explicitly disable timeout
    /// (useful to override a global setting). Disabled when --full is used.
    #[serde(rename = "timeout-ms", skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

impl Merge for ListConfig {
    fn merge_with(&self, other: &Self) -> Self {
        Self {
            full: other.full.or(self.full),
            branches: other.branches.or(self.branches),
            remotes: other.remotes.or(self.remotes),
            timeout_ms: other.timeout_ms.or(self.timeout_ms),
        }
    }
}

/// Configuration for the `wt step commit` command
///
/// Also used by `wt merge` for shared settings like `stage`.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default, JsonSchema)]
pub struct CommitConfig {
    /// What to stage before committing (default: all)
    /// Values: "all", "tracked", "none"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<StageMode>,

    /// LLM commit message generation settings
    ///
    /// Nested under `[commit.generation]` in TOML.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<CommitGenerationConfig>,
}

impl Merge for CommitConfig {
    fn merge_with(&self, other: &Self) -> Self {
        Self {
            stage: other.stage.or(self.stage),
            generation: match (&self.generation, &other.generation) {
                (None, None) => None,
                (Some(s), None) => Some(s.clone()),
                (None, Some(o)) => Some(o.clone()),
                (Some(s), Some(o)) => Some(s.merge_with(o)),
            },
        }
    }
}

/// Configuration for the `wt merge` command
///
/// Note: `stage` defaults from `[commit]` section, not here.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default, JsonSchema)]
pub struct MergeConfig {
    /// Squash commits when merging (default: true)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub squash: Option<bool>,

    /// Commit, squash, and rebase during merge (default: true)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<bool>,

    /// Rebase onto target branch before merging (default: true)
    ///
    /// When false, merge fails if branch is not already rebased.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rebase: Option<bool>,

    /// Remove worktree after merge (default: true)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remove: Option<bool>,

    /// Run project hooks (default: true)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verify: Option<bool>,
}

impl Merge for MergeConfig {
    fn merge_with(&self, other: &Self) -> Self {
        Self {
            squash: other.squash.or(self.squash),
            commit: other.commit.or(self.commit),
            rebase: other.rebase.or(self.rebase),
            remove: other.remove.or(self.remove),
            verify: other.verify.or(self.verify),
        }
    }
}

/// Configuration for the `wt select` command
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default, JsonSchema)]
pub struct SelectConfig {
    /// Pager command with flags for diff preview
    ///
    /// Overrides git's core.pager for the select command's preview panel.
    /// Use this to specify pager flags needed for non-TTY contexts.
    ///
    /// Example: `pager = "delta --paging=never"`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pager: Option<String>,
}

impl Merge for SelectConfig {
    fn merge_with(&self, other: &Self) -> Self {
        Self {
            pager: other.pager.clone().or_else(|| self.pager.clone()),
        }
    }
}

/// Settings that can be set globally or per-project.
///
/// This struct is flattened into both `UserConfig` (global) and `UserProjectOverrides`
/// (per-project), ensuring new settings are automatically available in both
/// contexts without manual synchronization.
///
/// Note: Hooks use append semantics when merging global with per-project:
/// - Global hooks (top-level in TOML) are in `UserConfig.configs.hooks`
/// - Per-project hooks are in `UserProjectOverrides.overrides.hooks`
/// - The `UserConfig::hooks()` method merges both with global running first
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default, JsonSchema)]
pub struct OverridableConfig {
    /// Hooks configuration.
    ///
    /// At top level: global hooks that run for all projects.
    /// In `[projects."..."]`: per-project hooks that append to global hooks.
    #[serde(flatten, default)]
    pub hooks: HooksConfig,

    /// Worktree path template
    #[serde(
        rename = "worktree-path",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub worktree_path: Option<String>,

    /// Configuration for the `wt list` command
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list: Option<ListConfig>,

    /// Configuration for the `wt step commit` command (also used by merge)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<CommitConfig>,

    /// Configuration for the `wt merge` command
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge: Option<MergeConfig>,

    /// Configuration for the `wt select` command
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub select: Option<SelectConfig>,
}

impl OverridableConfig {
    /// Returns true if all settings are None/default.
    ///
    /// Includes hooks check for per-project configs where hooks are stored here.
    pub fn is_empty(&self) -> bool {
        self.hooks == HooksConfig::default()
            && self.worktree_path.is_none()
            && self.list.is_none()
            && self.commit.is_none()
            && self.merge.is_none()
            && self.select.is_none()
    }
}

impl Merge for OverridableConfig {
    fn merge_with(&self, other: &Self) -> Self {
        Self {
            hooks: self.hooks.merge_with(&other.hooks), // Append semantics
            worktree_path: other
                .worktree_path
                .clone()
                .or_else(|| self.worktree_path.clone()),
            list: merge_optional(self.list.as_ref(), other.list.as_ref()),
            commit: merge_optional(self.commit.as_ref(), other.commit.as_ref()),
            merge: merge_optional(self.merge.as_ref(), other.merge.as_ref()),
            select: merge_optional(self.select.as_ref(), other.select.as_ref()),
        }
    }
}

/// Default worktree path template
fn default_worktree_path() -> String {
    "../{{ repo }}.{{ branch | sanitize }}".to_string()
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

    /// Load configuration from config file and environment variables.
    ///
    /// Configuration is loaded in the following order (later sources override earlier ones):
    /// 1. Default values
    /// 2. Config file (see struct documentation for platform-specific paths)
    /// 3. Environment variables (WORKTRUNK_*)
    pub fn load() -> Result<Self, ConfigError> {
        // Note: worktree-path has no default set here - it's handled by the getter
        // which returns the default when None. This allows us to distinguish
        // "user explicitly set this" from "using default".
        let mut builder = Config::builder();

        // Add config file if it exists
        let config_path = get_config_path();
        if let Some(config_path) = config_path.as_ref()
            && config_path.exists()
        {
            // Check for deprecated template variables and create migration file if needed
            // User config always gets migration file (it's global, not worktree-specific)
            // Pass None for repo since user config is global and not tied to any repository
            // Use show_brief_warning=true to emit a brief pointer to `wt config show`
            if let Ok(content) = std::fs::read_to_string(config_path) {
                let _ = super::deprecation::check_and_migrate(
                    config_path,
                    &content,
                    true,
                    "User config",
                    None,
                    true, // show_brief_warning
                );

                // Warn about unknown fields in the config file
                // (must check file content directly, not config.unknown, because
                // config.unknown includes env vars which shouldn't trigger warnings)
                let unknown_keys: std::collections::HashMap<_, _> = find_unknown_keys(&content)
                    .into_iter()
                    .filter(|(k, _)| {
                        !super::deprecation::DEPRECATED_SECTION_KEYS.contains(&k.as_str())
                    })
                    .collect();
                super::deprecation::warn_unknown_fields::<UserConfig>(
                    config_path,
                    &unknown_keys,
                    "User config",
                );
            }

            builder = builder.add_source(File::from(config_path.clone()));
        }

        // Add environment variables with WORKTRUNK prefix
        // - prefix_separator("_"): strip prefix with single underscore (WORKTRUNK_ → key)
        // - separator("__"): double underscore for nested fields (COMMIT__GENERATION__COMMAND → commit.generation.command)
        // - convert_case(Kebab): converts snake_case to kebab-case to match serde field names
        // Example: WORKTRUNK_WORKTREE_PATH → worktree-path
        builder = builder.add_source(
            config::Environment::with_prefix("WORKTRUNK")
                .prefix_separator("_")
                .separator("__")
                .convert_case(Case::Kebab),
        );

        // The config crate's `preserve_order` feature ensures TOML insertion order
        // is preserved (uses IndexMap instead of HashMap internally).
        // See: https://github.com/max-sixty/worktrunk/issues/737
        let config: Self = builder.build()?.try_deserialize()?;
        config.validate()?;

        Ok(config)
    }

    /// Validate configuration values.
    fn validate(&self) -> Result<(), ConfigError> {
        // Validate worktree path (only if explicitly set - default is always valid)
        if let Some(ref path) = self.configs.worktree_path {
            if path.is_empty() {
                return Err(ConfigError::Message("worktree-path cannot be empty".into()));
            }
            if std::path::Path::new(path).is_absolute() {
                return Err(ConfigError::Message(
                    "worktree-path must be relative, not absolute".into(),
                ));
            }
        }

        // Validate per-project configs
        for (project, project_config) in &self.projects {
            // Validate worktree path
            if let Some(ref path) = project_config.overrides.worktree_path {
                if path.is_empty() {
                    return Err(ConfigError::Message(format!(
                        "projects.{project}.worktree-path cannot be empty"
                    )));
                }
                if std::path::Path::new(path).is_absolute() {
                    return Err(ConfigError::Message(format!(
                        "projects.{project}.worktree-path must be relative, not absolute"
                    )));
                }
            }

            // Validate commit generation config (check both old and new locations)
            // Old: [projects."...".commit-generation] (deprecated)
            if let Some(ref cg) = project_config.commit_generation {
                if cg.template.is_some() && cg.template_file.is_some() {
                    return Err(ConfigError::Message(format!(
                        "projects.{project}.commit-generation.template and template-file are mutually exclusive"
                    )));
                }
                if cg.squash_template.is_some() && cg.squash_template_file.is_some() {
                    return Err(ConfigError::Message(format!(
                        "projects.{project}.commit-generation.squash-template and squash-template-file are mutually exclusive"
                    )));
                }
            }
            // New: [projects."...".commit.generation]
            if let Some(ref commit) = project_config.overrides.commit
                && let Some(ref cg) = commit.generation
            {
                if cg.template.is_some() && cg.template_file.is_some() {
                    return Err(ConfigError::Message(format!(
                        "projects.{project}.commit.generation.template and template-file are mutually exclusive"
                    )));
                }
                if cg.squash_template.is_some() && cg.squash_template_file.is_some() {
                    return Err(ConfigError::Message(format!(
                        "projects.{project}.commit.generation.squash-template and squash-template-file are mutually exclusive"
                    )));
                }
            }
        }

        // Validate commit generation config (check both old and new locations)
        let commit_gen = self.commit_generation(None);
        if commit_gen.template.is_some() && commit_gen.template_file.is_some() {
            return Err(ConfigError::Message(
                "commit.generation.template and commit.generation.template-file are mutually exclusive".into(),
            ));
        }

        if commit_gen.squash_template.is_some() && commit_gen.squash_template_file.is_some() {
            return Err(ConfigError::Message(
                "commit.generation.squash-template and commit.generation.squash-template-file are mutually exclusive".into(),
            ));
        }

        Ok(())
    }

    /// Load configuration from a TOML string for testing.
    #[cfg(test)]
    fn load_from_str(content: &str) -> Result<Self, ConfigError> {
        let config: Self =
            toml::from_str(content).map_err(|e| ConfigError::Message(e.to_string()))?;
        config.validate()?;
        Ok(config)
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
        use std::collections::HashMap;
        let template = match project {
            Some(p) => self.worktree_path_for_project(p),
            None => self.worktree_path(),
        };
        let mut vars = HashMap::new();
        vars.insert("main_worktree", main_worktree);
        vars.insert("repo", main_worktree);
        vars.insert("branch", branch);
        expand_template(&template, &vars, false, repo, "worktree-path")
    }

    /// Execute a mutation under an exclusive file lock.
    ///
    /// Acquires lock, reloads from disk, calls the mutator, and saves if mutator returns true.
    fn with_locked_mutation<F>(
        &mut self,
        config_path: Option<&std::path::Path>,
        mutate: F,
    ) -> Result<(), ConfigError>
    where
        F: FnOnce(&mut Self) -> bool,
    {
        let path = match config_path {
            Some(p) => p.to_path_buf(),
            None => get_config_path().ok_or_else(|| {
                ConfigError::Message(
                    "Cannot determine config directory. Set $HOME or $XDG_CONFIG_HOME".to_string(),
                )
            })?,
        };
        let _lock = acquire_config_lock(&path)?;
        self.reload_projects_from(config_path)?;

        if mutate(self) {
            self.save_impl(config_path)?;
        }
        Ok(())
    }

    /// Check if a command is approved for the given project.
    ///
    /// Normalizes both the stored approvals and the incoming command to canonical
    /// variable names before comparing. This allows approvals to match regardless
    /// of whether they were saved with deprecated variable names (e.g., `repo_root`)
    /// or current names (e.g., `repo_path`).
    pub fn is_command_approved(&self, project: &str, command: &str) -> bool {
        let normalized_command = super::deprecation::normalize_template_vars(command);
        self.projects
            .get(project)
            .map(|p| {
                p.approved_commands
                    .iter()
                    .any(|c| super::deprecation::normalize_template_vars(c) == normalized_command)
            })
            .unwrap_or(false)
    }

    /// Add an approved command and save to config file.
    ///
    /// Acquires lock, reloads from disk, adds command if not present, and saves.
    /// Pass `None` for default config path, or `Some(path)` for testing.
    pub fn approve_command(
        &mut self,
        project: String,
        command: String,
        config_path: Option<&std::path::Path>,
    ) -> Result<(), ConfigError> {
        self.with_locked_mutation(config_path, |config| {
            if config.is_command_approved(&project, &command) {
                return false;
            }
            config
                .projects
                .entry(project)
                .or_default()
                .approved_commands
                .push(command);
            true
        })
    }

    /// Reload only the projects section from disk, preserving other in-memory state
    ///
    /// This replaces the in-memory projects with the authoritative disk state,
    /// while keeping other config values (worktree-path, commit-generation, etc.).
    /// Callers should reload before modifying and saving to avoid race conditions.
    fn reload_projects_from(
        &mut self,
        config_path: Option<&std::path::Path>,
    ) -> Result<(), ConfigError> {
        let path = match config_path {
            Some(p) => Some(p.to_path_buf()),
            None => get_config_path(),
        };

        let Some(path) = path else {
            return Ok(()); // No config file to reload from
        };

        if !path.exists() {
            return Ok(()); // Nothing to reload
        }

        let content = std::fs::read_to_string(&path).map_err(|e| {
            ConfigError::Message(format!(
                "Failed to read config file {}: {}",
                path.display(),
                e
            ))
        })?;

        let disk_config: UserConfig = toml::from_str(&content).map_err(|e| {
            ConfigError::Message(format!(
                "Failed to parse config file {}: {}",
                path.display(),
                e
            ))
        })?;

        // Replace in-memory projects with disk state (disk is authoritative)
        self.projects = disk_config.projects;

        Ok(())
    }

    /// Revoke an approved command and save to config file.
    ///
    /// Acquires lock, reloads from disk, removes command if present, and saves.
    /// Pass `None` for default config path, or `Some(path)` for testing.
    pub fn revoke_command(
        &mut self,
        project: &str,
        command: &str,
        config_path: Option<&std::path::Path>,
    ) -> Result<(), ConfigError> {
        let project = project.to_string();
        let command = command.to_string();
        self.with_locked_mutation(config_path, |config| {
            let Some(project_config) = config.projects.get_mut(&project) else {
                return false;
            };
            let len_before = project_config.approved_commands.len();
            project_config.approved_commands.retain(|c| c != &command);
            let changed = len_before != project_config.approved_commands.len();

            // Only remove project entry if it has no other settings
            if project_config.is_empty() {
                config.projects.remove(&project);
            }
            changed
        })
    }

    /// Remove all approvals for a project and save to config file.
    ///
    /// Clears only the approved-commands list, preserving other per-project settings
    /// like worktree-path, commit-generation, list, commit, and merge configs.
    /// The project entry is removed only if all settings are empty after clearing.
    ///
    /// Acquires lock, reloads from disk, clears approvals, and saves.
    /// Pass `None` for default config path, or `Some(path)` for testing.
    pub fn revoke_project(
        &mut self,
        project: &str,
        config_path: Option<&std::path::Path>,
    ) -> Result<(), ConfigError> {
        let project = project.to_string();
        self.with_locked_mutation(config_path, |config| {
            let Some(project_config) = config.projects.get_mut(&project) else {
                return false;
            };
            if project_config.approved_commands.is_empty() {
                return false; // Nothing to clear
            }
            project_config.approved_commands.clear();
            // Only remove project entry if it has no other settings
            if project_config.is_empty() {
                config.projects.remove(&project);
            }
            true
        })
    }

    /// Set `skip-shell-integration-prompt = true` and save.
    ///
    /// Acquires lock, reloads from disk, sets flag if not already set, and saves.
    /// Pass `None` for default config path, or `Some(path)` for testing.
    pub fn set_skip_shell_integration_prompt(
        &mut self,
        config_path: Option<&std::path::Path>,
    ) -> Result<(), ConfigError> {
        self.with_locked_mutation(config_path, |config| {
            if config.skip_shell_integration_prompt {
                return false;
            }
            config.skip_shell_integration_prompt = true;
            true
        })
    }

    /// Save the current configuration to the default config file location
    pub fn save(&self) -> Result<(), ConfigError> {
        self.save_impl(None)
    }

    /// Internal save implementation that handles both default and custom paths
    fn save_impl(&self, config_path: Option<&std::path::Path>) -> Result<(), ConfigError> {
        match config_path {
            Some(path) => self.save_to(path),
            None => {
                let path = get_config_path().ok_or_else(|| {
                    ConfigError::Message(
                        "Cannot determine config directory. Set $HOME or $XDG_CONFIG_HOME environment variable".to_string(),
                    )
                })?;
                self.save_to(&path)
            }
        }
    }

    /// Format a string array as multiline TOML for readability
    ///
    /// TODO: toml_edit doesn't provide a built-in multiline array format option.
    /// Consider replacing with a dependency if one emerges that handles this automatically.
    fn format_multiline_array<'a>(items: impl Iterator<Item = &'a String>) -> toml_edit::Array {
        let mut array: toml_edit::Array = items.collect();
        for item in array.iter_mut() {
            item.decor_mut().set_prefix("\n    ");
        }
        array.set_trailing("\n");
        array.set_trailing_comma(true);
        array
    }

    /// Serialize a per-project config section (commit-generation, list, commit, merge).
    ///
    /// If the config is Some, serializes it as a nested table. If None, removes the section.
    /// Used when updating an existing file.
    fn serialize_project_config_section<T: Serialize>(
        projects: &mut toml_edit::Table,
        project_id: &str,
        section_name: &str,
        config: Option<&T>,
    ) {
        if let Some(cfg) = config {
            // Serialize to TOML value, then convert to toml_edit Item
            if let Ok(toml_value) = toml::to_string(cfg)
                && let Ok(parsed) = toml_value.parse::<toml_edit::DocumentMut>()
            {
                let mut table = toml_edit::Table::new();
                for (k, v) in parsed.iter() {
                    table[k] = v.clone();
                }
                projects[project_id][section_name] = toml_edit::Item::Table(table);
            }
        } else if let Some(project_table) = projects[project_id].as_table_mut() {
            project_table.remove(section_name);
        }
    }

    /// Recursively convert inline tables to standard tables for readability.
    ///
    /// When using `toml_edit::ser::to_document()`, nested structs are serialized as inline tables
    /// (e.g., `commit = { generation = { command = "..." } }`). This converts them to standard
    /// multi-line tables for better human readability.
    fn expand_inline_tables(table: &mut toml_edit::Table) {
        let keys: Vec<_> = table.iter().map(|(k, _)| k.to_string()).collect();
        for key in keys {
            let item = table.get_mut(&key).unwrap();
            if let Some(inline) = item.as_inline_table() {
                let mut new_table = inline.clone().into_table();
                Self::expand_inline_tables(&mut new_table);
                *item = toml_edit::Item::Table(new_table);
            } else if let Some(t) = item.as_table_mut() {
                Self::expand_inline_tables(t);
            }
        }
    }

    /// Save the current configuration to a specific file path
    ///
    /// Use this in tests to save to a temporary location instead of the user's config.
    /// Preserves comments and formatting in the existing file when possible.
    pub fn save_to(&self, config_path: &std::path::Path) -> Result<(), ConfigError> {
        // Create parent directory if it doesn't exist
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ConfigError::Message(format!("Failed to create config directory: {}", e))
            })?;
        }

        // If file exists, use toml_edit to preserve comments and formatting
        let toml_string = if config_path.exists() {
            let existing_content = std::fs::read_to_string(config_path)
                .map_err(|e| ConfigError::Message(format!("Failed to read config file: {}", e)))?;

            let mut doc: toml_edit::DocumentMut = existing_content
                .parse()
                .map_err(|e| ConfigError::Message(format!("Failed to parse config file: {}", e)))?;

            // Update skip-shell-integration-prompt flag
            if self.skip_shell_integration_prompt {
                doc["skip-shell-integration-prompt"] = toml_edit::value(true);
            } else {
                doc.remove("skip-shell-integration-prompt");
            }

            // Update the projects section
            // Ensure projects table exists
            if !doc.contains_key("projects") {
                doc["projects"] = toml_edit::Item::Table(toml_edit::Table::new());
            }

            if let Some(projects) = doc["projects"].as_table_mut() {
                // Remove stale projects
                let stale: Vec<_> = projects
                    .iter()
                    .filter(|(k, _)| !self.projects.contains_key(*k))
                    .map(|(k, _)| k.to_string())
                    .collect();
                for key in stale {
                    projects.remove(&key);
                }

                // Add/update projects
                for (project_id, project_config) in &self.projects {
                    if !projects.contains_key(project_id) {
                        projects[project_id] = toml_edit::Item::Table(toml_edit::Table::new());
                    }

                    // worktree-path (only if set)
                    if let Some(ref path) = project_config.overrides.worktree_path {
                        projects[project_id]["worktree-path"] = toml_edit::value(path);
                    } else if let Some(table) = projects[project_id].as_table_mut() {
                        table.remove("worktree-path");
                    }

                    // approved-commands
                    let commands =
                        Self::format_multiline_array(project_config.approved_commands.iter());
                    projects[project_id]["approved-commands"] = toml_edit::value(commands);

                    // Per-project nested config sections
                    Self::serialize_project_config_section(
                        projects,
                        project_id,
                        "commit-generation",
                        project_config.commit_generation.as_ref(),
                    );
                    Self::serialize_project_config_section(
                        projects,
                        project_id,
                        "list",
                        project_config.overrides.list.as_ref(),
                    );
                    Self::serialize_project_config_section(
                        projects,
                        project_id,
                        "commit",
                        project_config.overrides.commit.as_ref(),
                    );
                    Self::serialize_project_config_section(
                        projects,
                        project_id,
                        "merge",
                        project_config.overrides.merge.as_ref(),
                    );
                    Self::serialize_project_config_section(
                        projects,
                        project_id,
                        "select",
                        project_config.overrides.select.as_ref(),
                    );
                }
            }

            doc.to_string()
        } else {
            // No existing file: serialize struct directly, then post-process formatting
            let mut doc = toml_edit::ser::to_document(&self)
                .map_err(|e| ConfigError::Message(format!("Serialization error: {e}")))?;

            // Convert inline tables to standard tables for readability
            Self::expand_inline_tables(doc.as_table_mut());

            // Post-process: format approved-commands as multiline arrays for readability
            if let Some(projects) = doc.get_mut("projects").and_then(|p| p.as_table_mut()) {
                projects.set_implicit(true); // Don't emit [projects] header
                for (_, project) in projects.iter_mut() {
                    if let Some(arr) = project
                        .get_mut("approved-commands")
                        .and_then(|a| a.as_array_mut())
                    {
                        for item in arr.iter_mut() {
                            item.decor_mut().set_prefix("\n    ");
                        }
                        arr.set_trailing("\n");
                        arr.set_trailing_comma(true);
                    }
                }
            }

            doc.to_string()
        };

        std::fs::write(config_path, toml_string)
            .map_err(|e| ConfigError::Message(format!("Failed to write config file: {}", e)))?;

        Ok(())
    }
}

pub fn get_config_path() -> Option<PathBuf> {
    // Priority 1: CLI --config flag
    if let Some(path) = CONFIG_PATH.get() {
        return Some(path.clone());
    }

    // Priority 2: Environment variable (also used by tests)
    if let Ok(path) = std::env::var("WORKTRUNK_CONFIG_PATH") {
        return Some(PathBuf::from(path));
    }

    // In test builds, WORKTRUNK_CONFIG_PATH must be set to prevent polluting user config
    #[cfg(test)]
    panic!(
        "WORKTRUNK_CONFIG_PATH not set in test. Tests must use TestRepo which sets this automatically, \
        or set it manually to an isolated test config path."
    );

    // Production: use standard config location
    // choose_base_strategy uses:
    // - XDG on Linux (respects XDG_CONFIG_HOME, falls back to ~/.config)
    // - XDG on macOS (~/.config instead of ~/Library/Application Support)
    // - Windows conventions on Windows (%APPDATA%)
    #[cfg(not(test))]
    {
        let strategy = choose_base_strategy().ok()?;
        Some(strategy.config_dir().join("worktrunk").join("config.toml"))
    }
}

/// Returns all valid top-level keys in user config, derived from the JsonSchema.
///
/// This includes keys from UserConfig, OverridableConfig (flattened), and HooksConfig (flattened).
/// Public for use by the `WorktrunkConfig` trait implementation.
pub fn valid_user_config_keys() -> Vec<String> {
    use schemars::SchemaGenerator;

    let schema = SchemaGenerator::default().into_root_schema_for::<UserConfig>();

    // Extract property names from the schema
    // The schema flattens nested structs, so all top-level keys appear in properties
    schema
        .as_object()
        .and_then(|obj| obj.get("properties"))
        .and_then(|p| p.as_object())
        .map(|props| props.keys().cloned().collect())
        .unwrap_or_default()
}

/// Find unknown keys in user config TOML content.
///
/// Returns a map of unrecognized top-level keys (with their values) that will be ignored.
/// Compares against the known valid keys derived from the JsonSchema rather than using
/// serde flatten catchall (which doesn't work reliably with nested flattens).
/// The values are included to allow checking if keys belong in the other config type.
pub fn find_unknown_keys(contents: &str) -> std::collections::HashMap<String, toml::Value> {
    let Ok(table) = contents.parse::<toml::Table>() else {
        return std::collections::HashMap::new();
    };

    let valid_keys = valid_user_config_keys();

    table
        .into_iter()
        .filter(|(key, _)| !valid_keys.contains(key))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::Repository;

    /// Test fixture that creates a real temporary git repository.
    struct TestRepo {
        _dir: tempfile::TempDir,
        repo: Repository,
    }

    impl TestRepo {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            std::process::Command::new("git")
                .args(["init"])
                .current_dir(dir.path())
                .output()
                .unwrap();
            let repo = Repository::at(dir.path()).unwrap();
            Self { _dir: dir, repo }
        }
    }

    fn test_repo() -> TestRepo {
        TestRepo::new()
    }

    #[test]
    fn test_find_unknown_keys_empty() {
        // Valid config with no unknown keys
        let content = r#"
worktree-path = "../{{ main_worktree }}.{{ branch }}"
"#;
        let keys = find_unknown_keys(content);
        assert!(
            keys.is_empty(),
            "Expected no unknown keys, found: {:?}",
            keys
        );
    }

    #[test]
    fn test_find_unknown_keys_with_unknown() {
        // Config with unknown top-level keys
        let content = r#"
worktree-path = "../{{ main_worktree }}.{{ branch }}"
unknown-key = "value"
another-unknown = 42
"#;
        let keys = find_unknown_keys(content);
        assert!(keys.contains_key("unknown-key"));
        assert!(keys.contains_key("another-unknown"));
    }

    #[test]
    fn test_find_unknown_keys_known_sections() {
        // All known sections should not be reported
        let content = r#"
worktree-path = "../{{ main_worktree }}.{{ branch }}"

[commit-generation]
command = "llm"

[list]
full = true

[commit]
stage = "all"

[merge]
squash = true

[post-create]
run = "npm install"

[post-start]
run = "npm run build"

[post-switch]
rename-tab = "echo 'switched'"
"#;
        let keys = find_unknown_keys(content);
        assert!(keys.is_empty());
    }

    #[test]
    fn test_commit_generation_config_is_configured_empty() {
        let config = CommitGenerationConfig::default();
        assert!(!config.is_configured());
    }

    #[test]
    fn test_commit_generation_config_is_configured_with_command() {
        let config = CommitGenerationConfig {
            command: Some("llm".to_string()),
            ..Default::default()
        };
        assert!(config.is_configured());
    }

    #[test]
    fn test_commit_generation_config_is_configured_with_whitespace_only() {
        let config = CommitGenerationConfig {
            command: Some("   ".to_string()),
            ..Default::default()
        };
        assert!(!config.is_configured());
    }

    #[test]
    fn test_commit_generation_config_is_configured_with_empty_string() {
        let config = CommitGenerationConfig {
            command: Some("".to_string()),
            ..Default::default()
        };
        assert!(!config.is_configured());
    }

    #[test]
    fn test_stage_mode_default() {
        assert_eq!(StageMode::default(), StageMode::All);
    }

    #[test]
    fn test_stage_mode_serde() {
        // Test serialization
        let all_json = serde_json::to_string(&StageMode::All).unwrap();
        assert_eq!(all_json, "\"all\"");

        let tracked_json = serde_json::to_string(&StageMode::Tracked).unwrap();
        assert_eq!(tracked_json, "\"tracked\"");

        let none_json = serde_json::to_string(&StageMode::None).unwrap();
        assert_eq!(none_json, "\"none\"");

        // Test deserialization
        let all: StageMode = serde_json::from_str("\"all\"").unwrap();
        assert_eq!(all, StageMode::All);

        let tracked: StageMode = serde_json::from_str("\"tracked\"").unwrap();
        assert_eq!(tracked, StageMode::Tracked);

        let none: StageMode = serde_json::from_str("\"none\"").unwrap();
        assert_eq!(none, StageMode::None);
    }

    #[test]
    fn test_user_project_config_default() {
        let config = UserProjectOverrides::default();
        assert!(config.overrides.worktree_path.is_none());
        assert!(config.approved_commands.is_empty());
    }

    #[test]
    fn test_user_project_config_with_worktree_path_serde() {
        let config = UserProjectOverrides {
            overrides: OverridableConfig {
                worktree_path: Some(".worktrees/{{ branch | sanitize }}".to_string()),
                ..Default::default()
            },
            approved_commands: vec!["npm install".to_string()],
            ..Default::default()
        };
        let toml = toml::to_string(&config).unwrap();
        assert!(toml.contains("worktree-path"));
        assert!(toml.contains(".worktrees/{{ branch | sanitize }}"));

        let parsed: UserProjectOverrides = toml::from_str(&toml).unwrap();
        assert_eq!(
            parsed.overrides.worktree_path,
            Some(".worktrees/{{ branch | sanitize }}".to_string())
        );
        assert_eq!(parsed.approved_commands, vec!["npm install".to_string()]);
    }

    #[test]
    fn test_worktree_path_for_project_uses_project_specific() {
        let mut config = UserConfig::default();
        config.projects.insert(
            "github.com/user/repo".to_string(),
            UserProjectOverrides {
                overrides: OverridableConfig {
                    worktree_path: Some(".worktrees/{{ branch | sanitize }}".to_string()),
                    ..Default::default()
                },
                approved_commands: vec![],
                ..Default::default()
            },
        );

        // Project-specific path should be used
        assert_eq!(
            config.worktree_path_for_project("github.com/user/repo"),
            ".worktrees/{{ branch | sanitize }}"
        );
    }

    #[test]
    fn test_worktree_path_for_project_falls_back_to_global() {
        let mut config = UserConfig {
            configs: OverridableConfig {
                worktree_path: Some("../{{ repo }}-{{ branch | sanitize }}".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        config.projects.insert(
            "github.com/user/repo".to_string(),
            UserProjectOverrides {
                overrides: OverridableConfig {
                    worktree_path: None, // No project-specific path
                    ..Default::default()
                },
                approved_commands: vec!["npm install".to_string()],
                ..Default::default()
            },
        );

        // Should fall back to global worktree-path
        assert_eq!(
            config.worktree_path_for_project("github.com/user/repo"),
            "../{{ repo }}-{{ branch | sanitize }}"
        );
    }

    #[test]
    fn test_worktree_path_for_project_falls_back_to_default() {
        let config = UserConfig::default();

        // Unknown project should fall back to default template
        assert_eq!(
            config.worktree_path_for_project("github.com/unknown/project"),
            "../{{ repo }}.{{ branch | sanitize }}"
        );
    }

    #[test]
    fn test_format_path_with_project_override() {
        let test = test_repo();
        let mut config = UserConfig {
            configs: OverridableConfig {
                worktree_path: Some("../{{ repo }}.{{ branch | sanitize }}".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        config.projects.insert(
            "github.com/user/repo".to_string(),
            UserProjectOverrides {
                overrides: OverridableConfig {
                    worktree_path: Some(".worktrees/{{ branch | sanitize }}".to_string()),
                    ..Default::default()
                },
                approved_commands: vec![],
                ..Default::default()
            },
        );

        // With project identifier, should use project-specific template
        let path = config
            .format_path(
                "myrepo",
                "feature/branch",
                &test.repo,
                Some("github.com/user/repo"),
            )
            .unwrap();
        assert_eq!(path, ".worktrees/feature-branch");

        // Without project identifier, should use global template
        let path = config
            .format_path("myrepo", "feature/branch", &test.repo, None)
            .unwrap();
        assert_eq!(path, "../myrepo.feature-branch");
    }

    #[test]
    fn test_list_config_serde() {
        let config = ListConfig {
            full: Some(true),
            branches: Some(false),
            remotes: None,
            timeout_ms: Some(500),
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: ListConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.full, Some(true));
        assert_eq!(parsed.branches, Some(false));
        assert_eq!(parsed.remotes, None);
        assert_eq!(parsed.timeout_ms, Some(500));
    }

    #[test]
    fn test_commit_config_default() {
        let config = CommitConfig::default();
        assert!(config.stage.is_none());
    }

    #[test]
    fn test_worktrunk_config_default() {
        let config = UserConfig::default();
        // worktree_path is None by default, but the getter returns the default
        assert!(config.configs.worktree_path.is_none());
        assert_eq!(
            config.worktree_path(),
            "../{{ repo }}.{{ branch | sanitize }}"
        );
        assert!(config.projects.is_empty());
        assert!(config.configs.list.is_none());
        assert!(config.configs.commit.is_none());
        assert!(config.configs.merge.is_none());
        assert!(config.commit_generation.is_none());
        assert!(!config.skip_shell_integration_prompt);
    }

    #[test]
    fn test_worktrunk_config_is_command_approved_empty() {
        let config = UserConfig::default();
        assert!(!config.is_command_approved("some/project", "npm install"));
    }

    #[test]
    fn test_worktrunk_config_is_command_approved_with_commands() {
        let mut config = UserConfig::default();
        config.projects.insert(
            "github.com/user/repo".to_string(),
            UserProjectOverrides {
                approved_commands: vec!["npm install".to_string(), "npm test".to_string()],
                ..Default::default()
            },
        );
        assert!(config.is_command_approved("github.com/user/repo", "npm install"));
        assert!(config.is_command_approved("github.com/user/repo", "npm test"));
        assert!(!config.is_command_approved("github.com/user/repo", "rm -rf /"));
        assert!(!config.is_command_approved("other/project", "npm install"));
    }

    #[test]
    fn test_is_command_approved_normalizes_deprecated_vars() {
        // Approval saved with deprecated variable should match command with new variable
        let mut config = UserConfig::default();
        config.projects.insert(
            "github.com/user/repo".to_string(),
            UserProjectOverrides {
                approved_commands: vec![
                    "ln -sf {{ repo_root }}/node_modules".to_string(), // old var
                ],
                ..Default::default()
            },
        );

        // Should match when checking with new variable name
        assert!(config.is_command_approved(
            "github.com/user/repo",
            "ln -sf {{ repo_path }}/node_modules" // new var
        ));

        // Should still match exact old name too
        assert!(config.is_command_approved(
            "github.com/user/repo",
            "ln -sf {{ repo_root }}/node_modules" // old var
        ));
    }

    #[test]
    fn test_is_command_approved_normalizes_new_approval_matches_old_command() {
        // Approval saved with new variable should match command with deprecated variable
        let mut config = UserConfig::default();
        config.projects.insert(
            "github.com/user/repo".to_string(),
            UserProjectOverrides {
                approved_commands: vec![
                    "cd {{ worktree_path }} && npm install".to_string(), // new var
                ],
                ..Default::default()
            },
        );

        // Should match when checking with old variable name
        assert!(config.is_command_approved(
            "github.com/user/repo",
            "cd {{ worktree }} && npm install" // old var
        ));
    }

    #[test]
    fn test_is_command_approved_normalizes_multiple_vars() {
        let mut config = UserConfig::default();
        config.projects.insert(
            "github.com/user/repo".to_string(),
            UserProjectOverrides {
                approved_commands: vec![
                    "ln -sf {{ repo_root }}/modules {{ worktree }}/modules".to_string(),
                ],
                ..Default::default()
            },
        );

        // Should match with all new variable names
        assert!(config.is_command_approved(
            "github.com/user/repo",
            "ln -sf {{ repo_path }}/modules {{ worktree_path }}/modules"
        ));

        // Should match with mixed old/new (both normalize to same canonical form)
        assert!(config.is_command_approved(
            "github.com/user/repo",
            "ln -sf {{ repo_path }}/modules {{ worktree }}/modules"
        ));
    }

    #[test]
    fn test_worktrunk_config_format_path() {
        let test = test_repo();
        let config = UserConfig::default();
        let path = config
            .format_path("myrepo", "feature/branch", &test.repo, None)
            .unwrap();
        assert_eq!(path, "../myrepo.feature-branch");
    }

    #[test]
    fn test_worktrunk_config_format_path_custom_template() {
        let test = test_repo();
        let config = UserConfig {
            configs: OverridableConfig {
                worktree_path: Some(".worktrees/{{ branch }}".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let path = config
            .format_path("myrepo", "feature", &test.repo, None)
            .unwrap();
        assert_eq!(path, ".worktrees/feature");
    }

    #[test]
    fn test_merge_config_serde() {
        let config = MergeConfig {
            squash: Some(true),
            commit: Some(true),
            rebase: Some(false),
            remove: Some(true),
            verify: Some(true),
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: MergeConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.squash, Some(true));
        assert_eq!(parsed.rebase, Some(false));
    }

    #[test]
    fn test_skip_shell_integration_prompt_default_false() {
        let config = UserConfig::default();
        assert!(!config.skip_shell_integration_prompt);
    }

    #[test]
    fn test_skip_shell_integration_prompt_serde_roundtrip() {
        // Test serialization when true
        let config = UserConfig {
            skip_shell_integration_prompt: true,
            ..UserConfig::default()
        };
        let toml = toml::to_string(&config).unwrap();
        assert!(toml.contains("skip-shell-integration-prompt = true"));

        // Test deserialization
        let parsed: UserConfig = toml::from_str(&toml).unwrap();
        assert!(parsed.skip_shell_integration_prompt);
    }

    #[test]
    fn test_skip_shell_integration_prompt_skipped_when_false() {
        // When false, the field should not appear in serialized output
        let config = UserConfig::default();
        let toml = toml::to_string(&config).unwrap();
        assert!(!toml.contains("skip-shell-integration-prompt"));
    }

    #[test]
    fn test_skip_shell_integration_prompt_parsed_from_toml() {
        let content = r#"
worktree-path = "../{{ main_worktree }}.{{ branch }}"
skip-shell-integration-prompt = true
"#;
        let config: UserConfig = toml::from_str(content).unwrap();
        assert!(config.skip_shell_integration_prompt);
    }

    #[test]
    fn test_skip_shell_integration_prompt_defaults_when_missing() {
        let content = r#"
worktree-path = "../{{ main_worktree }}.{{ branch }}"
"#;
        let config: UserConfig = toml::from_str(content).unwrap();
        assert!(!config.skip_shell_integration_prompt);
    }

    // =========================================================================
    // Merge trait tests
    // =========================================================================

    #[test]
    fn test_merge_list_config() {
        let base = ListConfig {
            full: Some(true),
            branches: Some(false),
            remotes: None,
            timeout_ms: Some(1000),
        };
        let override_config = ListConfig {
            full: None,           // Should fall back to base
            branches: Some(true), // Should override
            remotes: Some(true),  // Should override (base was None)
            timeout_ms: None,     // Should fall back to base
        };

        let merged = base.merge_with(&override_config);
        assert_eq!(merged.full, Some(true)); // From base
        assert_eq!(merged.branches, Some(true)); // From override
        assert_eq!(merged.remotes, Some(true)); // From override
        assert_eq!(merged.timeout_ms, Some(1000)); // From base
    }

    #[test]
    fn test_merge_commit_config() {
        let base = CommitConfig {
            stage: Some(StageMode::All),
            generation: None,
        };
        let override_config = CommitConfig {
            stage: Some(StageMode::Tracked),
            generation: None,
        };

        let merged = base.merge_with(&override_config);
        assert_eq!(merged.stage, Some(StageMode::Tracked));
    }

    #[test]
    fn test_merge_commit_config_generation_base_only() {
        // Base has generation, override doesn't - use base
        let base = CommitConfig {
            stage: None,
            generation: Some(CommitGenerationConfig {
                command: Some("base-llm".to_string()),
                ..Default::default()
            }),
        };
        let override_config = CommitConfig {
            stage: None,
            generation: None,
        };

        let merged = base.merge_with(&override_config);
        assert_eq!(
            merged.generation.as_ref().unwrap().command,
            Some("base-llm".to_string())
        );
    }

    #[test]
    fn test_merge_commit_config_generation_override_only() {
        // Override has generation, base doesn't - use override
        let base = CommitConfig {
            stage: None,
            generation: None,
        };
        let override_config = CommitConfig {
            stage: None,
            generation: Some(CommitGenerationConfig {
                command: Some("override-llm".to_string()),
                ..Default::default()
            }),
        };

        let merged = base.merge_with(&override_config);
        assert_eq!(
            merged.generation.as_ref().unwrap().command,
            Some("override-llm".to_string())
        );
    }

    #[test]
    fn test_merge_commit_config_generation_both() {
        // Both have generation - merge them
        let base = CommitConfig {
            stage: Some(StageMode::All),
            generation: Some(CommitGenerationConfig {
                command: Some("base-llm".to_string()),
                template: Some("base-template".to_string()),
                ..Default::default()
            }),
        };
        let override_config = CommitConfig {
            stage: None, // Will use base's stage
            generation: Some(CommitGenerationConfig {
                command: Some("override-llm".to_string()), // Override command
                template: None,                            // Use base's template
                ..Default::default()
            }),
        };

        let merged = base.merge_with(&override_config);
        assert_eq!(merged.stage, Some(StageMode::All));
        let generation = merged.generation.as_ref().unwrap();
        assert_eq!(generation.command, Some("override-llm".to_string()));
        assert_eq!(generation.template, Some("base-template".to_string()));
    }

    #[test]
    fn test_merge_merge_config() {
        let base = MergeConfig {
            squash: Some(true),
            commit: Some(true),
            rebase: Some(true),
            remove: Some(true),
            verify: Some(true),
        };
        let override_config = MergeConfig {
            squash: Some(false), // Override
            commit: None,        // Fall back to base
            rebase: None,        // Fall back to base
            remove: Some(false), // Override
            verify: None,        // Fall back to base
        };

        let merged = base.merge_with(&override_config);
        assert_eq!(merged.squash, Some(false));
        assert_eq!(merged.commit, Some(true));
        assert_eq!(merged.rebase, Some(true));
        assert_eq!(merged.remove, Some(false));
        assert_eq!(merged.verify, Some(true));
    }

    #[test]
    fn test_merge_commit_generation_config() {
        let base = CommitGenerationConfig {
            command: Some("llm -m claude-haiku-4.5".to_string()),
            template: None,
            template_file: Some("~/.config/template.txt".to_string()),
            squash_template: None,
            squash_template_file: None,
        };
        let override_config = CommitGenerationConfig {
            command: Some("claude -p --model haiku".to_string()), // Override
            template: Some("custom".to_string()),                 // Override (was None)
            template_file: None,                                  // Fall back to base
            squash_template: None,
            squash_template_file: None,
        };

        let merged = base.merge_with(&override_config);
        assert_eq!(merged.command, Some("claude -p --model haiku".to_string()));
        assert_eq!(merged.template, Some("custom".to_string()));
        // When project sets template, template_file is cleared to maintain mutual exclusivity
        assert_eq!(merged.template_file, None);
    }

    #[test]
    fn test_commit_generation_merge_mutual_exclusivity() {
        // Global has template_file, project has template
        // Merged result should only have template (project wins, clears template_file)
        let global = CommitGenerationConfig {
            template_file: Some("~/.config/template.txt".to_string()),
            ..Default::default()
        };
        let project = CommitGenerationConfig {
            template: Some("inline template".to_string()),
            ..Default::default()
        };

        let merged = global.merge_with(&project);
        assert_eq!(merged.template, Some("inline template".to_string()));
        assert_eq!(merged.template_file, None); // Cleared because project set template

        // Reverse: global has template, project has template_file
        let global = CommitGenerationConfig {
            template: Some("global template".to_string()),
            ..Default::default()
        };
        let project = CommitGenerationConfig {
            template_file: Some("project-file.txt".to_string()),
            ..Default::default()
        };

        let merged = global.merge_with(&project);
        assert_eq!(merged.template, None); // Cleared because project set template_file
        assert_eq!(merged.template_file, Some("project-file.txt".to_string()));

        // Neither set in project: inherit both from global
        let global = CommitGenerationConfig {
            template: Some("global template".to_string()),
            ..Default::default()
        };
        let project = CommitGenerationConfig::default();

        let merged = global.merge_with(&project);
        assert_eq!(merged.template, Some("global template".to_string()));
        assert_eq!(merged.template_file, None);
    }

    #[test]
    fn test_commit_generation_merge_squash_template_mutual_exclusivity() {
        // Global has squash_template_file, project has squash_template
        // Merged result should only have squash_template (project wins)
        let global = CommitGenerationConfig {
            squash_template_file: Some("~/.config/squash.txt".to_string()),
            ..Default::default()
        };
        let project = CommitGenerationConfig {
            squash_template: Some("inline squash".to_string()),
            ..Default::default()
        };

        let merged = global.merge_with(&project);
        assert_eq!(merged.squash_template, Some("inline squash".to_string()));
        assert_eq!(merged.squash_template_file, None);

        // Reverse: global has squash_template, project has squash_template_file
        let global = CommitGenerationConfig {
            squash_template: Some("global squash".to_string()),
            ..Default::default()
        };
        let project = CommitGenerationConfig {
            squash_template_file: Some("project-squash.txt".to_string()),
            ..Default::default()
        };

        let merged = global.merge_with(&project);
        assert_eq!(merged.squash_template, None);
        assert_eq!(
            merged.squash_template_file,
            Some("project-squash.txt".to_string())
        );
    }

    // =========================================================================
    // Effective config methods tests
    // =========================================================================

    #[test]
    fn test_effective_commit_generation_no_project() {
        let config = UserConfig {
            configs: OverridableConfig {
                commit: Some(CommitConfig {
                    stage: None,
                    generation: Some(CommitGenerationConfig {
                        command: Some("global-llm".to_string()),
                        ..Default::default()
                    }),
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        let effective = config.commit_generation(None);
        assert_eq!(effective.command, Some("global-llm".to_string()));
    }

    #[test]
    fn test_effective_commit_generation_with_project_override() {
        let mut config = UserConfig {
            configs: OverridableConfig {
                commit: Some(CommitConfig {
                    stage: None,
                    generation: Some(CommitGenerationConfig {
                        command: Some("global-llm".to_string()),
                        ..Default::default()
                    }),
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        config.projects.insert(
            "github.com/user/repo".to_string(),
            UserProjectOverrides {
                overrides: OverridableConfig {
                    commit: Some(CommitConfig {
                        stage: None,
                        generation: Some(CommitGenerationConfig {
                            command: Some("project-llm".to_string()),
                            ..Default::default()
                        }),
                    }),
                    ..Default::default()
                },
                ..Default::default()
            },
        );

        // With project identifier, should merge project config
        let effective = config.commit_generation(Some("github.com/user/repo"));
        assert_eq!(effective.command, Some("project-llm".to_string()));

        // Without project or unknown project, should use global
        let effective = config.commit_generation(None);
        assert_eq!(effective.command, Some("global-llm".to_string()));

        let effective = config.commit_generation(Some("github.com/other/repo"));
        assert_eq!(effective.command, Some("global-llm".to_string()));
    }

    #[test]
    fn test_effective_merge_with_partial_override() {
        let mut config = UserConfig {
            configs: OverridableConfig {
                merge: Some(MergeConfig {
                    squash: Some(true),
                    commit: Some(true),
                    rebase: Some(true),
                    remove: Some(true),
                    verify: Some(true),
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        config.projects.insert(
            "github.com/user/repo".to_string(),
            UserProjectOverrides {
                overrides: OverridableConfig {
                    merge: Some(MergeConfig {
                        squash: Some(false), // Only override squash
                        commit: None,
                        rebase: None,
                        remove: None,
                        verify: None,
                    }),
                    ..Default::default()
                },
                ..Default::default()
            },
        );

        let effective = config.merge(Some("github.com/user/repo")).unwrap();
        assert_eq!(effective.squash, Some(false)); // From project
        assert_eq!(effective.commit, Some(true)); // From global
        assert_eq!(effective.rebase, Some(true)); // From global
    }

    #[test]
    fn test_effective_list_project_only() {
        // No global list config, only project config
        let mut config = UserConfig::default();
        assert!(config.configs.list.is_none());

        config.projects.insert(
            "github.com/user/repo".to_string(),
            UserProjectOverrides {
                overrides: OverridableConfig {
                    list: Some(ListConfig {
                        full: Some(true),
                        branches: None,
                        remotes: None,
                        timeout_ms: None,
                    }),
                    ..Default::default()
                },
                ..Default::default()
            },
        );

        let effective = config.list(Some("github.com/user/repo")).unwrap();
        assert_eq!(effective.full, Some(true));
        assert!(effective.branches.is_none());

        // No global, no matching project = None
        assert!(config.list(Some("github.com/other/repo")).is_none());
    }

    #[test]
    fn test_effective_select_with_project_override() {
        // Test that OverridableConfig merge works correctly for select
        let mut config = UserConfig {
            configs: OverridableConfig {
                select: Some(SelectConfig {
                    pager: Some("delta".to_string()),
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        config.projects.insert(
            "github.com/user/repo".to_string(),
            UserProjectOverrides {
                overrides: OverridableConfig {
                    select: Some(SelectConfig {
                        pager: Some("bat".to_string()),
                    }),
                    ..Default::default()
                },
                ..Default::default()
            },
        );

        // Project override takes precedence
        let effective = config.select(Some("github.com/user/repo")).unwrap();
        assert_eq!(effective.pager, Some("bat".to_string()));

        // No project override = use global
        let effective = config.select(Some("github.com/other/repo")).unwrap();
        assert_eq!(effective.pager, Some("delta".to_string()));

        // No project = use global
        let effective = config.select(None).unwrap();
        assert_eq!(effective.pager, Some("delta".to_string()));
    }

    #[test]
    fn test_effective_commit_global_only() {
        // Only global config, no project config
        let config = UserConfig {
            configs: OverridableConfig {
                commit: Some(CommitConfig {
                    stage: Some(StageMode::Tracked),
                    generation: None,
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        let effective = config.commit(Some("github.com/any/project")).unwrap();
        assert_eq!(effective.stage, Some(StageMode::Tracked));
    }

    // =========================================================================
    // Per-project config serde tests
    // =========================================================================

    #[test]
    fn test_user_project_config_with_nested_configs_serde() {
        let config = UserProjectOverrides {
            approved_commands: vec!["npm install".to_string()],
            commit_generation: None, // Deprecated field, use commit.generation instead
            overrides: OverridableConfig {
                worktree_path: Some(".worktrees/{{ branch }}".to_string()),
                list: Some(ListConfig {
                    full: Some(true),
                    ..Default::default()
                }),
                commit: Some(CommitConfig {
                    stage: Some(StageMode::Tracked),
                    generation: Some(CommitGenerationConfig {
                        command: Some("llm -m gpt-4".to_string()),
                        ..Default::default()
                    }),
                }),
                merge: Some(MergeConfig {
                    squash: Some(false),
                    ..Default::default()
                }),
                ..Default::default()
            },
        };

        let toml = toml::to_string(&config).unwrap();
        let parsed: UserProjectOverrides = toml::from_str(&toml).unwrap();

        assert_eq!(
            parsed.overrides.worktree_path,
            Some(".worktrees/{{ branch }}".to_string())
        );
        assert_eq!(
            parsed
                .overrides
                .commit
                .as_ref()
                .unwrap()
                .generation
                .as_ref()
                .unwrap()
                .command,
            Some("llm -m gpt-4".to_string())
        );
        assert_eq!(parsed.overrides.list.as_ref().unwrap().full, Some(true));
        assert_eq!(
            parsed.overrides.commit.as_ref().unwrap().stage,
            Some(StageMode::Tracked)
        );
        assert_eq!(parsed.overrides.merge.as_ref().unwrap().squash, Some(false));
    }

    #[test]
    fn test_full_config_with_per_project_sections_serde() {
        // Test new format: [commit.generation] instead of [commit-generation]
        let content = r#"
worktree-path = "../{{ repo }}.{{ branch | sanitize }}"

[commit.generation]
command = "llm -m claude-haiku-4.5"

[projects."github.com/user/repo"]
worktree-path = ".worktrees/{{ branch | sanitize }}"
approved-commands = ["npm install"]

[projects."github.com/user/repo".commit.generation]
command = "claude -p --model opus"

[projects."github.com/user/repo".list]
full = true

[projects."github.com/user/repo".merge]
squash = false
"#;

        let config: UserConfig = toml::from_str(content).unwrap();

        // Global config
        assert_eq!(
            config.configs.worktree_path,
            Some("../{{ repo }}.{{ branch | sanitize }}".to_string())
        );
        assert_eq!(
            config
                .configs
                .commit
                .as_ref()
                .unwrap()
                .generation
                .as_ref()
                .unwrap()
                .command,
            Some("llm -m claude-haiku-4.5".to_string())
        );

        // Project config
        let project = config.projects.get("github.com/user/repo").unwrap();
        assert_eq!(
            project.overrides.worktree_path,
            Some(".worktrees/{{ branch | sanitize }}".to_string())
        );
        assert_eq!(
            project
                .overrides
                .commit
                .as_ref()
                .unwrap()
                .generation
                .as_ref()
                .unwrap()
                .command,
            Some("claude -p --model opus".to_string())
        );
        assert_eq!(project.overrides.list.as_ref().unwrap().full, Some(true));
        assert_eq!(
            project.overrides.merge.as_ref().unwrap().squash,
            Some(false)
        );

        // Effective config for project
        let effective_cg = config.commit_generation(Some("github.com/user/repo"));
        assert_eq!(
            effective_cg.command,
            Some("claude -p --model opus".to_string())
        );

        let effective_merge = config.merge(Some("github.com/user/repo")).unwrap();
        assert_eq!(effective_merge.squash, Some(false));
    }

    #[test]
    fn test_deprecated_commit_generation_format_serde() {
        // Test old format: [commit-generation] is still parsed for backward compatibility
        let content = r#"
[commit-generation]
command = "llm -m claude-haiku-4.5"

[projects."github.com/user/repo".commit-generation]
command = "claude -p --model opus"
"#;

        let config: UserConfig = toml::from_str(content).unwrap();

        // Old format parsed into commit_generation field
        assert_eq!(
            config.commit_generation.as_ref().unwrap().command,
            Some("llm -m claude-haiku-4.5".to_string())
        );

        // Project override uses deprecated field
        let project = config.projects.get("github.com/user/repo").unwrap();
        assert_eq!(
            project.commit_generation.as_ref().unwrap().command,
            Some("claude -p --model opus".to_string())
        );

        // Effective config uses the deprecated values
        let effective_cg = config.commit_generation(Some("github.com/user/repo"));
        assert_eq!(
            effective_cg.command,
            Some("claude -p --model opus".to_string())
        );
    }

    #[test]
    fn test_deprecated_commit_generation_with_args_field() {
        // Test that old format with args field still parses (args is ignored)
        // This ensures backward compatibility for users who haven't migrated yet
        let content = r#"
[commit-generation]
command = "llm"
args = ["-m", "claude-haiku-4.5"]
"#;

        let result: Result<UserConfig, _> = toml::from_str(content);
        assert!(
            result.is_ok(),
            "Old format with args field should parse (args is ignored): {:?}",
            result.err()
        );

        let config = result.unwrap();
        // Command is parsed, args is ignored (struct no longer has args field)
        assert_eq!(
            config.commit_generation.as_ref().unwrap().command,
            Some("llm".to_string())
        );
    }

    // Validation tests

    #[test]
    fn test_validation_empty_worktree_path() {
        let content = r#"worktree-path = """#;
        let result = UserConfig::load_from_str(content);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("worktree-path cannot be empty"), "{err}");
    }

    #[test]
    fn test_validation_absolute_worktree_path() {
        // Use platform-appropriate absolute path
        let content = if cfg!(windows) {
            r#"worktree-path = "C:\\absolute\\path""#
        } else {
            r#"worktree-path = "/absolute/path""#
        };
        let result = UserConfig::load_from_str(content);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("must be relative"), "{err}");
    }

    #[test]
    fn test_validation_project_empty_worktree_path() {
        let content = r#"
[projects."github.com/user/repo"]
worktree-path = ""
"#;
        let result = UserConfig::load_from_str(content);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("worktree-path cannot be empty"), "{err}");
    }

    #[test]
    fn test_validation_project_absolute_worktree_path() {
        // Use platform-appropriate absolute path
        let content = if cfg!(windows) {
            r#"
[projects."github.com/user/repo"]
worktree-path = "C:\\absolute\\path"
"#
        } else {
            r#"
[projects."github.com/user/repo"]
worktree-path = "/absolute/path"
"#
        };
        let result = UserConfig::load_from_str(content);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("must be relative"), "{err}");
    }

    #[test]
    fn test_validation_template_mutual_exclusivity() {
        let content = r#"
[commit-generation]
template = "inline template"
template-file = "path/to/file"
"#;
        let result = UserConfig::load_from_str(content);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("mutually exclusive"), "{err}");
    }

    #[test]
    fn test_validation_squash_template_mutual_exclusivity() {
        let content = r#"
[commit-generation]
squash-template = "inline template"
squash-template-file = "path/to/file"
"#;
        let result = UserConfig::load_from_str(content);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("mutually exclusive"), "{err}");
    }

    #[test]
    fn test_validation_project_template_mutual_exclusivity() {
        let content = r#"
[projects."github.com/user/repo".commit-generation]
template = "inline template"
template-file = "path/to/file"
"#;
        let result = UserConfig::load_from_str(content);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("mutually exclusive"), "{err}");
    }

    #[test]
    fn test_validation_project_squash_template_mutual_exclusivity() {
        let content = r#"
[projects."github.com/user/repo".commit-generation]
squash-template = "inline template"
squash-template-file = "path/to/file"
"#;
        let result = UserConfig::load_from_str(content);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("mutually exclusive"), "{err}");
    }

    // New format [commit.generation] validation tests

    #[test]
    fn test_validation_new_format_template_mutual_exclusivity() {
        let content = r#"
[commit.generation]
template = "inline template"
template-file = "path/to/file"
"#;
        let result = UserConfig::load_from_str(content);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("mutually exclusive"), "{err}");
    }

    #[test]
    fn test_validation_new_format_squash_template_mutual_exclusivity() {
        let content = r#"
[commit.generation]
squash-template = "inline template"
squash-template-file = "path/to/file"
"#;
        let result = UserConfig::load_from_str(content);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("mutually exclusive"), "{err}");
    }

    #[test]
    fn test_validation_new_format_project_template_mutual_exclusivity() {
        let content = r#"
[projects."github.com/user/repo".commit.generation]
template = "inline template"
template-file = "path/to/file"
"#;
        let result = UserConfig::load_from_str(content);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("mutually exclusive"), "{err}");
    }

    #[test]
    fn test_validation_new_format_project_squash_template_mutual_exclusivity() {
        let content = r#"
[projects."github.com/user/repo".commit.generation]
squash-template = "inline template"
squash-template-file = "path/to/file"
"#;
        let result = UserConfig::load_from_str(content);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("mutually exclusive"), "{err}");
    }

    // =========================================================================
    // save_to() tests
    // =========================================================================

    #[test]
    fn test_save_to_new_file_with_commit_generation() {
        // Test that save_to() creates a new file with commit.generation section
        // This exercises the "create from scratch" branch when no existing file exists
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");

        let config = UserConfig {
            configs: OverridableConfig {
                commit: Some(CommitConfig {
                    stage: None,
                    generation: Some(CommitGenerationConfig {
                        command: Some("llm -m haiku".to_string()),
                        ..Default::default()
                    }),
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        config.save_to(&config_path).unwrap();

        let saved = std::fs::read_to_string(&config_path).unwrap();
        assert!(
            saved.contains("[commit.generation]"),
            "Should use new format: {saved}"
        );
        assert!(
            saved.contains("command = \"llm -m haiku\""),
            "Should contain command: {saved}"
        );
    }

    #[test]
    fn test_save_to_new_file_with_deprecated_commit_generation() {
        // Test that save_to() serializes deprecated commit_generation field
        // (for backward compat when loading old configs and re-saving)
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");

        let config = UserConfig {
            commit_generation: Some(CommitGenerationConfig {
                command: Some("old-llm".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };

        config.save_to(&config_path).unwrap();

        let saved = std::fs::read_to_string(&config_path).unwrap();
        assert!(
            saved.contains("[commit-generation]"),
            "Should use deprecated format: {saved}"
        );
        assert!(
            saved.contains("command = \"old-llm\""),
            "Should contain command: {saved}"
        );
    }

    #[test]
    fn test_save_to_new_file_with_skip_shell_integration() {
        // Test skip-shell-integration-prompt is only written when true
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");

        let config = UserConfig {
            skip_shell_integration_prompt: true,
            ..Default::default()
        };

        config.save_to(&config_path).unwrap();

        let saved = std::fs::read_to_string(&config_path).unwrap();
        assert!(
            saved.contains("skip-shell-integration-prompt = true"),
            "Should contain flag: {saved}"
        );
    }

    #[test]
    fn test_save_to_new_file_with_worktree_path() {
        // Test worktree-path is written when set
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");

        let config = UserConfig {
            configs: OverridableConfig {
                worktree_path: Some("../{{ repo }}.{{ branch }}".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        config.save_to(&config_path).unwrap();

        let saved = std::fs::read_to_string(&config_path).unwrap();
        assert!(
            saved.contains("worktree-path = \"../{{ repo }}.{{ branch }}\""),
            "Should contain worktree-path: {saved}"
        );
    }

    // =========================================================================
    // Per-project hooks tests (append semantics)
    // =========================================================================

    /// Helper to parse hooks from TOML
    fn parse_hooks(toml_str: &str) -> HooksConfig {
        toml::from_str(toml_str).unwrap()
    }

    #[test]
    fn test_hooks_merge_append_semantics() {
        // Global has post-start, per-project has post-start
        // Both should run (global first, then per-project)
        let mut config = UserConfig {
            configs: OverridableConfig {
                hooks: parse_hooks("post-start = \"echo global\""),
                ..Default::default()
            },
            ..Default::default()
        };

        config.projects.insert(
            "github.com/user/repo".to_string(),
            UserProjectOverrides {
                overrides: OverridableConfig {
                    hooks: parse_hooks("post-start = \"echo project\""),
                    ..Default::default()
                },
                ..Default::default()
            },
        );

        let effective = config.hooks(Some("github.com/user/repo"));
        let post_start = effective.post_start.unwrap();
        let commands = post_start.commands();
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0].template, "echo global");
        assert_eq!(commands[1].template, "echo project");
    }

    #[test]
    fn test_hooks_no_project_override_uses_global() {
        // Global has hooks, project doesn't - global hooks used
        let config = UserConfig {
            configs: OverridableConfig {
                hooks: parse_hooks("post-start = \"echo global\""),
                ..Default::default()
            },
            ..Default::default()
        };

        let effective = config.hooks(Some("github.com/other/repo"));
        let post_start = effective.post_start.unwrap();
        let commands = post_start.commands();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].template, "echo global");
    }

    #[test]
    fn test_hooks_project_only_no_global() {
        // Project has hooks, global doesn't - project hooks used
        let mut config = UserConfig::default();

        config.projects.insert(
            "github.com/user/repo".to_string(),
            UserProjectOverrides {
                overrides: OverridableConfig {
                    hooks: parse_hooks("post-start = \"echo project\""),
                    ..Default::default()
                },
                ..Default::default()
            },
        );

        let effective = config.hooks(Some("github.com/user/repo"));
        let post_start = effective.post_start.unwrap();
        let commands = post_start.commands();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].template, "echo project");
    }

    #[test]
    fn test_hooks_different_hook_types_not_merged() {
        // Global has post-start, per-project has pre-commit
        // These should remain separate (different hook types)
        let mut config = UserConfig {
            configs: OverridableConfig {
                hooks: parse_hooks("post-start = \"echo global-start\""),
                ..Default::default()
            },
            ..Default::default()
        };

        config.projects.insert(
            "github.com/user/repo".to_string(),
            UserProjectOverrides {
                overrides: OverridableConfig {
                    hooks: parse_hooks("pre-commit = \"echo project-commit\""),
                    ..Default::default()
                },
                ..Default::default()
            },
        );

        let effective = config.hooks(Some("github.com/user/repo"));

        // post-start: only global
        let post_start = effective.post_start.unwrap();
        let start_commands = post_start.commands();
        assert_eq!(start_commands.len(), 1);
        assert_eq!(start_commands[0].template, "echo global-start");

        // pre-commit: only project
        let pre_commit = effective.pre_commit.unwrap();
        let commit_commands = pre_commit.commands();
        assert_eq!(commit_commands.len(), 1);
        assert_eq!(commit_commands[0].template, "echo project-commit");
    }

    #[test]
    fn test_hooks_none_project_uses_global() {
        // When no project is provided, only global hooks are used
        let config = UserConfig {
            configs: OverridableConfig {
                hooks: parse_hooks("post-start = \"echo global\""),
                ..Default::default()
            },
            ..Default::default()
        };

        let effective = config.hooks(None);
        let post_start = effective.post_start.unwrap();
        let commands = post_start.commands();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].template, "echo global");
    }

    #[test]
    fn test_hooks_in_overridable_config_is_empty() {
        // Default hooks should be considered empty
        let config = OverridableConfig::default();
        assert!(config.is_empty());

        // With hooks set, should not be empty
        let config = OverridableConfig {
            hooks: parse_hooks("post-start = \"echo test\""),
            ..Default::default()
        };
        assert!(!config.is_empty());
    }

    /// Validates that valid_user_config_keys() includes all hook types from HookType enum.
    ///
    /// The JsonSchema derivation should include all HooksConfig fields, which correspond
    /// to HookType variants. HookType uses strum's Display with kebab-case serialization,
    /// which matches the serde field names.
    #[test]
    fn test_valid_user_config_keys_includes_all_hook_types() {
        use crate::git::HookType;
        use strum::IntoEnumIterator;

        let valid_keys = valid_user_config_keys();

        for hook_type in HookType::iter() {
            let key = hook_type.to_string(); // e.g., "post-create", "pre-merge"
            assert!(
                valid_keys.contains(&key),
                "HookType::{hook_type:?} ({key}) is missing from valid_user_config_keys()"
            );
        }
    }

    /// Validates that all keys from valid_user_config_keys() are accepted by serde.
    ///
    /// Creates a TOML config with each key set to a valid value and verifies
    /// deserialization succeeds. This ensures the JsonSchema matches serde's expectations.
    #[test]
    fn test_valid_user_config_keys_all_deserialize() {
        let valid_keys = valid_user_config_keys();

        // Build a TOML string with all keys
        // Top-level scalar values must come before table sections
        let mut scalar_lines = Vec::new();
        let mut table_lines = Vec::new();

        for key in &valid_keys {
            match key.as_str() {
                "projects" => continue, // Skip - table type tested separately
                "skip-shell-integration-prompt" => {
                    scalar_lines.push(format!("{key} = true"));
                }
                "worktree-path" => {
                    scalar_lines.push(format!("{key} = \"test-value\""));
                }
                "list" | "commit" | "merge" | "select" | "commit-generation" => {
                    // Table sections with minimal content
                    table_lines.push(format!("[{key}]"));
                }
                // Hook keys take string values
                _ => {
                    scalar_lines.push(format!("{key} = \"test-value\""));
                }
            };
        }

        // Scalars first, then tables
        scalar_lines.extend(table_lines);
        let toml_content = scalar_lines.join("\n");

        // Should deserialize without error
        let result: Result<UserConfig, _> = toml::from_str(&toml_content);
        assert!(
            result.is_ok(),
            "Failed to deserialize config with all valid keys:\n{toml_content}\nError: {:?}",
            result.err()
        );
    }
}
