//! User-level configuration
//!
//! Personal preferences and per-project approved commands, not checked into git.

use config::{Config, ConfigError, File};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::OnceLock;

#[cfg(not(test))]
use etcetera::base_strategy::{BaseStrategy, choose_base_strategy};

/// Global override for config path, set via --config CLI flag
static CONFIG_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Set the global config path override (called from CLI --config flag)
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
)]
#[serde(rename_all = "lowercase")]
pub enum StageMode {
    /// Stage everything: untracked files + unstaged tracked changes
    #[default]
    All,
    /// Stage tracked changes only (like `git add -u`)
    Tracked,
    /// Stage nothing, commit only what's already in the index
    None,
}

impl StageMode {
    /// Returns the lowercase string representation for CLI display.
    pub fn as_str(self) -> &'static str {
        match self {
            StageMode::All => "all",
            StageMode::Tracked => "tracked",
            StageMode::None => "none",
        }
    }
}

/// User-level configuration for worktree path formatting and LLM integration.
///
/// This config is stored at `~/.config/worktrunk/config.toml` (or platform equivalent)
/// and is NOT checked into git. Each developer maintains their own user config.
///
/// The `worktree-path` template is relative to the repository root.
/// Supported variables:
/// - `{{ main_worktree }}` - Main worktree directory name
/// - `{{ branch }}` - Branch name (slashes replaced with dashes)
///
/// # Examples
///
/// ```toml
/// # Default - parent directory siblings
/// worktree-path = "../{{ main_worktree }}.{{ branch }}"
///
/// # Inside repo (clean, no redundant directory)
/// worktree-path = ".worktrees/{{ branch }}"
///
/// # Repository-namespaced (useful for shared directories with multiple repos)
/// worktree-path = "../worktrees/{{ main_worktree }}/{{ branch }}"
///
/// # Commit generation configuration
/// [commit-generation]
/// command = "llm"  # Command to invoke for generating commit messages (e.g., "llm", "claude")
/// args = ["-s"]    # Arguments to pass to the command
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
/// `__` separator for nested fields (e.g., `WORKTRUNK_COMMIT_GENERATION__COMMAND`).
#[derive(Debug, Serialize, Deserialize)]
pub struct WorktrunkConfig {
    #[serde(rename = "worktree-path")]
    pub worktree_path: String,

    #[serde(default, rename = "commit-generation")]
    pub commit_generation: CommitGenerationConfig,

    /// Per-project configuration (approved commands, etc.)
    /// Uses BTreeMap for deterministic serialization order and better diff readability
    #[serde(default)]
    pub projects: std::collections::BTreeMap<String, UserProjectConfig>,

    /// Configuration for the `wt list` command
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list: Option<ListConfig>,

    /// Configuration for the `wt step commit` command (also used by merge)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<CommitConfig>,

    /// Configuration for the `wt merge` command
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge: Option<MergeConfig>,
}

/// Configuration for commit message generation
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct CommitGenerationConfig {
    /// Command to invoke for generating commit messages (e.g., "llm", "claude")
    #[serde(default)]
    pub command: Option<String>,

    /// Arguments to pass to the command
    #[serde(default)]
    pub args: Vec<String>,

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

/// Per-project user configuration
///
/// Stored in the user's config file under `[projects."project-id"]`.
/// Contains project-specific settings that are user preferences, not checked into git.
///
/// # TOML Format
/// ```toml
/// [projects."github.com/user/repo"]
/// approved-commands = ["npm install", "npm test"]
/// ```
///
/// # Future Extensibility
/// This structure is designed to accommodate additional per-project settings:
/// - default-target-branch
/// - auto-squash preferences
/// - project-specific hooks
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
pub struct UserProjectConfig {
    /// Commands that have been approved for automatic execution in this project
    #[serde(
        default,
        rename = "approved-commands",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub approved_commands: Vec<String>,
}

/// Configuration for the `wt list` command
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ListConfig {
    /// Show CI, conflicts, and diffs by default
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full: Option<bool>,

    /// Include branches without worktrees by default
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branches: Option<bool>,

    /// Include remote branches by default
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remotes: Option<bool>,
}

/// Configuration for the `wt step commit` command
///
/// Also used by `wt merge` for shared settings like `stage`.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
pub struct CommitConfig {
    /// What to stage before committing (default: all)
    /// Values: "all", "tracked", "none"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<StageMode>,
}

/// Configuration for the `wt merge` command
///
/// Note: `stage` defaults from `[commit]` section, not here.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct MergeConfig {
    /// Squash commits when merging (default: true)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub squash: Option<bool>,

    /// Create commit after merge (default: true)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<bool>,

    /// Remove worktree after merge (default: true)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remove: Option<bool>,

    /// Run project hooks (default: true)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verify: Option<bool>,
}

impl Default for WorktrunkConfig {
    fn default() -> Self {
        Self {
            worktree_path: "../{{ main_worktree }}.{{ branch }}".to_string(),
            commit_generation: CommitGenerationConfig::default(),
            projects: std::collections::BTreeMap::new(),
            list: None,
            commit: None,
            merge: None,
        }
    }
}

impl WorktrunkConfig {
    /// Load configuration from config file and environment variables.
    ///
    /// Configuration is loaded in the following order (later sources override earlier ones):
    /// 1. Default values
    /// 2. Config file (see struct documentation for platform-specific paths)
    /// 3. Environment variables (WORKTRUNK_*)
    pub fn load() -> Result<Self, ConfigError> {
        let defaults = Self::default();

        let mut builder = Config::builder()
            .set_default("worktree-path", defaults.worktree_path)?
            .set_default(
                "commit-generation.command",
                defaults.commit_generation.command.unwrap_or_default(),
            )?
            .set_default("commit-generation.args", defaults.commit_generation.args)?;

        // Add config file if it exists
        if let Some(config_path) = get_config_path()
            && config_path.exists()
        {
            builder = builder.add_source(File::from(config_path));
        }

        // Add environment variables with WORKTRUNK prefix
        // Uses "__" separator (default) to support field names with underscores
        // Example: WORKTRUNK_COMMIT_GENERATION__COMMAND maps to commit-generation.command
        builder = builder.add_source(config::Environment::with_prefix("WORKTRUNK").separator("__"));

        let config: Self = builder.build()?.try_deserialize()?;

        // Validate worktree path
        if config.worktree_path.is_empty() {
            return Err(ConfigError::Message("worktree-path cannot be empty".into()));
        }
        if std::path::Path::new(&config.worktree_path).is_absolute() {
            return Err(ConfigError::Message(
                "worktree-path must be relative, not absolute".into(),
            ));
        }

        // Validate commit generation config
        if config.commit_generation.template.is_some()
            && config.commit_generation.template_file.is_some()
        {
            return Err(ConfigError::Message(
                "commit-generation.template and commit-generation.template-file are mutually exclusive".into(),
            ));
        }

        if config.commit_generation.squash_template.is_some()
            && config.commit_generation.squash_template_file.is_some()
        {
            return Err(ConfigError::Message(
                "commit-generation.squash-template and commit-generation.squash-template-file are mutually exclusive".into(),
            ));
        }

        Ok(config)
    }

    /// Format a worktree path using this configuration's template.
    ///
    /// # Arguments
    /// * `main_worktree` - Main worktree directory name (replaces {{ main_worktree }} in template)
    /// * `branch` - Branch name (replaces {{ branch }} in template, slashes sanitized to dashes)
    ///
    /// # Examples
    /// ```
    /// use worktrunk::config::WorktrunkConfig;
    ///
    /// let config = WorktrunkConfig::default();
    /// let path = config.format_path("myproject", "feature/foo").unwrap();
    /// assert_eq!(path, "../myproject.feature-foo");
    /// ```
    pub fn format_path(&self, main_worktree: &str, branch: &str) -> Result<String, String> {
        expand_template(
            &self.worktree_path,
            main_worktree,
            branch,
            &std::collections::HashMap::new(),
        )
    }

    /// Check if a command is approved for the given project
    pub fn is_command_approved(&self, project: &str, command: &str) -> bool {
        self.projects
            .get(project)
            .map(|p| p.approved_commands.iter().any(|c| c == command))
            .unwrap_or(false)
    }

    /// Add an approved command and save to config file
    pub fn approve_command(&mut self, project: String, command: String) -> Result<(), ConfigError> {
        self.approve_command_to(project, command, None)
    }

    /// Add an approved command and save to a specific config file (for testing)
    pub fn approve_command_to(
        &mut self,
        project: String,
        command: String,
        config_path: Option<&std::path::Path>,
    ) -> Result<(), ConfigError> {
        if self.is_command_approved(&project, &command) {
            return Ok(());
        }

        self.projects
            .entry(project)
            .or_default()
            .approved_commands
            .push(command);
        self.save_impl(config_path)
    }

    /// Revoke an approved command and save to config file
    pub fn revoke_command(&mut self, project: &str, command: &str) -> Result<(), ConfigError> {
        self.revoke_command_to(project, command, None)
    }

    /// Revoke an approved command and save to a specific config file (for testing)
    #[doc(hidden)]
    pub fn revoke_command_to(
        &mut self,
        project: &str,
        command: &str,
        config_path: Option<&std::path::Path>,
    ) -> Result<(), ConfigError> {
        if let Some(project_config) = self.projects.get_mut(project) {
            let len_before = project_config.approved_commands.len();
            project_config.approved_commands.retain(|c| c != command);
            let changed = len_before != project_config.approved_commands.len();

            if project_config.approved_commands.is_empty() {
                self.projects.remove(project);
            }

            if changed {
                self.save_impl(config_path)?;
            }
        }
        Ok(())
    }

    /// Remove all approvals for a project and save to config file
    pub fn revoke_project(&mut self, project: &str) -> Result<(), ConfigError> {
        self.revoke_project_to(project, None)
    }

    /// Remove all approvals for a project and save to a specific config file (for testing)
    #[doc(hidden)]
    pub fn revoke_project_to(
        &mut self,
        project: &str,
        config_path: Option<&std::path::Path>,
    ) -> Result<(), ConfigError> {
        if self.projects.remove(project).is_some() {
            self.save_impl(config_path)?;
        }
        Ok(())
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
                    ConfigError::Message("Could not determine config path".to_string())
                })?;
                self.save_to(&path)
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

            // Only update the projects section - that's the only thing we modify programmatically
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
                    let commands: toml_edit::Array =
                        project_config.approved_commands.iter().collect();
                    projects[project_id]["approved-commands"] = toml_edit::value(commands);
                }
            }

            doc.to_string()
        } else {
            // No existing file, create from scratch
            toml::to_string_pretty(self)
                .map_err(|e| ConfigError::Message(format!("Failed to serialize config: {}", e)))?
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
