//! User-level configuration
//!
//! Personal preferences and per-project approved commands, not checked into git.

mod accessors;
mod merge;
mod mutation;
mod path;
mod persistence;
mod resolved;
mod schema;
mod sections;
#[cfg(test)]
mod tests;

use config::{Case, Config, ConfigError, File};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// Re-export public types
pub use merge::Merge;
pub use path::{get_config_path, set_config_path};
pub use resolved::ResolvedConfig;
pub use schema::{find_unknown_keys, valid_user_config_keys};
pub use sections::{
    CommitConfig, CommitGenerationConfig, ListConfig, MergeConfig, OverridableConfig, SelectConfig,
    StageMode, UserProjectOverrides,
};

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

    /// Skip the first-run commit generation prompt
    #[serde(
        default,
        rename = "skip-commit-generation-prompt",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub skip_commit_generation_prompt: bool,
}

impl UserConfig {
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
            // Use show_brief_warning=true to emit a brief pointer to `wt config show`
            // Warning is deduplicated per-process via WARNED_DEPRECATED_PATHS.
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
        } else if let Some(config_path) = config_path.as_ref()
            && path::is_config_path_explicit()
        {
            // Warn if user explicitly specified a config path that doesn't exist
            crate::styling::eprintln!(
                "{}",
                crate::styling::warning_message(format!(
                    "Config file not found: {}",
                    config_path.display()
                ))
            );
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

    /// Load configuration from a TOML string for testing.
    #[cfg(test)]
    pub(crate) fn load_from_str(content: &str) -> Result<Self, ConfigError> {
        let config: Self =
            toml::from_str(content).map_err(|e| ConfigError::Message(e.to_string()))?;
        config.validate()?;
        Ok(config)
    }
}
