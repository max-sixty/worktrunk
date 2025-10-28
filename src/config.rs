use config::{Config, ConfigError, File};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use toml;

#[cfg(not(test))]
use etcetera::base_strategy::{BaseStrategy, choose_base_strategy};

/// Configuration for worktree path formatting and LLM integration.
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

    /// Commands that have been approved for automatic execution
    #[serde(default, rename = "approved-commands")]
    pub approved_commands: Vec<ApprovedCommand>,
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
}

/// Project-specific configuration (stored in .config/wt.toml within the project)
///
/// # Template Variables
///
/// Commands support template variable expansion:
/// - `{repo}` - Repository name (e.g., "my-project")
/// - `{branch}` - Branch name (e.g., "feature-foo")
/// - `{worktree}` - Absolute path to the worktree
/// - `{repo_root}` - Absolute path to the repository root
///
/// Additionally, `pre-merge-check` commands support:
/// - `{target}` - Target branch for the merge (e.g., "main")
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
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

    /// Commands to execute before merging (blocking, fail-fast validation)
    /// Supports string (single command), array (sequential), or table (named, sequential)
    /// All commands must exit with code 0 for merge to proceed
    ///
    /// Available template variables: `{repo}`, `{branch}`, `{worktree}`, `{repo_root}`, `{target}`
    #[serde(default, rename = "pre-merge-check")]
    pub pre_merge_check: Option<CommandConfig>,

    /// Commands to execute after successful merge in the main worktree (blocking)
    /// Supports string (single command), array (sequential), or table (named, sequential)
    /// Runs after push succeeds but before cleanup
    ///
    /// Available template variables: `{repo}`, `{branch}`, `{worktree}`, `{repo_root}`, `{target}`
    #[serde(default, rename = "post-merge-command")]
    pub post_merge_command: Option<CommandConfig>,
}

/// Configuration for commands - supports multiple formats
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CommandConfig {
    /// Single command as a string
    Single(String),
    /// Multiple commands as an array
    Multiple(Vec<String>),
    /// Named commands as a table (map)
    Named(std::collections::HashMap<String, String>),
}

/// Approved command for automatic execution
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ApprovedCommand {
    /// Project identifier (git remote URL or repo name)
    pub project: String,
    /// Command that was approved
    pub command: String,
}

impl Default for WorktrunkConfig {
    fn default() -> Self {
        Self {
            worktree_path: "../{main-worktree}.{branch}".to_string(),
            commit_generation: CommitGenerationConfig::default(),
            approved_commands: Vec::new(),
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

fn get_config_path() -> Option<PathBuf> {
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
        self.approved_commands
            .iter()
            .any(|ac| ac.project == project && ac.command == command)
    }

    /// Add an approved command and save to config file
    pub fn approve_command(&mut self, project: String, command: String) -> Result<(), ConfigError> {
        // Don't add duplicates
        if self.is_command_approved(&project, &command) {
            return Ok(());
        }

        self.approved_commands
            .push(ApprovedCommand { project, command });
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

        self.approved_commands
            .push(ApprovedCommand { project, command });
        self.save_to(config_path)
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
        assert!(config.approved_commands.is_empty());
    }

    #[test]
    fn test_format_worktree_path() {
        let config = WorktrunkConfig {
            worktree_path: "{main-worktree}.{branch}".to_string(),
            commit_generation: CommitGenerationConfig::default(),
            approved_commands: Vec::new(),
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
            approved_commands: Vec::new(),
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
            approved_commands: Vec::new(),
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
            approved_commands: Vec::new(),
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
            approved_commands: Vec::new(),
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
            approved_commands: Vec::new(),
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
        assert!(config.pre_merge_check.is_none());
        assert!(config.post_merge_command.is_none());
    }

    #[test]
    fn test_command_config_single() {
        let toml = r#"post-create-command = "npm install""#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        assert!(matches!(
            config.post_create_command,
            Some(CommandConfig::Single(_))
        ));
    }

    #[test]
    fn test_command_config_multiple() {
        let toml = r#"post-create-command = ["npm install", "npm test"]"#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        match config.post_create_command {
            Some(CommandConfig::Multiple(cmds)) => {
                assert_eq!(cmds.len(), 2);
                assert_eq!(cmds[0], "npm install");
                assert_eq!(cmds[1], "npm test");
            }
            _ => panic!("Expected Multiple variant"),
        }
    }

    #[test]
    fn test_command_config_named() {
        let toml = r#"
            [post-start-command]
            server = "npm run dev"
            watch = "npm run watch"
        "#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        match config.post_start_command {
            Some(CommandConfig::Named(cmds)) => {
                assert_eq!(cmds.len(), 2);
                assert_eq!(cmds.get("server"), Some(&"npm run dev".to_string()));
                assert_eq!(cmds.get("watch"), Some(&"npm run watch".to_string()));
            }
            _ => panic!("Expected Named variant"),
        }
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
    fn test_pre_merge_check_single() {
        let toml = r#"pre-merge-check = "cargo test""#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        assert!(matches!(
            config.pre_merge_check,
            Some(CommandConfig::Single(_))
        ));
    }

    #[test]
    fn test_pre_merge_check_multiple() {
        let toml = r#"pre-merge-check = ["cargo fmt -- --check", "cargo test"]"#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        match config.pre_merge_check {
            Some(CommandConfig::Multiple(cmds)) => {
                assert_eq!(cmds.len(), 2);
                assert_eq!(cmds[0], "cargo fmt -- --check");
                assert_eq!(cmds[1], "cargo test");
            }
            _ => panic!("Expected Multiple variant"),
        }
    }

    #[test]
    fn test_pre_merge_check_named() {
        let toml = r#"
            [pre-merge-check]
            format = "cargo fmt -- --check"
            lint = "cargo clippy"
            test = "cargo test"
        "#;
        let config: ProjectConfig = toml::from_str(toml).unwrap();
        match config.pre_merge_check {
            Some(CommandConfig::Named(cmds)) => {
                assert_eq!(cmds.len(), 3);
                assert_eq!(
                    cmds.get("format"),
                    Some(&"cargo fmt -- --check".to_string())
                );
                assert_eq!(cmds.get("lint"), Some(&"cargo clippy".to_string()));
                assert_eq!(cmds.get("test"), Some(&"cargo test".to_string()));
            }
            _ => panic!("Expected Named variant"),
        }
    }

    #[test]
    fn test_approved_command_equality() {
        let cmd1 = ApprovedCommand {
            project: "github.com/user/repo".to_string(),
            command: "npm install".to_string(),
        };
        let cmd2 = ApprovedCommand {
            project: "github.com/user/repo".to_string(),
            command: "npm install".to_string(),
        };
        let cmd3 = ApprovedCommand {
            project: "github.com/user/repo".to_string(),
            command: "npm test".to_string(),
        };
        assert_eq!(cmd1, cmd2);
        assert_ne!(cmd1, cmd3);
    }

    #[test]
    fn test_is_command_approved() {
        let mut config = WorktrunkConfig::default();
        config.approved_commands.push(ApprovedCommand {
            project: "github.com/user/repo".to_string(),
            command: "npm install".to_string(),
        });

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
        let count_before = config.approved_commands.len();
        config
            .approve_command_to(
                "github.com/user/repo".to_string(),
                "npm install".to_string(),
                &config_path,
            )
            .unwrap();
        assert_eq!(config.approved_commands.len(), count_before);
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
}
