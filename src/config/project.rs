//! Project-level configuration
//!
//! Configuration that is checked into the repository and shared across all developers.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::ConfigError;
use super::commands::CommandConfig;
use super::is_default;
use super::{CopyIgnoredConfig, HooksConfig, StepConfig};

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
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq, JsonSchema)]
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
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq, JsonSchema)]
pub struct ProjectCiConfig {
    /// CI platform override. When set, skips URL-based platform detection.
    ///
    /// Values: "github" or "gitlab"
    #[serde(default)]
    pub platform: Option<String>,
}

/// Project-level forge configuration.
///
/// Override forge detection when URL-based detection fails (e.g., SSH host
/// aliases, GitHub Enterprise, or self-hosted GitLab with custom domains).
///
/// # Example
///
/// ```toml
/// [forge]
/// platform = "github"              # or "gitlab"
/// hostname = "github.example.com"  # API hostname for GHE / self-hosted GitLab
/// ```
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq, JsonSchema)]
pub struct ProjectForgeConfig {
    /// Forge platform override. When set, skips URL-based platform detection.
    ///
    /// Values: "github" or "gitlab"
    #[serde(default)]
    pub platform: Option<String>,

    /// API hostname for GitHub Enterprise or self-hosted GitLab.
    ///
    /// Only needed when the remote URL uses an SSH host alias that doesn't
    /// resolve to the real API hostname. For standard github.com/gitlab.com
    /// setups, this is not needed.
    #[serde(default)]
    pub hostname: Option<String>,
}

impl ProjectListConfig {
    /// Returns true if any list configuration is set.
    pub fn is_configured(&self) -> bool {
        self.url.is_some()
    }
}

impl ProjectConfig {
    /// Get the CI platform override if configured.
    ///
    /// Deprecated: use [`forge_platform()`](Self::forge_platform) instead.
    pub fn ci_platform(&self) -> Option<&str> {
        self.ci.platform.as_deref()
    }

    /// Get the forge platform override, checking `[forge]` first then `[ci]`.
    pub fn forge_platform(&self) -> Option<&str> {
        self.forge
            .platform
            .as_deref()
            .or_else(|| self.ci_platform())
    }

    /// Get the forge API hostname if configured.
    pub fn forge_hostname(&self) -> Option<&str> {
        self.forge.hostname.as_deref()
    }

    /// Get `wt step copy-ignored` configuration if configured.
    pub fn copy_ignored(&self) -> Option<&CopyIgnoredConfig> {
        self.step.copy_ignored.as_ref()
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
/// - `{{ primary_worktree_path }}` - Primary worktree path (main worktree for normal repos; default branch worktree for bare repos)
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
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq, JsonSchema)]
pub struct ProjectConfig {
    /// Project hooks (same keys as user hooks, flattened at top level)
    #[serde(flatten, default)]
    pub hooks: HooksConfig,

    /// Configuration for `wt list` output
    #[serde(default, skip_serializing_if = "is_default")]
    pub list: ProjectListConfig,

    /// CI configuration (platform override). Deprecated: use `[forge]` instead.
    #[serde(default, skip_serializing_if = "is_default")]
    pub ci: ProjectCiConfig,

    /// Forge configuration (platform detection override, API hostname)
    #[serde(default, skip_serializing_if = "is_default")]
    pub forge: ProjectForgeConfig,

    /// Configuration for `wt step` subcommands.
    #[serde(default, skip_serializing_if = "is_default")]
    pub step: StepConfig,

    /// \[experimental\] Command aliases for `wt step <name>`.
    ///
    /// Each alias maps a name to a [`CommandConfig`] — a string for a single
    /// command, a named table (`[aliases.NAME]`) for concurrent commands, or
    /// `[[aliases.NAME]]` blocks for sequential pipeline steps. All hook
    /// template variables are available (e.g., `{{ branch }}`,
    /// `{{ worktree_path }}`).
    ///
    /// ```toml
    /// [aliases]
    /// deploy = "cd {{ worktree_path }} && make deploy"
    /// lint = "npm run lint"
    /// ```
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub aliases: BTreeMap<String, CommandConfig>,
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
        let config_path = match repo
            .project_config_path()
            .map_err(|e| ConfigError(format!("Failed to get config path: {}", e)))?
        {
            Some(path) if path.exists() => path,
            _ => return Ok(None),
        };

        // Load directly with toml crate to preserve insertion order (with preserve_order feature)
        let contents = std::fs::read_to_string(&config_path)
            .map_err(|e| ConfigError(format!("Failed to read config file: {}", e)))?;

        // Check for deprecated template variables and create migration file if needed
        // Only write migration file in main worktree, not linked worktrees
        // Use show_brief_warning=true to emit a brief pointer to `wt config show`
        let is_main_worktree = !repo.current_worktree().is_linked().unwrap_or(true);
        let repo_for_hints = if write_hints { Some(repo) } else { None };
        let _ = super::deprecation::check_and_migrate(
            &config_path,
            &contents,
            is_main_worktree,
            "Project config",
            repo_for_hints,
            true, // show_brief_warning
        );

        // Warn about unknown fields (only in main worktree where it's actionable)
        if is_main_worktree {
            super::deprecation::warn_unknown_fields::<ProjectConfig>(
                &config_path,
                &find_unknown_keys(&contents),
                "Project config",
            );
        }

        let config: ProjectConfig = toml::from_str(&contents).map_err(|e| {
            ConfigError(format!(
                "Project config at {} failed to parse:\n{e}",
                crate::path::format_path_for_display(&config_path),
            ))
        })?;

        Ok(Some(config))
    }
}

/// Returns all valid top-level keys in project config, derived from the JsonSchema.
///
/// This includes keys from ProjectConfig and HooksConfig (flattened).
/// Public for use by the `WorktrunkConfig` trait implementation.
pub fn valid_project_config_keys() -> Vec<String> {
    use schemars::SchemaGenerator;

    let schema = SchemaGenerator::default().into_root_schema_for::<ProjectConfig>();

    schema
        .as_object()
        .and_then(|obj| obj.get("properties"))
        .and_then(|p| p.as_object())
        .map(|props| props.keys().cloned().collect())
        .unwrap_or_default()
}

/// Find unknown keys in project config TOML content
///
/// Returns a map of unrecognized top-level keys (with their values) that will be ignored.
/// Compares against the known valid keys derived from the JsonSchema.
/// The values are included to allow checking if keys belong in the other config type.
pub fn find_unknown_keys(contents: &str) -> std::collections::HashMap<String, toml::Value> {
    let Ok(table) = contents.parse::<toml::Table>() else {
        return std::collections::HashMap::new();
    };

    let valid_keys = valid_project_config_keys();

    table
        .into_iter()
        .filter(|(key, _)| !valid_keys.contains(key))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(
            config.list.url.as_deref(),
            Some("http://localhost:{{ branch | hash_port }}")
        );
        assert!(config.list.is_configured());
    }

    #[test]
    fn test_deserialize_list_empty() {
        let contents = r#"
[list]
"#;
        let config: ProjectConfig = toml::from_str(contents).unwrap();
        assert!(config.list.url.is_none());
        assert!(!config.list.is_configured());
    }

    #[test]
    fn test_deserialize_step_copy_ignored() {
        let contents = r#"
[step.copy-ignored]
exclude = [".conductor/", ".entire/"]
"#;
        let config: ProjectConfig = toml::from_str(contents).unwrap();
        assert_eq!(
            config.copy_ignored().unwrap().exclude,
            vec![".conductor/".to_string(), ".entire/".to_string()]
        );
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
        assert_eq!(config.ci.platform.as_deref(), Some("github"));
    }

    #[test]
    fn test_deserialize_ci_platform_gitlab() {
        let contents = r#"
[ci]
platform = "gitlab"
"#;
        let config: ProjectConfig = toml::from_str(contents).unwrap();
        assert_eq!(config.ci.platform.as_deref(), Some("gitlab"));
    }

    #[test]
    fn test_deserialize_ci_empty() {
        let contents = r#"
[ci]
"#;
        let config: ProjectConfig = toml::from_str(contents).unwrap();
        assert!(config.ci.platform.is_none());
    }

    #[test]
    fn test_ci_config_default() {
        let config = ProjectCiConfig::default();
        assert!(config.platform.is_none());
    }

    // ============================================================================
    // ForgeConfig Tests
    // ============================================================================

    #[test]
    fn test_deserialize_forge_platform() {
        let contents = r#"
[forge]
platform = "github"
"#;
        let config: ProjectConfig = toml::from_str(contents).unwrap();
        assert_eq!(config.forge_platform(), Some("github"));
        assert!(config.forge_hostname().is_none());
    }

    #[test]
    fn test_deserialize_forge_hostname() {
        let contents = r#"
[forge]
platform = "github"
hostname = "github.example.com"
"#;
        let config: ProjectConfig = toml::from_str(contents).unwrap();
        assert_eq!(config.forge_platform(), Some("github"));
        assert_eq!(config.forge_hostname(), Some("github.example.com"));
    }

    #[test]
    fn test_forge_platform_falls_back_to_ci() {
        let contents = r#"
[ci]
platform = "gitlab"
"#;
        let config: ProjectConfig = toml::from_str(contents).unwrap();
        // forge.platform not set, falls back to ci.platform
        assert_eq!(config.forge_platform(), Some("gitlab"));
    }

    #[test]
    fn test_forge_platform_takes_precedence_over_ci() {
        let contents = r#"
[ci]
platform = "gitlab"

[forge]
platform = "github"
"#;
        let config: ProjectConfig = toml::from_str(contents).unwrap();
        // forge.platform takes precedence
        assert_eq!(config.forge_platform(), Some("github"));
        // ci.platform still accessible directly
        assert_eq!(config.ci_platform(), Some("gitlab"));
    }

    #[test]
    fn test_forge_config_default() {
        let config = ProjectForgeConfig::default();
        assert!(config.platform.is_none());
        assert!(config.hostname.is_none());
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

[step.copy-ignored]
exclude = [".conductor/"]
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
        assert!(keys.contains_key("unknown-key"));
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
        assert!(keys.contains_key("foo"));
        assert!(keys.contains_key("baz"));
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
