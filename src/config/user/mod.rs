//! User-level configuration
//!
//! Personal preferences and per-project approved commands, not checked into git.

mod accessors;
mod merge;
pub(crate) mod mutation;
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
pub use path::{
    config_path, default_config_path, default_system_config_path, set_config_path,
    system_config_path,
};
pub use resolved::ResolvedConfig;
pub use schema::{find_unknown_keys, valid_user_config_keys};
pub use sections::{
    CommitConfig, CommitGenerationConfig, CopyIgnoredConfig, ListConfig, MergeConfig,
    OverridableConfig, SelectConfig, StageMode, StepConfig, SwitchConfig, SwitchPickerConfig,
    UserProjectOverrides,
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

    /// Settings that can be overridden per-project (worktree-path, list, commit, merge, switch, step, select, hooks)
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
    fn normalize_deprecated_sections(&mut self) {
        Self::normalize_commit_generation_section(
            &mut self.commit_generation,
            &mut self.configs.commit,
        );
        Self::normalize_select_section(&mut self.configs.select, &mut self.configs.switch);
        if let Some(merge) = &mut self.configs.merge {
            merge.normalize_deprecated_fields();
        }
        if let Some(switch) = &mut self.configs.switch {
            switch.normalize_deprecated_fields();
        }

        for project in self.projects.values_mut() {
            Self::normalize_commit_generation_section(
                &mut project.commit_generation,
                &mut project.overrides.commit,
            );
            Self::normalize_select_section(
                &mut project.overrides.select,
                &mut project.overrides.switch,
            );
            if let Some(merge) = &mut project.overrides.merge {
                merge.normalize_deprecated_fields();
            }
            if let Some(switch) = &mut project.overrides.switch {
                switch.normalize_deprecated_fields();
            }
        }
    }

    fn normalize_commit_generation_section(
        deprecated: &mut Option<CommitGenerationConfig>,
        commit: &mut Option<CommitConfig>,
    ) {
        let Some(deprecated_config) = deprecated.take() else {
            return;
        };

        if deprecated_config == CommitGenerationConfig::default() {
            return;
        }

        let commit_config = commit.get_or_insert_with(CommitConfig::default);
        if commit_config.generation.is_none() {
            commit_config.generation = Some(deprecated_config);
        }
    }

    fn normalize_select_section(
        deprecated: &mut Option<SelectConfig>,
        switch: &mut Option<SwitchConfig>,
    ) {
        let Some(select_config) = deprecated.take() else {
            return;
        };

        if select_config == SelectConfig::default() {
            return;
        }

        let switch_config = switch.get_or_insert_with(SwitchConfig::default);
        if switch_config.picker.is_none() {
            switch_config.picker = Some(SwitchPickerConfig {
                pager: select_config.pager,
                timeout_ms: None,
            });
        }
    }

    /// Load configuration from system config, user config, and environment variables.
    ///
    /// Configuration is loaded in the following order (later sources override earlier ones):
    /// 1. Default values
    /// 2. System config (organization-wide defaults)
    /// 3. User config file (personal preferences)
    /// 4. Environment variables (WORKTRUNK_*)
    pub fn load() -> Result<Self, ConfigError> {
        // Note: worktree-path has no default set here - it's handled by the getter
        // which returns the default when None. This allows us to distinguish
        // "user explicitly set this" from "using default".
        let mut builder = Config::builder();

        // Add system config if it exists (lowest priority file source)
        if let Some(system_path) = path::system_config_path() {
            if let Ok(content) = std::fs::read_to_string(&system_path) {
                // Warn about unknown fields in system config
                let unknown_keys: std::collections::HashMap<_, _> = find_unknown_keys(&content)
                    .into_iter()
                    .filter(|(k, _)| {
                        !super::deprecation::DEPRECATED_SECTION_KEYS.contains(&k.as_str())
                    })
                    .collect();
                super::deprecation::warn_unknown_fields::<UserConfig>(
                    &system_path,
                    &unknown_keys,
                    "System config",
                );
            }
            builder = builder.add_source(File::from(system_path));
        }

        // Add user config file if it exists (overrides system config)
        let config_path = config_path();
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
                    crate::path::format_path_for_display(config_path)
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
        let mut config: Self = builder.build()?.try_deserialize()?;
        config.normalize_deprecated_sections();
        config.validate()?;

        Ok(config)
    }

    /// Load configuration from a TOML string for testing.
    #[cfg(test)]
    pub(crate) fn load_from_str(content: &str) -> Result<Self, ConfigError> {
        let mut config: Self =
            toml::from_str(content).map_err(|e| ConfigError::Message(e.to_string()))?;
        config.normalize_deprecated_sections();
        config.validate()?;
        Ok(config)
    }
}
