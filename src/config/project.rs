//! Project-level configuration
//!
//! Configuration that is checked into the repository and shared across all developers.

use config::ConfigError;
use serde::{Deserialize, Serialize};

use super::commands::CommandConfig;

/// Project-specific configuration with hooks.
///
/// This config is stored at `<repo>/.config/wt.toml` within the repository and
/// IS checked into git. It defines project-specific hooks that run automatically
/// during worktree operations. All developers working on the project share this config.
///
/// # Template Variables
///
/// All hooks support these template variables:
/// - `{{ repo }}` - Repository name (e.g., "my-project")
/// - `{{ branch }}` - Branch name (e.g., "feature-foo")
/// - `{{ worktree }}` - Absolute path to the worktree
/// - `{{ worktree_name }}` - Worktree directory name (e.g., "my-project.feature-foo")
/// - `{{ repo_root }}` - Absolute path to the repository root
/// - `{{ default_branch }}` - Default branch name (e.g., "main")
/// - `{{ commit }}` - Current HEAD commit SHA (full 40-character hash)
/// - `{{ short_commit }}` - Current HEAD commit SHA (short 7-character hash)
/// - `{{ remote }}` - Primary remote name (e.g., "origin")
/// - `{{ upstream }}` - Upstream tracking branch (e.g., "origin/feature"), if configured
///
/// Merge-related hooks (`pre-commit`, `pre-merge`, `post-merge`) also support:
/// - `{{ target }}` - Target branch for the merge (e.g., "main")
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct ProjectConfig {
    /// Commands to execute sequentially before worktree is ready (blocking)
    /// Supports string (single command) or table (named, sequential)
    ///
    /// Available template variables: `{{ repo }}`, `{{ branch }}`, `{{ worktree }}`, `{{ worktree_name }}`, `{{ repo_root }}`, `{{ default_branch }}`, `{{ commit }}`, `{{ short_commit }}`, `{{ remote }}`, `{{ upstream }}`
    #[serde(default, rename = "post-create")]
    pub post_create: Option<CommandConfig>,

    /// Commands to execute in parallel as background processes (non-blocking)
    /// Supports string (single command) or table (named, parallel)
    ///
    /// Available template variables: `{{ repo }}`, `{{ branch }}`, `{{ worktree }}`, `{{ worktree_name }}`, `{{ repo_root }}`, `{{ default_branch }}`, `{{ commit }}`, `{{ short_commit }}`, `{{ remote }}`, `{{ upstream }}`
    #[serde(default, rename = "post-start")]
    pub post_start: Option<CommandConfig>,

    /// Commands to execute before committing changes during merge (blocking, fail-fast validation)
    /// Supports string (single command) or table (named, sequential)
    /// All commands must exit with code 0 for commit to proceed
    /// Runs before any commit operation during `wt merge` (both squash and no-squash modes)
    ///
    /// Available template variables: `{{ repo }}`, `{{ branch }}`, `{{ worktree }}`, `{{ worktree_name }}`, `{{ repo_root }}`, `{{ default_branch }}`, `{{ commit }}`, `{{ short_commit }}`, `{{ remote }}`, `{{ upstream }}`, `{{ target }}`
    #[serde(default, rename = "pre-commit")]
    pub pre_commit: Option<CommandConfig>,

    /// Commands to execute before merging (blocking, fail-fast validation)
    /// Supports string (single command) or table (named, sequential)
    /// All commands must exit with code 0 for merge to proceed
    ///
    /// Available template variables: `{{ repo }}`, `{{ branch }}`, `{{ worktree }}`, `{{ worktree_name }}`, `{{ repo_root }}`, `{{ default_branch }}`, `{{ commit }}`, `{{ short_commit }}`, `{{ remote }}`, `{{ upstream }}`, `{{ target }}`
    #[serde(default, rename = "pre-merge")]
    pub pre_merge: Option<CommandConfig>,

    /// Commands to execute after successful merge in the main worktree (blocking)
    /// Supports string (single command) or table (named, sequential)
    /// Runs after push succeeds but before cleanup
    ///
    /// Available template variables: `{{ repo }}`, `{{ branch }}`, `{{ worktree }}`, `{{ worktree_name }}`, `{{ repo_root }}`, `{{ default_branch }}`, `{{ commit }}`, `{{ short_commit }}`, `{{ remote }}`, `{{ upstream }}`, `{{ target }}`
    #[serde(default, rename = "post-merge")]
    pub post_merge: Option<CommandConfig>,

    /// Commands to execute before a worktree is removed (blocking)
    /// Supports string (single command) or table (named, sequential)
    /// Runs in the worktree before removal; non-zero exit aborts removal
    ///
    /// Available template variables: `{{ repo }}`, `{{ branch }}`, `{{ worktree }}`, `{{ worktree_name }}`, `{{ repo_root }}`, `{{ default_branch }}`, `{{ commit }}`, `{{ short_commit }}`, `{{ remote }}`, `{{ upstream }}`
    #[serde(default, rename = "pre-remove")]
    pub pre_remove: Option<CommandConfig>,

    /// Captures unknown fields for validation warnings
    #[serde(flatten, default, skip_serializing)]
    unknown: std::collections::HashMap<String, toml::Value>,
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

/// Find unknown keys in project config TOML content
///
/// Returns a list of unrecognized top-level keys that will be silently ignored.
/// Uses serde deserialization with flatten to automatically detect unknown fields.
pub fn find_unknown_keys(contents: &str) -> Vec<String> {
    // Deserialize into ProjectConfig - unknown fields are captured in the `unknown` map
    let Ok(config) = toml::from_str::<ProjectConfig>(contents) else {
        return vec![];
    };

    config.unknown.into_keys().collect()
}
