//! Project-level configuration
//!
//! Configuration that is checked into the repository and shared across all developers.

use config::ConfigError;
use serde::{Deserialize, Serialize};

use super::commands::CommandConfig;

/// Project-specific configuration with hooks and commands.
///
/// This config is stored at `<repo>/.config/wt.toml` within the repository and
/// IS checked into git. It defines project-specific commands that run automatically
/// during worktree operations. All developers working on the project share this config.
///
/// # Template Variables
///
/// All commands support these template variables:
/// - `{{ repo }}` - Repository name (e.g., "my-project")
/// - `{{ branch }}` - Branch name (e.g., "feature-foo")
/// - `{{ worktree }}` - Absolute path to the worktree
/// - `{{ repo_root }}` - Absolute path to the repository root
///
/// Merge-related commands (`pre-commit-command`, `pre-merge-command`, `post-merge-command`) also support:
/// - `{{ target }}` - Target branch for the merge (e.g., "main")
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct ProjectConfig {
    /// Commands to execute sequentially before worktree is ready (blocking)
    /// Supports string (single command), array (sequential), or table (named, sequential)
    ///
    /// Available template variables: `{{ repo }}`, `{{ branch }}`, `{{ worktree }}`, `{{ repo_root }}`
    #[serde(default, rename = "post-create-command")]
    pub post_create_command: Option<CommandConfig>,

    /// Commands to execute in parallel as background processes (non-blocking)
    /// Supports string (single), array (parallel), or table (named, parallel)
    ///
    /// Available template variables: `{{ repo }}`, `{{ branch }}`, `{{ worktree }}`, `{{ repo_root }}`
    #[serde(default, rename = "post-start-command")]
    pub post_start_command: Option<CommandConfig>,

    /// Commands to execute before committing changes during merge (blocking, fail-fast validation)
    /// Supports string (single command), array (sequential), or table (named, sequential)
    /// All commands must exit with code 0 for commit to proceed
    /// Runs before any commit operation during `wt merge` (both squash and no-squash modes)
    ///
    /// Available template variables: `{{ repo }}`, `{{ branch }}`, `{{ worktree }}`, `{{ repo_root }}`, `{{ target }}`
    #[serde(default, rename = "pre-commit-command")]
    pub pre_commit_command: Option<CommandConfig>,

    /// Commands to execute before merging (blocking, fail-fast validation)
    /// Supports string (single command), array (sequential), or table (named, sequential)
    /// All commands must exit with code 0 for merge to proceed
    ///
    /// Available template variables: `{{ repo }}`, `{{ branch }}`, `{{ worktree }}`, `{{ repo_root }}`, `{{ target }}`
    #[serde(default, rename = "pre-merge-command")]
    pub pre_merge_command: Option<CommandConfig>,

    /// Commands to execute after successful merge in the main worktree (blocking)
    /// Supports string (single command), array (sequential), or table (named, sequential)
    /// Runs after push succeeds but before cleanup
    ///
    /// Available template variables: `{{ repo }}`, `{{ branch }}`, `{{ worktree }}`, `{{ repo_root }}`, `{{ target }}`
    #[serde(default, rename = "post-merge-command")]
    pub post_merge_command: Option<CommandConfig>,
}

impl ProjectConfig {
    /// Load project configuration from .config/wt.toml in the repository root
    pub fn load(repo_root: &std::path::Path) -> Result<Option<Self>, ConfigError> {
        let config_path = repo_root.join(".config").join("wt.toml");

        if !config_path.exists() {
            return Ok(None);
        }

        // Load directly with toml crate to preserve insertion order (with preserve_order feature)
        let contents = std::fs::read_to_string(&config_path)
            .map_err(|e| ConfigError::Message(format!("Failed to read config file: {}", e)))?;

        let config: ProjectConfig = toml::from_str(&contents)
            .map_err(|e| ConfigError::Message(format!("Failed to parse TOML: {}", e)))?;

        Ok(Some(config))
    }
}
