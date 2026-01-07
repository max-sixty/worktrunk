//! Project-level configuration
//!
//! Configuration that is checked into the repository and shared across all developers.

use config::ConfigError;
use serde::{Deserialize, Serialize};

use super::HooksConfig;

/// Project-level configuration for `wt list` output.
///
/// This is distinct from user-level `ListConfig` which controls CLI defaults.
/// Project-level config is for project-specific features like dev server URLs.
///
/// # Example
///
/// ```toml
/// [list]
/// url = "http://localhost:{{ branch | hash_port }}"
/// ```
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct ProjectListConfig {
    /// URL template for dev server links shown in `wt list`.
    ///
    /// Available variable: `{{ branch }}` (the branch name).
    /// Available filters: `{{ branch | hash_port }}` (deterministic port 10000-19999),
    /// `{{ branch | sanitize }}` (filesystem-safe name).
    ///
    /// The URL is displayed with health-check styling: dim if the port is not
    /// listening, normal if it is.
    #[serde(default)]
    pub url: Option<String>,
}

/// Project-level CI configuration.
///
/// Override CI platform detection when URL-based detection fails (e.g., GitHub
/// Enterprise or self-hosted GitLab with custom domains).
///
/// # Example
///
/// ```toml
/// [ci]
/// platform = "github"  # or "gitlab"
/// ```
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct ProjectCiConfig {
    /// CI platform override. When set, skips URL-based platform detection.
    ///
    /// Values: "github" or "gitlab"
    #[serde(default)]
    pub platform: Option<String>,
}

impl ProjectListConfig {
    /// Returns true if any list configuration is set.
    pub fn is_configured(&self) -> bool {
        self.url.is_some()
    }
}

impl ProjectConfig {
    /// Get the CI platform override if configured.
    pub fn ci_platform(&self) -> Option<&str> {
        self.ci.as_ref().and_then(|ci| ci.platform.as_deref())
    }
}

/// Project-specific configuration with hooks.
///
/// This config is stored at `<repo>/.config/wt.toml` within the repository and
/// IS checked into git. It defines project-specific hooks that run automatically
/// during worktree operations. All developers working on the project share this config.
///
/// # Template Variables
///
/// All hooks support these template variables:
/// - `{{ repo }}` - Repository directory name (e.g., "myproject")
/// - `{{ repo_path }}` - Absolute path to repository root (e.g., "/path/to/myproject")
/// - `{{ branch }}` - Branch name (e.g., "feature/auth")
/// - `{{ worktree_name }}` - Worktree directory name (e.g., "myproject.feature-auth")
/// - `{{ worktree_path }}` - Absolute path to the worktree (e.g., "/path/to/myproject.feature-auth")
/// - `{{ main_worktree_path }}` - Absolute path to main worktree (e.g., "/path/to/myproject")
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
/// - `{{ branch | sanitize }}` - Replace `/` and `\` with `-` (e.g., "feature-auth")
/// - `{{ branch | hash_port }}` - Hash string to deterministic port (10000-19999)
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct ProjectConfig {
    /// Project hooks (same keys as user hooks, flattened at top level)
    #[serde(flatten, default)]
    pub hooks: HooksConfig,

    /// Configuration for `wt list` output
    #[serde(default)]
    pub list: Option<ProjectListConfig>,

    /// CI configuration (platform override)
    #[serde(default)]
    pub ci: Option<ProjectCiConfig>,

    /// Captures unknown fields for validation warnings
    #[serde(flatten, default, skip_serializing)]
    unknown: std::collections::HashMap<String, toml::Value>,
}

impl ProjectConfig {
    /// Load project configuration from .config/wt.toml in the repository root
    ///
    /// Set `write_hints` to true for normal usage. Set to false during completion
    /// to avoid side effects (writing git config hints).
    pub fn load(
        repo: &crate::git::Repository,
        write_hints: bool,
    ) -> Result<Option<Self>, ConfigError> {
        let repo_root = repo
            .worktree_root()
            .map_err(|e| ConfigError::Message(format!("Failed to get worktree root: {}", e)))?;
        let config_path = repo_root.join(".config").join("wt.toml");

        if !config_path.exists() {
            return Ok(None);
        }

        // Load directly with toml crate to preserve insertion order (with preserve_order feature)
        let contents = std::fs::read_to_string(&config_path)
            .map_err(|e| ConfigError::Message(format!("Failed to read config file: {}", e)))?;

        // Check for deprecated template variables and create migration file if needed
        // Only write migration file in main worktree (where .git is a directory)
        // Linked worktrees have .git as a file pointing to the main worktree
        let is_main_worktree = repo_root.join(".git").is_dir();
        let repo_for_hints = if write_hints { Some(repo) } else { None };
        let _ = super::deprecation::check_and_migrate(
            &config_path,
            &contents,
            is_main_worktree,
            "Project config",
            repo_for_hints,
        );

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
        assert!(config.hooks.post_create.is_none());
        assert!(config.hooks.post_start.is_none());
        assert!(config.hooks.post_switch.is_none());
        assert!(config.hooks.pre_commit.is_none());
        assert!(config.hooks.pre_merge.is_none());
        assert!(config.hooks.post_merge.is_none());
        assert!(config.hooks.pre_remove.is_none());
        assert!(config.list.is_none());
        assert!(config.ci.is_none());
    }

    // ============================================================================
    // Deserialization Tests
    // ============================================================================

    #[test]
    fn test_deserialize_empty_config() {
        let contents = "";
        let config: ProjectConfig = toml::from_str(contents).unwrap();
        assert!(config.hooks.post_create.is_none());
        assert!(config.hooks.pre_merge.is_none());
    }

    #[test]
    fn test_deserialize_post_create_string() {
        let contents = r#"post-create = "npm install""#;
        let config: ProjectConfig = toml::from_str(contents).unwrap();
        assert!(config.hooks.post_create.is_some());
    }

    #[test]
    fn test_deserialize_post_start_table() {
        let contents = r#"
[post-start]
build = "cargo build"
test = "cargo test"
"#;
        let config: ProjectConfig = toml::from_str(contents).unwrap();
        assert!(config.hooks.post_start.is_some());
    }

    #[test]
    fn test_deserialize_pre_merge() {
        let contents = r#"pre-merge = "cargo test""#;
        let config: ProjectConfig = toml::from_str(contents).unwrap();
        assert!(config.hooks.pre_merge.is_some());
    }

    #[test]
    fn test_deserialize_post_merge() {
        let contents = r#"post-merge = "git push origin main""#;
        let config: ProjectConfig = toml::from_str(contents).unwrap();
        assert!(config.hooks.post_merge.is_some());
    }

    #[test]
    fn test_deserialize_pre_remove() {
        let contents = r#"pre-remove = "echo cleaning up""#;
        let config: ProjectConfig = toml::from_str(contents).unwrap();
        assert!(config.hooks.pre_remove.is_some());
    }

    #[test]
    fn test_deserialize_pre_commit() {
        let contents = r#"pre-commit = "cargo fmt --check""#;
        let config: ProjectConfig = toml::from_str(contents).unwrap();
        assert!(config.hooks.pre_commit.is_some());
    }

    #[test]
    fn test_deserialize_all_hooks() {
        let contents = r#"
post-create = "npm install"
post-start = "npm run watch"
post-switch = "rename-tab"
pre-commit = "cargo fmt --check"
pre-merge = "cargo test"
post-merge = "git push"
pre-remove = "echo bye"
"#;
        let config: ProjectConfig = toml::from_str(contents).unwrap();
        assert!(config.hooks.post_create.is_some());
        assert!(config.hooks.post_start.is_some());
        assert!(config.hooks.post_switch.is_some());
        assert!(config.hooks.pre_commit.is_some());
        assert!(config.hooks.pre_merge.is_some());
        assert!(config.hooks.post_merge.is_some());
        assert!(config.hooks.pre_remove.is_some());
    }

    // ============================================================================
    // ListConfig Tests
    // ============================================================================

    #[test]
    fn test_deserialize_list_url() {
        let contents = r#"
[list]
url = "http://localhost:{{ branch | hash_port }}"
"#;
        let config: ProjectConfig = toml::from_str(contents).unwrap();
        assert!(config.list.is_some());
        let list = config.list.unwrap();
        assert_eq!(
            list.url.as_deref(),
            Some("http://localhost:{{ branch | hash_port }}")
        );
        assert!(list.is_configured());
    }

    #[test]
    fn test_deserialize_list_empty() {
        let contents = r#"
[list]
"#;
        let config: ProjectConfig = toml::from_str(contents).unwrap();
        assert!(config.list.is_some());
        let list = config.list.unwrap();
        assert!(list.url.is_none());
        assert!(!list.is_configured());
    }

    #[test]
    fn test_list_config_default() {
        let config = ProjectListConfig::default();
        assert!(config.url.is_none());
        assert!(!config.is_configured());
    }

    // ============================================================================
    // CiConfig Tests
    // ============================================================================

    #[test]
    fn test_deserialize_ci_platform_github() {
        let contents = r#"
[ci]
platform = "github"
"#;
        let config: ProjectConfig = toml::from_str(contents).unwrap();
        assert!(config.ci.is_some());
        let ci = config.ci.unwrap();
        assert_eq!(ci.platform.as_deref(), Some("github"));
    }

    #[test]
    fn test_deserialize_ci_platform_gitlab() {
        let contents = r#"
[ci]
platform = "gitlab"
"#;
        let config: ProjectConfig = toml::from_str(contents).unwrap();
        assert!(config.ci.is_some());
        let ci = config.ci.unwrap();
        assert_eq!(ci.platform.as_deref(), Some("gitlab"));
    }

    #[test]
    fn test_deserialize_ci_empty() {
        let contents = r#"
[ci]
"#;
        let config: ProjectConfig = toml::from_str(contents).unwrap();
        assert!(config.ci.is_some());
        let ci = config.ci.unwrap();
        assert!(ci.platform.is_none());
    }

    #[test]
    fn test_ci_config_default() {
        let config = ProjectCiConfig::default();
        assert!(config.platform.is_none());
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
