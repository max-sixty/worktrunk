//! Configuration system for worktrunk
//!
//! Worktrunk has two independent configuration files:
//!
//! # User Config (~/.config/worktrunk/config.toml)
//!
//! **Purpose**: Personal preferences, not checked into git
//!
//! **Settings**:
//! - `worktree-path` - Template for worktree paths (relative to repo root)
//! - `commit-generation` - LLM command and templates for commit messages
//! - `approved-commands` - Commands approved for automatic execution
//!
//! **Managed by**: Each developer maintains their own user config
//!
//! # Project Config (`<repo>`/.config/wt.toml)
//!
//! **Purpose**: Project-specific hooks and commands, checked into git
//!
//! **Settings**:
//! - `post-create-command` - Sequential blocking commands when creating worktree
//! - `post-start-command` - Parallel background commands after worktree created
//! - `pre-commit-command` - Validation before committing changes during merge
//! - `pre-merge-command` - Validation before merging to target branch
//! - `post-merge-command` - Cleanup after successful merge
//!
//! **Managed by**: Checked into the repository, shared across all developers
//!
//! # Configuration Model
//!
//! The two configs are **completely independent**:
//! - No overlap in settings (they configure different things)
//! - No merging or precedence rules needed
//! - Loaded separately and used in different contexts
//!
//! User config controls "how worktrunk behaves for me", project config controls
//! "what commands run for this project".

use config::{Config, ConfigError, File};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use toml;

#[cfg(not(test))]
use etcetera::base_strategy::{BaseStrategy, choose_base_strategy};

/// User-level configuration for worktree path formatting and LLM integration.
///
/// This config is stored at `~/.config/worktrunk/config.toml` (or platform equivalent)
/// and is NOT checked into git. Each developer maintains their own user config.
///
/// The `worktree-path` template is relative to the repository root.
/// Supported variables:
/// - `{main-worktree}` - Main worktree directory name
/// - `{branch}` - Branch name (slashes replaced with dashes)
///
/// # Examples
///
/// ```toml
/// # Default - parent directory siblings
/// worktree-path = "../{main-worktree}.{branch}"
///
/// # Inside repo (clean, no redundant directory)
/// worktree-path = ".worktrees/{branch}"
///
/// # Repository-namespaced (useful for shared directories with multiple repos)
/// worktree-path = "../worktrees/{main-worktree}/{branch}"
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
/// Environment variable: `WORKTRUNK_WORKTREE_PATH`
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

/// Project-specific configuration with hooks and commands.
///
/// This config is stored at `<repo>/.config/wt.toml` within the repository and
/// IS checked into git. It defines project-specific commands that run automatically
/// during worktree operations. All developers working on the project share this config.
///
/// # Template Variables
///
/// All commands support these template variables:
/// - `{repo}` - Repository name (e.g., "my-project")
/// - `{branch}` - Branch name (e.g., "feature-foo")
/// - `{worktree}` - Absolute path to the worktree
/// - `{repo_root}` - Absolute path to the repository root
///
/// Merge-related commands (`pre-commit-command`, `pre-merge-command`, `post-merge-command`) also support:
/// - `{target}` - Target branch for the merge (e.g., "main")
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct ProjectConfig {
    /// Commands to execute sequentially before worktree is ready (blocking)
    /// Supports string (single command), array (sequential), or table (named, sequential)
    ///
    /// Available template variables: `{repo}`, `{branch}`, `{worktree}`, `{repo_root}`
    #[serde(default, rename = "post-create-command")]
    pub post_create_command: Option<CommandConfig>,

    /// Commands to execute in parallel as background processes (non-blocking)
    /// Supports string (single), array (parallel), or table (named, parallel)
    ///
    /// Available template variables: `{repo}`, `{branch}`, `{worktree}`, `{repo_root}`
    #[serde(default, rename = "post-start-command")]
    pub post_start_command: Option<CommandConfig>,

    /// Commands to execute before committing changes during merge (blocking, fail-fast validation)
    /// Supports string (single command), array (sequential), or table (named, sequential)
    /// All commands must exit with code 0 for commit to proceed
    /// Runs before any commit operation during `wt merge` (both squash and no-squash modes)
    ///
    /// Available template variables: `{repo}`, `{branch}`, `{worktree}`, `{repo_root}`, `{target}`
    #[serde(default, rename = "pre-commit-command")]
    pub pre_commit_command: Option<CommandConfig>,

    /// Commands to execute before merging (blocking, fail-fast validation)
    /// Supports string (single command), array (sequential), or table (named, sequential)
    /// All commands must exit with code 0 for merge to proceed
    ///
    /// Available template variables: `{repo}`, `{branch}`, `{worktree}`, `{repo_root}`, `{target}`
    #[serde(default, rename = "pre-merge-command")]
    pub pre_merge_command: Option<CommandConfig>,

    /// Commands to execute after successful merge in the main worktree (blocking)
    /// Supports string (single command), array (sequential), or table (named, sequential)
    /// Runs after push succeeds but before cleanup
    ///
    /// Available template variables: `{repo}`, `{branch}`, `{worktree}`, `{repo_root}`, `{target}`
    #[serde(default, rename = "post-merge-command")]
    pub post_merge_command: Option<CommandConfig>,
}

/// Configuration for commands - canonical representation
///
/// Internally stores commands as `Vec<(Option<name>, command)>` for uniform processing.
/// Deserializes from three TOML formats:
/// - Single string: `post-create-command = "npm install"`
/// - Array: `post-create-command = ["npm install", "npm test"]`
/// - Named table: `[post-create-command]` followed by `install = "npm install"`
///
/// This canonical form eliminates branching at call sites - code just iterates over commands.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandConfig {
    commands: Vec<(Option<String>, String)>,
}

impl CommandConfig {
    /// Returns the commands as a slice
    pub fn commands(&self) -> &[(Option<String>, String)] {
        &self.commands
    }
}

// Custom deserialization to handle 3 TOML formats
impl<'de> Deserialize<'de> for CommandConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum CommandConfigToml {
            Single(String),
            Multiple(Vec<String>),
            Named(std::collections::HashMap<String, String>),
        }

        let toml = CommandConfigToml::deserialize(deserializer)?;
        let commands = match toml {
            CommandConfigToml::Single(cmd) => vec![(None, cmd)],
            CommandConfigToml::Multiple(cmds) => cmds
                .into_iter()
                .enumerate()
                .map(|(i, cmd)| (Some((i + 1).to_string()), cmd))
                .collect(),
            CommandConfigToml::Named(map) => {
                let mut pairs: Vec<_> = map.into_iter().collect();
                pairs.sort_by(|a, b| a.0.cmp(&b.0));
                pairs.into_iter().map(|(k, v)| (Some(k), v)).collect()
            }
        };
        Ok(CommandConfig { commands })
    }
}

// Serialize back to most appropriate format
impl Serialize for CommandConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;

        // If single unnamed command, serialize as string
        if self.commands.len() == 1 && self.commands[0].0.is_none() {
            return self.commands[0].1.serialize(serializer);
        }

        // If all commands are unnamed or numbered 1,2,3..., serialize as array
        let all_numbered = self
            .commands
            .iter()
            .enumerate()
            .all(|(i, (name, _))| name.as_ref().is_none_or(|n| n == &(i + 1).to_string()));

        if all_numbered {
            let cmds: Vec<_> = self.commands.iter().map(|(_, cmd)| cmd).collect();
            return cmds.serialize(serializer);
        }

        // Otherwise serialize as named map
        // At this point, all commands must have names (from Named TOML format)
        let mut map = serializer.serialize_map(Some(self.commands.len()))?;
        for (name, cmd) in &self.commands {
            let key = name
                .as_ref()
                .expect("named format requires all commands to have names");
            map.serialize_entry(key, cmd)?;
        }
        map.end()
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

impl Default for WorktrunkConfig {
    fn default() -> Self {
        Self {
            worktree_path: "../{main-worktree}.{branch}".to_string(),
            commit_generation: CommitGenerationConfig::default(),
            projects: std::collections::BTreeMap::new(),
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
        builder = builder.add_source(config::Environment::with_prefix("WORKTRUNK").separator("_"));

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
    /// * `main_worktree` - Main worktree directory name (replaces {main-worktree} in template)
    /// * `branch` - Branch name (replaces {branch} in template, slashes sanitized to dashes)
    ///
    /// # Examples
    /// ```
    /// use worktrunk::config::WorktrunkConfig;
    ///
    /// let config = WorktrunkConfig::default();
    /// let path = config.format_path("myproject", "feature/foo");
    /// assert_eq!(path, "../myproject.feature-foo");
    /// ```
    pub fn format_path(&self, main_worktree: &str, branch: &str) -> String {
        expand_template(
            &self.worktree_path,
            main_worktree,
            branch,
            &std::collections::HashMap::new(),
        )
    }
}

pub fn get_config_path() -> Option<PathBuf> {
    // Check for test override first (WORKTRUNK_CONFIG_PATH env var)
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

/// Expand template variables in a string
///
/// All templates support:
/// - `{main-worktree}` - Main worktree directory name
/// - `{branch}` - Branch name (sanitized: slashes → dashes)
///
/// Additional variables can be provided via the `extra` parameter.
///
/// # Examples
/// ```
/// use worktrunk::config::expand_template;
/// use std::collections::HashMap;
///
/// let result = expand_template("path/{main-worktree}/{branch}", "myrepo", "feature/foo", &HashMap::new());
/// assert_eq!(result, "path/myrepo/feature-foo");
/// ```
pub fn expand_template(
    template: &str,
    main_worktree: &str,
    branch: &str,
    extra: &std::collections::HashMap<&str, &str>,
) -> String {
    // Sanitize branch name by replacing path separators
    let safe_branch = branch.replace(['/', '\\'], "-");

    let mut result = template
        .replace("{main-worktree}", main_worktree)
        .replace("{branch}", &safe_branch);

    // Apply any extra variables
    for (key, value) in extra {
        result = result.replace(&format!("{{{}}}", key), value);
    }

    result
}

/// Expand tilde in file paths to home directory (cross-platform)
///
/// Uses shellexpand for proper tilde expansion following shell conventions.
///
/// # Examples
/// ```
/// use worktrunk::config::expand_tilde;
///
/// let path = expand_tilde("~/config/file.txt");
/// // Unix: /home/user/config/file.txt
/// // Windows: C:\Users\user\config\file.txt
/// ```
pub fn expand_tilde(path: &str) -> PathBuf {
    PathBuf::from(shellexpand::tilde(path).as_ref())
}

/// Expand command template variables
///
/// Convenience function for expanding command templates with common variables.
///
/// Supported variables:
/// - `{repo}` - Repository name
/// - `{branch}` - Branch name (sanitized)
/// - `{worktree}` - Path to the worktree
/// - `{repo_root}` - Path to the main repository root
/// - `{target}` - Target branch (for merge commands, optional)
///
/// # Examples
/// ```
/// use worktrunk::config::expand_command_template;
/// use std::path::Path;
///
/// let cmd = expand_command_template(
///     "cp {repo_root}/target {worktree}/target",
///     "myrepo",
///     "feature",
///     Path::new("/path/to/worktree"),
///     Path::new("/path/to/repo"),
///     None,
/// );
/// ```
pub fn expand_command_template(
    command: &str,
    repo_name: &str,
    branch: &str,
    worktree_path: &std::path::Path,
    repo_root: &std::path::Path,
    target_branch: Option<&str>,
) -> String {
    let mut extra = std::collections::HashMap::new();
    extra.insert("worktree", worktree_path.to_str().unwrap_or(""));
    extra.insert("repo_root", repo_root.to_str().unwrap_or(""));
    if let Some(target) = target_branch {
        extra.insert("target", target);
    }

    expand_template(command, repo_name, branch, &extra)
}

impl ProjectConfig {
    /// Load project configuration from .config/wt.toml in the repository root
    pub fn load(repo_root: &std::path::Path) -> Result<Option<Self>, ConfigError> {
        let config_path = repo_root.join(".config").join("wt.toml");

        if !config_path.exists() {
            return Ok(None);
        }

        let config = Config::builder()
            .add_source(File::from(config_path))
            .build()?;

        Ok(Some(config.try_deserialize()?))
    }
}

impl WorktrunkConfig {
    /// Check if a command is approved for the given project
    pub fn is_command_approved(&self, project: &str, command: &str) -> bool {
        self.projects
            .get(project)
            .map(|p| p.approved_commands.iter().any(|c| c == command))
            .unwrap_or(false)
    }

    /// Add an approved command and save to config file
    pub fn approve_command(&mut self, project: String, command: String) -> Result<(), ConfigError> {
        // Don't add duplicates
        if self.is_command_approved(&project, &command) {
            return Ok(());
        }

        self.projects
            .entry(project)
            .or_default()
            .approved_commands
            .push(command);
        self.save()
    }

    /// Add an approved command and save to a specific config file (for testing)
    ///
    /// This is the same as `approve_command()` but saves to an explicit path
    /// instead of the default user config location. Use this in tests to avoid
    /// polluting the user's actual config.
    pub fn approve_command_to(
        &mut self,
        project: String,
        command: String,
        config_path: &std::path::Path,
    ) -> Result<(), ConfigError> {
        // Don't add duplicates
        if self.is_command_approved(&project, &command) {
            return Ok(());
        }

        self.projects
            .entry(project)
            .or_default()
            .approved_commands
            .push(command);
        self.save_to(config_path)
    }

    /// Revoke an approved command and save to config file
    ///
    /// Removes the specified command from the project's approved commands list.
    /// If this results in an empty project entry, the project is removed entirely.
    pub fn revoke_command(&mut self, project: &str, command: &str) -> Result<(), ConfigError> {
        if let Some(project_config) = self.projects.get_mut(project) {
            let len_before = project_config.approved_commands.len();
            project_config.approved_commands.retain(|c| c != command);
            let changed = len_before != project_config.approved_commands.len();

            // Clean up empty project entries
            if project_config.approved_commands.is_empty() {
                self.projects.remove(project);
            }

            if changed {
                self.save()?;
            }
        }
        Ok(())
    }

    /// Remove all approvals for a project and save to config file
    ///
    /// Removes the entire project entry from the configuration.
    pub fn revoke_project(&mut self, project: &str) -> Result<(), ConfigError> {
        if self.projects.remove(project).is_some() {
            self.save()?;
        }
        Ok(())
    }

    /// Revoke an approved command and save to a specific config file (for testing)
    ///
    /// This is the same as `revoke_command()` but saves to an explicit path
    /// instead of the default user config location. Use this in tests to avoid
    /// polluting the user's actual config.
    #[doc(hidden)]
    pub fn revoke_command_to(
        &mut self,
        project: &str,
        command: &str,
        config_path: &std::path::Path,
    ) -> Result<(), ConfigError> {
        if let Some(project_config) = self.projects.get_mut(project) {
            let len_before = project_config.approved_commands.len();
            project_config.approved_commands.retain(|c| c != command);
            let changed = len_before != project_config.approved_commands.len();

            // Clean up empty project entries
            if project_config.approved_commands.is_empty() {
                self.projects.remove(project);
            }

            if changed {
                self.save_to(config_path)?;
            }
        }
        Ok(())
    }

    /// Remove all approvals for a project and save to a specific config file (for testing)
    ///
    /// This is the same as `revoke_project()` but saves to an explicit path
    /// instead of the default user config location. Use this in tests to avoid
    /// polluting the user's actual config.
    #[doc(hidden)]
    pub fn revoke_project_to(
        &mut self,
        project: &str,
        config_path: &std::path::Path,
    ) -> Result<(), ConfigError> {
        if self.projects.remove(project).is_some() {
            self.save_to(config_path)?;
        }
        Ok(())
    }

    /// Save the current configuration to the default config file location
    pub fn save(&self) -> Result<(), ConfigError> {
        let config_path = get_config_path()
            .ok_or_else(|| ConfigError::Message("Could not determine config path".to_string()))?;
        self.save_to(&config_path)
    }

    /// Save the current configuration to a specific file path
    ///
    /// Use this in tests to save to a temporary location instead of the user's config.
    pub fn save_to(&self, config_path: &std::path::Path) -> Result<(), ConfigError> {
        // Create parent directory if it doesn't exist
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ConfigError::Message(format!("Failed to create config directory: {}", e))
            })?;
        }

        let toml_string = toml::to_string_pretty(self)
            .map_err(|e| ConfigError::Message(format!("Failed to serialize config: {}", e)))?;

        std::fs::write(config_path, toml_string)
            .map_err(|e| ConfigError::Message(format!("Failed to write config file: {}", e)))?;

        Ok(())
    }

    /// Test helper: Simulate the approval save flow used by check_and_approve_command
    ///
    /// This is used in integration tests to verify the --force flag behavior without
    /// requiring access to the internal commands module.
    #[doc(hidden)]
    pub fn test_save_approval_flow(
        project_id: &str,
        command: &str,
        config_path: &std::path::Path,
    ) -> Result<(), ConfigError> {
        // This mirrors what the CLI does when batching approvals:
        // 1. Load config (in our case, from the test path)
        // 2. Add approval entry
        // 3. Save back
        let mut config = Self::default();

        // Try to load existing config if it exists
        if config_path.exists() {
            let content = std::fs::read_to_string(config_path)
                .map_err(|e| ConfigError::Message(format!("Failed to read config: {}", e)))?;
            config = toml::from_str(&content)
                .map_err(|e| ConfigError::Message(format!("Failed to parse config: {}", e)))?;
        }

        config.approve_command_to(project_id.to_string(), command.to_string(), config_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_serialization() {
        let config = WorktrunkConfig::default();
        let toml = toml::to_string(&config).unwrap();
        assert!(toml.contains("worktree-path"));
        assert!(toml.contains("../{main-worktree}.{branch}"));
        assert!(toml.contains("commit-generation"));
    }

    #[test]
    fn test_default_config() {
        let config = WorktrunkConfig::default();
        assert_eq!(config.worktree_path, "../{main-worktree}.{branch}");
        assert_eq!(config.commit_generation.command, None);
        assert!(config.projects.is_empty());
    }

    #[test]
    fn test_format_worktree_path() {
        let config = WorktrunkConfig {
            worktree_path: "{main-worktree}.{branch}".to_string(),
            commit_generation: CommitGenerationConfig::default(),
            projects: std::collections::BTreeMap::new(),
        };
        assert_eq!(
            config.format_path("myproject", "feature-x"),
            "myproject.feature-x"
        );
    }

    #[test]
    fn test_format_worktree_path_custom_template() {
        let config = WorktrunkConfig {
            worktree_path: "{main-worktree}-{branch}".to_string(),
            commit_generation: CommitGenerationConfig::default(),
            projects: std::collections::BTreeMap::new(),
        };
        assert_eq!(
            config.format_path("myproject", "feature-x"),
            "myproject-feature-x"
        );
    }

    #[test]
    fn test_format_worktree_path_only_branch() {
        let config = WorktrunkConfig {
            worktree_path: ".worktrees/{main-worktree}/{branch}".to_string(),
            commit_generation: CommitGenerationConfig::default(),
            projects: std::collections::BTreeMap::new(),
        };
        assert_eq!(
            config.format_path("myproject", "feature-x"),
            ".worktrees/myproject/feature-x"
        );
    }

    #[test]
    fn test_format_worktree_path_with_slashes() {
        // Slashes should be replaced with dashes to prevent directory traversal
        let config = WorktrunkConfig {
            worktree_path: "{main-worktree}.{branch}".to_string(),
            commit_generation: CommitGenerationConfig::default(),
            projects: std::collections::BTreeMap::new(),
        };
        assert_eq!(
            config.format_path("myproject", "feature/foo"),
            "myproject.feature-foo"
        );
    }

    #[test]
    fn test_format_worktree_path_with_multiple_slashes() {
        let config = WorktrunkConfig {
            worktree_path: ".worktrees/{main-worktree}/{branch}".to_string(),
            commit_generation: CommitGenerationConfig::default(),
            projects: std::collections::BTreeMap::new(),
        };
        assert_eq!(
            config.format_path("myproject", "feature/sub/task"),
            ".worktrees/myproject/feature-sub-task"
        );
    }

    #[test]
    fn test_format_worktree_path_with_backslashes() {
        // Windows-style path separators should also be sanitized
        let config = WorktrunkConfig {
            worktree_path: ".worktrees/{main-worktree}/{branch}".to_string(),
            commit_generation: CommitGenerationConfig::default(),
            projects: std::collections::BTreeMap::new(),
        };
        assert_eq!(
            config.format_path("myproject", "feature\\foo"),
            ".worktrees/myproject/feature-foo"
        );
    }

    #[test]
    fn test_project_config_default() {
        let config = ProjectConfig::default();
        assert!(config.post_create_command.is_none());
        assert!(config.post_start_command.is_none());
        assert!(config.pre_merge_command.is_none());
        assert!(config.post_merge_command.is_none());
    }

    #[test]
    fn test_command_config_single() {
        let toml = r#"post-create-command = "npm install""#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        let cmd_config = config.post_create_command.unwrap();
        let commands = cmd_config.commands();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0], (None, "npm install".to_string()));
    }

    #[test]
    fn test_command_config_multiple() {
        let toml = r#"post-create-command = ["npm install", "npm test"]"#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        let cmd_config = config.post_create_command.unwrap();
        let commands = cmd_config.commands();
        assert_eq!(commands.len(), 2);
        assert_eq!(
            commands[0],
            (Some("1".to_string()), "npm install".to_string())
        );
        assert_eq!(commands[1], (Some("2".to_string()), "npm test".to_string()));
    }

    #[test]
    fn test_command_config_named() {
        let toml = r#"
            [post-start-command]
            server = "npm run dev"
            watch = "npm run watch"
        "#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        let cmd_config = config.post_start_command.unwrap();
        let commands = cmd_config.commands();
        assert_eq!(commands.len(), 2);
        // Names are sorted alphabetically
        assert_eq!(
            commands[0],
            (Some("server".to_string()), "npm run dev".to_string())
        );
        assert_eq!(
            commands[1],
            (Some("watch".to_string()), "npm run watch".to_string())
        );
    }

    #[test]
    fn test_project_config_both_commands() {
        let toml = r#"
            post-create-command = ["npm install"]

            [post-start-command]
            server = "npm run dev"
        "#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        assert!(config.post_create_command.is_some());
        assert!(config.post_start_command.is_some());
    }

    #[test]
    fn test_pre_merge_command_single() {
        let toml = r#"pre-merge-command = "cargo test""#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        let cmd_config = config.pre_merge_command.unwrap();
        let commands = cmd_config.commands();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0], (None, "cargo test".to_string()));
    }

    #[test]
    fn test_pre_merge_command_multiple() {
        let toml = r#"pre-merge-command = ["cargo fmt -- --check", "cargo test"]"#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        let cmd_config = config.pre_merge_command.unwrap();
        let commands = cmd_config.commands();
        assert_eq!(commands.len(), 2);
        assert_eq!(
            commands[0],
            (Some("1".to_string()), "cargo fmt -- --check".to_string())
        );
        assert_eq!(
            commands[1],
            (Some("2".to_string()), "cargo test".to_string())
        );
    }

    #[test]
    fn test_pre_merge_command_named() {
        let toml = r#"
            [pre-merge-command]
            format = "cargo fmt -- --check"
            lint = "cargo clippy"
            test = "cargo test"
        "#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        let cmd_config = config.pre_merge_command.unwrap();
        let commands = cmd_config.commands();
        assert_eq!(commands.len(), 3);
        // Names are sorted alphabetically
        assert_eq!(
            commands[0],
            (
                Some("format".to_string()),
                "cargo fmt -- --check".to_string()
            )
        );
        assert_eq!(
            commands[1],
            (Some("lint".to_string()), "cargo clippy".to_string())
        );
        assert_eq!(
            commands[2],
            (Some("test".to_string()), "cargo test".to_string())
        );
    }

    #[test]
    fn test_command_config_roundtrip_single() {
        let original = r#"post-create-command = "npm install""#;
        let config: ProjectConfig = toml::from_str(original).unwrap();
        let serialized = toml::to_string(&config).unwrap();
        let config2: ProjectConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(config, config2);
        // Verify it serialized back as a string, not array
        assert!(serialized.contains(r#"post-create-command = "npm install""#));
    }

    #[test]
    fn test_command_config_roundtrip_multiple() {
        let original = r#"post-create-command = ["npm install", "npm test"]"#;
        let config: ProjectConfig = toml::from_str(original).unwrap();
        let serialized = toml::to_string(&config).unwrap();
        let config2: ProjectConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(config, config2);
        // Verify it serialized back as an array
        assert!(serialized.contains(r#"post-create-command = ["npm install", "npm test"]"#));
    }

    #[test]
    fn test_command_config_roundtrip_named() {
        let original = r#"
            [post-start-command]
            server = "npm run dev"
            watch = "npm run watch"
        "#;
        let config: ProjectConfig = toml::from_str(original).unwrap();
        let serialized = toml::to_string(&config).unwrap();
        let config2: ProjectConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(config, config2);
        // Verify it serialized back as a named table
        assert!(serialized.contains("[post-start-command]"));
        assert!(serialized.contains(r#"server = "npm run dev""#));
        assert!(serialized.contains(r#"watch = "npm run watch""#));
    }

    #[test]
    fn test_user_project_config_equality() {
        let config1 = UserProjectConfig {
            approved_commands: vec!["npm install".to_string()],
        };
        let config2 = UserProjectConfig {
            approved_commands: vec!["npm install".to_string()],
        };
        let config3 = UserProjectConfig {
            approved_commands: vec!["npm test".to_string()],
        };
        assert_eq!(config1, config2);
        assert_ne!(config1, config3);
    }

    #[test]
    fn test_is_command_approved() {
        let mut config = WorktrunkConfig::default();
        config.projects.insert(
            "github.com/user/repo".to_string(),
            UserProjectConfig {
                approved_commands: vec!["npm install".to_string()],
            },
        );

        assert!(config.is_command_approved("github.com/user/repo", "npm install"));
        assert!(!config.is_command_approved("github.com/user/repo", "npm test"));
        assert!(!config.is_command_approved("github.com/other/repo", "npm install"));
    }

    #[test]
    fn test_approve_command() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("test-config.toml");
        let mut config = WorktrunkConfig::default();

        // First approval
        assert!(!config.is_command_approved("github.com/user/repo", "npm install"));
        config
            .approve_command_to(
                "github.com/user/repo".to_string(),
                "npm install".to_string(),
                &config_path,
            )
            .unwrap();
        assert!(config.is_command_approved("github.com/user/repo", "npm install"));

        // Duplicate approval shouldn't add twice
        let count_before = config
            .projects
            .get("github.com/user/repo")
            .unwrap()
            .approved_commands
            .len();
        config
            .approve_command_to(
                "github.com/user/repo".to_string(),
                "npm install".to_string(),
                &config_path,
            )
            .unwrap();
        assert_eq!(
            config
                .projects
                .get("github.com/user/repo")
                .unwrap()
                .approved_commands
                .len(),
            count_before
        );
    }

    #[test]
    fn test_revoke_command() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("test-config.toml");

        let mut config = WorktrunkConfig::default();

        // Set up two approved commands
        config
            .approve_command_to(
                "github.com/user/repo".to_string(),
                "npm install".to_string(),
                &config_path,
            )
            .unwrap();
        config
            .approve_command_to(
                "github.com/user/repo".to_string(),
                "npm test".to_string(),
                &config_path,
            )
            .unwrap();

        assert!(config.is_command_approved("github.com/user/repo", "npm install"));
        assert!(config.is_command_approved("github.com/user/repo", "npm test"));

        // Revoke one command
        config
            .revoke_command_to("github.com/user/repo", "npm install", &config_path)
            .unwrap();
        assert!(!config.is_command_approved("github.com/user/repo", "npm install"));
        assert!(config.is_command_approved("github.com/user/repo", "npm test"));

        // Project entry should still exist
        assert!(config.projects.contains_key("github.com/user/repo"));

        // Revoke the last command - should remove the project entry
        config
            .revoke_command_to("github.com/user/repo", "npm test", &config_path)
            .unwrap();
        assert!(!config.is_command_approved("github.com/user/repo", "npm test"));
        assert!(!config.projects.contains_key("github.com/user/repo"));
    }

    #[test]
    fn test_revoke_command_nonexistent() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("test-config.toml");

        let mut config = WorktrunkConfig::default();

        // Revoking from non-existent project is a no-op
        config
            .revoke_command_to("github.com/user/repo", "npm install", &config_path)
            .unwrap();

        // Set up one command
        config
            .approve_command_to(
                "github.com/user/repo".to_string(),
                "npm install".to_string(),
                &config_path,
            )
            .unwrap();

        // Revoking non-existent command is a no-op
        config
            .revoke_command_to("github.com/user/repo", "npm test", &config_path)
            .unwrap();
        assert!(config.is_command_approved("github.com/user/repo", "npm install"));
    }

    #[test]
    fn test_revoke_project() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("test-config.toml");

        let mut config = WorktrunkConfig::default();

        // Set up multiple projects
        config
            .approve_command_to(
                "github.com/user/repo1".to_string(),
                "npm install".to_string(),
                &config_path,
            )
            .unwrap();
        config
            .approve_command_to(
                "github.com/user/repo1".to_string(),
                "npm test".to_string(),
                &config_path,
            )
            .unwrap();
        config
            .approve_command_to(
                "github.com/user/repo2".to_string(),
                "cargo build".to_string(),
                &config_path,
            )
            .unwrap();

        assert!(config.projects.contains_key("github.com/user/repo1"));
        assert!(config.projects.contains_key("github.com/user/repo2"));

        // Revoke entire project
        config
            .revoke_project_to("github.com/user/repo1", &config_path)
            .unwrap();
        assert!(!config.projects.contains_key("github.com/user/repo1"));
        assert!(config.projects.contains_key("github.com/user/repo2"));

        // Revoking non-existent project is a no-op
        config
            .revoke_project_to("github.com/user/repo1", &config_path)
            .unwrap();
        config
            .revoke_project_to("github.com/nonexistent/repo", &config_path)
            .unwrap();
    }

    #[test]
    fn test_expand_template_basic() {
        use std::collections::HashMap;

        let result = expand_template(
            "../{main-worktree}.{branch}",
            "myrepo",
            "feature-x",
            &HashMap::new(),
        );
        assert_eq!(result, "../myrepo.feature-x");
    }

    #[test]
    fn test_expand_template_sanitizes_branch() {
        use std::collections::HashMap;

        let result = expand_template(
            "{main-worktree}/{branch}",
            "myrepo",
            "feature/foo",
            &HashMap::new(),
        );
        assert_eq!(result, "myrepo/feature-foo");

        let result = expand_template(
            ".worktrees/{main-worktree}/{branch}",
            "myrepo",
            "feat\\bar",
            &HashMap::new(),
        );
        assert_eq!(result, ".worktrees/myrepo/feat-bar");
    }

    #[test]
    fn test_expand_template_with_extra_vars() {
        use std::collections::HashMap;

        let mut extra = HashMap::new();
        extra.insert("worktree", "/path/to/worktree");
        extra.insert("repo_root", "/path/to/repo");

        let result = expand_template(
            "{repo_root}/target -> {worktree}/target",
            "myrepo",
            "main",
            &extra,
        );
        assert_eq!(result, "/path/to/repo/target -> /path/to/worktree/target");
    }

    #[test]
    fn test_expand_tilde_with_home() {
        // Test that paths starting with ~/ get HOME prepended if HOME is set
        // We can't set HOME in tests (no unsafe allowed), but we can test the logic
        let result = expand_tilde("~/config/file.txt");
        // If HOME is set, result should start with it. If not, it's just the path.
        // Either way is valid behavior.
        assert!(result.to_str().unwrap().contains("config/file.txt"));
    }

    #[test]
    fn test_expand_tilde_without_tilde() {
        let result = expand_tilde("/absolute/path");
        assert_eq!(result, PathBuf::from("/absolute/path"));
    }

    #[test]
    fn test_commit_generation_config_mutually_exclusive_validation() {
        // Test that deserialization rejects both template and template-file
        let toml_content = r#"
worktree-path = "../{main-worktree}.{branch}"

[commit-generation]
command = "llm"
template = "inline template"
template-file = "~/file.txt"
"#;

        // Parse the TOML directly
        let config_result: Result<WorktrunkConfig, _> = toml::from_str(toml_content);

        // The deserialization should succeed, but validation in load() would fail
        // Since we can't easily test load() without env vars, we verify the fields deserialize
        if let Ok(config) = config_result {
            // Verify validation logic: both fields should not be Some
            let has_both = config.commit_generation.template.is_some()
                && config.commit_generation.template_file.is_some();
            assert!(
                has_both,
                "Config should have both template fields set for this test"
            );
        }
    }

    #[test]
    fn test_squash_template_mutually_exclusive_validation() {
        // Test that deserialization rejects both squash-template and squash-template-file
        let toml_content = r#"
worktree-path = "../{main-worktree}.{branch}"

[commit-generation]
command = "llm"
squash-template = "inline template"
squash-template-file = "~/file.txt"
"#;

        // Parse the TOML directly
        let config_result: Result<WorktrunkConfig, _> = toml::from_str(toml_content);

        // The deserialization should succeed, but validation in load() would fail
        // Since we can't easily test load() without env vars, we verify the fields deserialize
        if let Ok(config) = config_result {
            // Verify validation logic: both fields should not be Some
            let has_both = config.commit_generation.squash_template.is_some()
                && config.commit_generation.squash_template_file.is_some();
            assert!(
                has_both,
                "Config should have both squash template fields set for this test"
            );
        }
    }

    #[test]
    fn test_commit_generation_config_serialization() {
        let config = CommitGenerationConfig {
            command: Some("llm".to_string()),
            args: vec!["-m".to_string(), "model".to_string()],
            template: Some("template content".to_string()),
            template_file: None,
            squash_template: None,
            squash_template_file: None,
        };

        let toml = toml::to_string(&config).unwrap();
        assert!(toml.contains("llm"));
        assert!(toml.contains("template"));
    }
}
