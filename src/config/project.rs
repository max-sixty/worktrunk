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
///
/// # Filters
///
/// - `{{ branch | sanitize }}` - Replace `/` and `\` with `-` (e.g., "feature-foo")
/// - `{{ branch | hash_port }}` - Hash string to deterministic port (10000-19999)
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
    /// Runs after push and cleanup complete
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

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================================
    // ProjectConfig Default Tests
    // ============================================================================

    #[test]
    fn test_project_config_default() {
        let config = ProjectConfig::default();
        assert!(config.post_create.is_none());
        assert!(config.post_start.is_none());
        assert!(config.pre_commit.is_none());
        assert!(config.pre_merge.is_none());
        assert!(config.post_merge.is_none());
        assert!(config.pre_remove.is_none());
    }

    // ============================================================================
    // Deserialization Tests
    // ============================================================================

    #[test]
    fn test_deserialize_empty_config() {
        let contents = "";
        let config: ProjectConfig = toml::from_str(contents).unwrap();
        assert!(config.post_create.is_none());
        assert!(config.pre_merge.is_none());
    }

    #[test]
    fn test_deserialize_post_create_string() {
        let contents = r#"post-create = "npm install""#;
        let config: ProjectConfig = toml::from_str(contents).unwrap();
        assert!(config.post_create.is_some());
    }

    #[test]
    fn test_deserialize_post_start_table() {
        let contents = r#"
[post-start]
build = "cargo build"
test = "cargo test"
"#;
        let config: ProjectConfig = toml::from_str(contents).unwrap();
        assert!(config.post_start.is_some());
    }

    #[test]
    fn test_deserialize_pre_merge() {
        let contents = r#"pre-merge = "cargo test""#;
        let config: ProjectConfig = toml::from_str(contents).unwrap();
        assert!(config.pre_merge.is_some());
    }

    #[test]
    fn test_deserialize_post_merge() {
        let contents = r#"post-merge = "git push origin main""#;
        let config: ProjectConfig = toml::from_str(contents).unwrap();
        assert!(config.post_merge.is_some());
    }

    #[test]
    fn test_deserialize_pre_remove() {
        let contents = r#"pre-remove = "echo cleaning up""#;
        let config: ProjectConfig = toml::from_str(contents).unwrap();
        assert!(config.pre_remove.is_some());
    }

    #[test]
    fn test_deserialize_pre_commit() {
        let contents = r#"pre-commit = "cargo fmt --check""#;
        let config: ProjectConfig = toml::from_str(contents).unwrap();
        assert!(config.pre_commit.is_some());
    }

    #[test]
    fn test_deserialize_all_hooks() {
        let contents = r#"
post-create = "npm install"
post-start = "npm run watch"
pre-commit = "cargo fmt --check"
pre-merge = "cargo test"
post-merge = "git push"
pre-remove = "echo bye"
"#;
        let config: ProjectConfig = toml::from_str(contents).unwrap();
        assert!(config.post_create.is_some());
        assert!(config.post_start.is_some());
        assert!(config.pre_commit.is_some());
        assert!(config.pre_merge.is_some());
        assert!(config.post_merge.is_some());
        assert!(config.pre_remove.is_some());
    }

    // ============================================================================
    // find_unknown_keys Tests
    // ============================================================================

    #[test]
    fn test_find_unknown_keys_empty() {
        let contents = "";
        let keys = find_unknown_keys(contents);
        assert!(keys.is_empty());
    }

    #[test]
    fn test_find_unknown_keys_all_known() {
        let contents = r#"
post-create = "npm install"
pre-merge = "cargo test"
"#;
        let keys = find_unknown_keys(contents);
        assert!(keys.is_empty());
    }

    #[test]
    fn test_find_unknown_keys_unknown_key() {
        let contents = r#"
post-create = "npm install"
unknown-key = "value"
"#;
        let keys = find_unknown_keys(contents);
        assert_eq!(keys.len(), 1);
        assert!(keys.contains(&"unknown-key".to_string()));
    }

    #[test]
    fn test_find_unknown_keys_multiple_unknown() {
        let contents = r#"
foo = "bar"
baz = "qux"
post-create = "npm install"
"#;
        let keys = find_unknown_keys(contents);
        assert_eq!(keys.len(), 2);
        assert!(keys.contains(&"foo".to_string()));
        assert!(keys.contains(&"baz".to_string()));
    }

    #[test]
    fn test_find_unknown_keys_invalid_toml() {
        let contents = "invalid { toml }}}";
        let keys = find_unknown_keys(contents);
        // Returns empty vec for invalid TOML (graceful fallback)
        assert!(keys.is_empty());
    }

    // ============================================================================
    // Serialization Tests
    // ============================================================================

    #[test]
    fn test_serialize_empty_config() {
        let config = ProjectConfig::default();
        let serialized = toml::to_string(&config).unwrap();
        // Default config should serialize to empty or minimal string
        assert!(serialized.is_empty() || serialized.trim().is_empty());
    }

    #[test]
    fn test_config_equality() {
        let config1 = ProjectConfig::default();
        let config2 = ProjectConfig::default();
        assert_eq!(config1, config2);
    }

    #[test]
    fn test_config_clone() {
        let contents = r#"pre-merge = "cargo test""#;
        let config: ProjectConfig = toml::from_str(contents).unwrap();
        let cloned = config.clone();
        assert_eq!(config, cloned);
    }
}
