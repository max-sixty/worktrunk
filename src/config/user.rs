//! User-level configuration
//!
//! Personal preferences and per-project approved commands, not checked into git.

use config::{Case, Config, ConfigError, File};
use serde::{Deserialize, Deserializer, Serialize};
use std::path::PathBuf;
use std::sync::OnceLock;

use super::HooksConfig;

/// Deserialize a Vec<String> that can also accept a single String
/// This enables setting array config fields via environment variables
fn deserialize_string_or_vec<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de;

    struct StringOrVec;

    impl<'de> de::Visitor<'de> for StringOrVec {
        type Value = Vec<String>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("string or array of strings")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(vec![value.to_string()])
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: de::SeqAccess<'de>,
        {
            let mut vec = Vec::new();
            while let Some(elem) = seq.next_element()? {
                vec.push(elem);
            }
            Ok(vec)
        }
    }

    deserializer.deserialize_any(StringOrVec)
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
/// - `{{ repo }}` - Repository directory name
/// - `{{ branch }}` - Raw branch name (e.g., `feature/foo`)
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
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct WorktrunkConfig {
    #[serde(
        rename = "worktree-path",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) worktree_path: Option<String>,

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

    // =========================================================================
    // User-level hooks (same syntax as project hooks, run before project hooks)
    // =========================================================================
    #[serde(flatten, default)]
    pub hooks: HooksConfig,

    /// Skip the first-run shell integration prompt
    #[serde(
        default,
        rename = "skip-shell-integration-prompt",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub skip_shell_integration_prompt: bool,

    /// Captures unknown fields for validation warnings
    #[serde(flatten, default, skip_serializing)]
    pub(crate) unknown: std::collections::HashMap<String, toml::Value>,
}

/// Configuration for commit message generation
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct CommitGenerationConfig {
    /// Command to invoke for generating commit messages (e.g., "llm", "claude")
    #[serde(default)]
    pub command: Option<String>,

    /// Arguments to pass to the command
    /// Accepts either an array or a single string (for env var compatibility)
    #[serde(default, deserialize_with = "deserialize_string_or_vec")]
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
    /// Show CI and `main` diffstat by default
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

/// Default worktree path template
fn default_worktree_path() -> String {
    "../{{ repo }}.{{ branch | sanitize }}".to_string()
}

impl WorktrunkConfig {
    /// Returns the worktree path template, falling back to the default if not set.
    pub fn worktree_path(&self) -> String {
        self.worktree_path
            .clone()
            .unwrap_or_else(default_worktree_path)
    }

    /// Load configuration from config file and environment variables.
    ///
    /// Configuration is loaded in the following order (later sources override earlier ones):
    /// 1. Default values
    /// 2. Config file (see struct documentation for platform-specific paths)
    /// 3. Environment variables (WORKTRUNK_*)
    pub fn load() -> Result<Self, ConfigError> {
        let defaults = Self::default();

        // Note: worktree-path has no default set here - it's handled by the getter
        // which returns the default when None. This allows us to distinguish
        // "user explicitly set this" from "using default".
        let mut builder = Config::builder()
            .set_default(
                "commit-generation.command",
                defaults.commit_generation.command.unwrap_or_default(),
            )?
            .set_default("commit-generation.args", defaults.commit_generation.args)?;

        // Add config file if it exists
        if let Some(config_path) = get_config_path()
            && config_path.exists()
        {
            // Check for deprecated template variables and create migration file if needed
            // User config always gets migration file (it's global, not worktree-specific)
            if let Ok(content) = std::fs::read_to_string(&config_path) {
                let _ = super::deprecation::check_and_migrate(
                    &config_path,
                    &content,
                    true,
                    "User config",
                );
            }

            builder = builder.add_source(File::from(config_path.clone()));
        }

        // Add environment variables with WORKTRUNK prefix
        // - prefix_separator("_"): strip prefix with single underscore (WORKTRUNK_ → key)
        // - separator("__"): double underscore for nested fields (COMMIT_GENERATION__COMMAND → commit-generation.command)
        // - convert_case(Kebab): converts snake_case to kebab-case to match serde field names
        // Example: WORKTRUNK_WORKTREE_PATH → worktree-path
        builder = builder.add_source(
            config::Environment::with_prefix("WORKTRUNK")
                .prefix_separator("_")
                .separator("__")
                .convert_case(Case::Kebab),
        );

        let config: Self = builder.build()?.try_deserialize()?;

        // Validate worktree path (only if explicitly set - default is always valid)
        if let Some(ref path) = config.worktree_path {
            if path.is_empty() {
                return Err(ConfigError::Message("worktree-path cannot be empty".into()));
            }
            if std::path::Path::new(path).is_absolute() {
                return Err(ConfigError::Message(
                    "worktree-path must be relative, not absolute".into(),
                ));
            }
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
    /// * `branch` - Branch name (replaces {{ branch }} in template; use `{{ branch | sanitize }}` for paths)
    ///
    /// # Examples
    /// ```
    /// use worktrunk::config::WorktrunkConfig;
    ///
    /// // Default template uses {{ branch | sanitize }} for filesystem-safe paths
    /// let config = WorktrunkConfig::default();
    /// let path = config.format_path("myproject", "feature/foo").unwrap();
    /// assert_eq!(path, "../myproject.feature-foo");
    /// ```
    pub fn format_path(&self, main_worktree: &str, branch: &str) -> Result<String, String> {
        use std::collections::HashMap;
        let mut vars = HashMap::new();
        vars.insert("main_worktree", main_worktree);
        vars.insert("repo", main_worktree);
        vars.insert("branch", branch);
        expand_template(&self.worktree_path(), &vars, false)
    }

    /// Check if a command is approved for the given project
    pub fn is_command_approved(&self, project: &str, command: &str) -> bool {
        self.projects
            .get(project)
            .map(|p| p.approved_commands.iter().any(|c| c == command))
            .unwrap_or(false)
    }

    /// Add an approved command and save to config file.
    ///
    /// Reloads from disk before modifying to reduce race conditions from concurrent processes.
    /// Pass `None` for default config path, or `Some(path)` for testing.
    pub fn approve_command(
        &mut self,
        project: String,
        command: String,
        config_path: Option<&std::path::Path>,
    ) -> Result<(), ConfigError> {
        // Reload from disk first to get fresh state (fixes race condition where
        // concurrent approvals would overwrite each other)
        self.reload_projects_from(config_path)?;

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

        let disk_config: WorktrunkConfig = toml::from_str(&content).map_err(|e| {
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
    /// Reloads from disk before modifying to reduce race conditions from concurrent processes.
    /// Pass `None` for default config path, or `Some(path)` for testing.
    pub fn revoke_command(
        &mut self,
        project: &str,
        command: &str,
        config_path: Option<&std::path::Path>,
    ) -> Result<(), ConfigError> {
        // Reload from disk first to get fresh state (fixes race condition)
        self.reload_projects_from(config_path)?;

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

    /// Remove all approvals for a project and save to config file.
    ///
    /// Reloads from disk before modifying to reduce race conditions from concurrent processes.
    /// Pass `None` for default config path, or `Some(path)` for testing.
    pub fn revoke_project(
        &mut self,
        project: &str,
        config_path: Option<&std::path::Path>,
    ) -> Result<(), ConfigError> {
        // Reload from disk first to get fresh state (fixes race condition)
        self.reload_projects_from(config_path)?;

        if self.projects.remove(project).is_some() {
            self.save_impl(config_path)?;
        }
        Ok(())
    }

    /// Set `skip-shell-integration-prompt = true` and save.
    ///
    /// Pass `None` for default config path, or `Some(path)` for testing.
    pub fn set_skip_shell_integration_prompt(
        &mut self,
        config_path: Option<&std::path::Path>,
    ) -> Result<(), ConfigError> {
        if self.skip_shell_integration_prompt {
            return Ok(());
        }
        // Reload from disk first to avoid clobbering concurrent changes
        self.reload_projects_from(config_path)?;
        self.skip_shell_integration_prompt = true;
        self.save_impl(config_path)
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
                    let commands =
                        Self::format_multiline_array(project_config.approved_commands.iter());
                    projects[project_id]["approved-commands"] = toml_edit::value(commands);
                }
            }

            doc.to_string()
        } else {
            // No existing file, create from scratch using toml_edit for consistent formatting
            let mut doc = toml_edit::DocumentMut::new();

            // Only write worktree-path if explicitly set (not the default)
            if let Some(ref path) = self.worktree_path {
                doc["worktree-path"] = toml_edit::value(path);
            }

            // skip-shell-integration-prompt (only if true)
            if self.skip_shell_integration_prompt {
                doc["skip-shell-integration-prompt"] = toml_edit::value(true);
            }

            // commit-generation section
            doc["commit-generation"] = toml_edit::Item::Table(toml_edit::Table::new());
            let commit_args: toml_edit::Array = self.commit_generation.args.iter().collect();
            doc["commit-generation"]["args"] = toml_edit::value(commit_args);
            if let Some(ref cmd) = self.commit_generation.command {
                doc["commit-generation"]["command"] = toml_edit::value(cmd);
            }

            // projects section with multiline arrays
            if !self.projects.is_empty() {
                let mut projects_table = toml_edit::Table::new();
                projects_table.set_implicit(true); // Don't emit [projects] header
                for (project_id, project_config) in &self.projects {
                    let mut table = toml_edit::Table::new();
                    let commands =
                        Self::format_multiline_array(project_config.approved_commands.iter());
                    table["approved-commands"] = toml_edit::value(commands);
                    projects_table[project_id] = toml_edit::Item::Table(table);
                }
                doc["projects"] = toml_edit::Item::Table(projects_table);
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

/// Find unknown keys in user config TOML content
///
/// Returns a list of unrecognized top-level keys that will be silently ignored.
/// Uses serde deserialization with flatten to automatically detect unknown fields.
pub fn find_unknown_keys(contents: &str) -> Vec<String> {
    // Deserialize into WorktrunkConfig - unknown fields are captured in the `unknown` map
    let Ok(config) = toml::from_str::<WorktrunkConfig>(contents) else {
        return vec![];
    };

    config.unknown.into_keys().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_unknown_keys_empty() {
        // Valid config with no unknown keys
        let content = r#"
worktree-path = "../{{ main_worktree }}.{{ branch }}"
"#;
        let keys = find_unknown_keys(content);
        assert!(keys.is_empty());
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
        assert!(keys.contains(&"unknown-key".to_string()));
        assert!(keys.contains(&"another-unknown".to_string()));
    }

    #[test]
    fn test_find_unknown_keys_invalid_toml() {
        // Invalid TOML should return empty
        let content = "this is not valid toml {{{";
        let keys = find_unknown_keys(content);
        assert!(keys.is_empty());
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
        let config = UserProjectConfig::default();
        assert!(config.approved_commands.is_empty());
    }

    #[test]
    fn test_list_config_serde() {
        let config = ListConfig {
            full: Some(true),
            branches: Some(false),
            remotes: None,
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: ListConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.full, Some(true));
        assert_eq!(parsed.branches, Some(false));
        assert_eq!(parsed.remotes, None);
    }

    #[test]
    fn test_commit_config_default() {
        let config = CommitConfig::default();
        assert!(config.stage.is_none());
    }

    #[test]
    fn test_worktrunk_config_default() {
        let config = WorktrunkConfig::default();
        // worktree_path is None by default, but the getter returns the default
        assert!(config.worktree_path.is_none());
        assert_eq!(
            config.worktree_path(),
            "../{{ repo }}.{{ branch | sanitize }}"
        );
        assert!(config.projects.is_empty());
        assert!(config.list.is_none());
        assert!(config.commit.is_none());
        assert!(config.merge.is_none());
        assert!(!config.commit_generation.is_configured());
        assert!(!config.skip_shell_integration_prompt);
    }

    #[test]
    fn test_worktrunk_config_is_command_approved_empty() {
        let config = WorktrunkConfig::default();
        assert!(!config.is_command_approved("some/project", "npm install"));
    }

    #[test]
    fn test_worktrunk_config_is_command_approved_with_commands() {
        let mut config = WorktrunkConfig::default();
        config.projects.insert(
            "github.com/user/repo".to_string(),
            UserProjectConfig {
                approved_commands: vec!["npm install".to_string(), "npm test".to_string()],
            },
        );
        assert!(config.is_command_approved("github.com/user/repo", "npm install"));
        assert!(config.is_command_approved("github.com/user/repo", "npm test"));
        assert!(!config.is_command_approved("github.com/user/repo", "rm -rf /"));
        assert!(!config.is_command_approved("other/project", "npm install"));
    }

    #[test]
    fn test_worktrunk_config_format_path() {
        let config = WorktrunkConfig::default();
        let path = config.format_path("myrepo", "feature/branch").unwrap();
        assert_eq!(path, "../myrepo.feature-branch");
    }

    #[test]
    fn test_worktrunk_config_format_path_custom_template() {
        let config = WorktrunkConfig {
            worktree_path: Some(".worktrees/{{ branch }}".to_string()),
            ..Default::default()
        };
        let path = config.format_path("myrepo", "feature").unwrap();
        assert_eq!(path, ".worktrees/feature");
    }

    #[test]
    fn test_deserialize_string_or_vec_from_string() {
        #[derive(serde::Deserialize)]
        struct Test {
            #[serde(deserialize_with = "deserialize_string_or_vec")]
            args: Vec<String>,
        }

        let json = r#"{"args": "single"}"#;
        let parsed: Test = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.args, vec!["single".to_string()]);
    }

    #[test]
    fn test_deserialize_string_or_vec_from_array() {
        #[derive(serde::Deserialize)]
        struct Test {
            #[serde(deserialize_with = "deserialize_string_or_vec")]
            args: Vec<String>,
        }

        let json = r#"{"args": ["one", "two", "three"]}"#;
        let parsed: Test = serde_json::from_str(json).unwrap();
        assert_eq!(
            parsed.args,
            vec!["one".to_string(), "two".to_string(), "three".to_string()]
        );
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
        let config = WorktrunkConfig::default();
        assert!(!config.skip_shell_integration_prompt);
    }

    #[test]
    fn test_skip_shell_integration_prompt_serde_roundtrip() {
        // Test serialization when true
        let config = WorktrunkConfig {
            skip_shell_integration_prompt: true,
            ..WorktrunkConfig::default()
        };
        let toml = toml::to_string(&config).unwrap();
        assert!(toml.contains("skip-shell-integration-prompt = true"));

        // Test deserialization
        let parsed: WorktrunkConfig = toml::from_str(&toml).unwrap();
        assert!(parsed.skip_shell_integration_prompt);
    }

    #[test]
    fn test_skip_shell_integration_prompt_skipped_when_false() {
        // When false, the field should not appear in serialized output
        let config = WorktrunkConfig::default();
        let toml = toml::to_string(&config).unwrap();
        assert!(!toml.contains("skip-shell-integration-prompt"));
    }

    #[test]
    fn test_skip_shell_integration_prompt_parsed_from_toml() {
        let content = r#"
worktree-path = "../{{ main_worktree }}.{{ branch }}"
skip-shell-integration-prompt = true
"#;
        let config: WorktrunkConfig = toml::from_str(content).unwrap();
        assert!(config.skip_shell_integration_prompt);
    }

    #[test]
    fn test_skip_shell_integration_prompt_defaults_when_missing() {
        let content = r#"
worktree-path = "../{{ main_worktree }}.{{ branch }}"
"#;
        let config: WorktrunkConfig = toml::from_str(content).unwrap();
        assert!(!config.skip_shell_integration_prompt);
    }
}
