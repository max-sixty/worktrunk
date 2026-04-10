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

use std::path::PathBuf;

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
    OverridableConfig, StageMode, StepConfig, SwitchConfig, SwitchPickerConfig,
    UserProjectOverrides,
};

/// Distinguishes *why* `UserConfig::load()` failed so callers can emit
/// targeted diagnostics (file errors with line/col vs env-var attribution).
#[derive(Debug)]
pub enum LoadError {
    /// A config file failed to parse. The `toml::de::Error` includes
    /// line/column info and a source-snippet pointer.
    File {
        path: PathBuf,
        label: &'static str,
        err: Box<toml::de::Error>,
    },
    /// Config files parsed cleanly; applying env-var overrides failed.
    /// `override_vars` lists `WORKTRUNK_*` env vars that could be the cause.
    Env {
        err: ConfigError,
        override_vars: Vec<String>,
    },
    /// Other errors (validation, config-crate internals).
    Other(ConfigError),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::File { path, label, err } => {
                write!(
                    f,
                    "{label} at {} failed to parse:\n{err}",
                    crate::path::format_path_for_display(path)
                )
            }
            LoadError::Env { err, .. } => write!(f, "{err}"),
            LoadError::Other(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for LoadError {}

/// Returns names of `WORKTRUNK_*` env vars that could be value overrides,
/// excluding infrastructure paths and the `WORKTRUNK_TEST_` namespace.
// TODO: This hardcoded exclusion list is a smell — ideally the config loading
// layer would track which source each value came from, making attribution
// automatic rather than heuristic. Consider integrating this into the config
// crate's source-tracking or building our own env-var overlay.
fn collect_worktrunk_override_vars() -> Vec<String> {
    const INFRA_VARS: &[&str] = &[
        "WORKTRUNK_CONFIG_PATH",
        "WORKTRUNK_SYSTEM_CONFIG_PATH",
        "WORKTRUNK_APPROVALS_PATH",
    ];
    let mut vars: Vec<String> = std::env::vars()
        .filter_map(|(k, _)| {
            if !k.starts_with("WORKTRUNK_") {
                return None;
            }
            if INFRA_VARS.contains(&k.as_str()) || k.starts_with("WORKTRUNK_TEST_") {
                return None;
            }
            Some(k)
        })
        .collect();
    vars.sort();
    vars
}

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
    /// Per-project configuration (approved commands, etc.)
    /// Uses BTreeMap for deterministic serialization order and better diff readability
    #[serde(default)]
    pub projects: std::collections::BTreeMap<String, UserProjectOverrides>,

    /// Settings that can be overridden per-project (worktree-path, list, commit, merge, switch, step, hooks)
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
    /// Load configuration from system config, user config, and environment variables.
    ///
    /// Configuration is loaded in the following order (later sources override earlier ones):
    /// 1. Default values
    /// 2. System config (organization-wide defaults)
    /// 3. User config file (personal preferences)
    /// 4. Environment variables (WORKTRUNK_*)
    pub fn load() -> Result<Self, ConfigError> {
        Self::load_with_cause().map_err(|e| ConfigError::Message(e.to_string()))
    }

    /// Like [`load()`](Self::load), but returns a [`LoadError`] that
    /// distinguishes file-level parse failures (with line/col) from
    /// env-var override failures. Used by `Repository::user_config()`
    /// to emit targeted diagnostics.
    pub(crate) fn load_with_cause() -> Result<Self, LoadError> {
        // Note: worktree-path has no default set here - it's handled by the getter
        // which returns the default when None. This allows us to distinguish
        // "user explicitly set this" from "using default".
        let mut builder = Config::builder();

        // Add system config if it exists (lowest priority file source)
        if let Some(system_path) = path::system_config_path()
            && let Ok(content) = std::fs::read_to_string(&system_path)
        {
            // Warn about unknown fields in system config
            super::deprecation::warn_unknown_fields::<UserConfig>(
                &system_path,
                &find_unknown_keys(&content),
                "System config",
            );

            // Feed migrated content to serde so deprecated patterns parse correctly
            let migrated = super::deprecation::migrate_content(&content);

            // Pre-validate with the toml crate for rich line/col errors.
            // Try OverridableConfig first — it has section fields (list,
            // commit, merge, ...) as direct fields, so toml tracks their
            // location correctly. UserConfig's flatten loses field paths.
            // Then try UserConfig to catch non-section fields (projects,
            // skip-*-prompt) that OverridableConfig silently ignores.
            if let Err(err) = toml::from_str::<OverridableConfig>(&migrated) {
                return Err(LoadError::File {
                    path: system_path,
                    label: "System config",
                    err: Box::new(err),
                });
            }
            if let Err(err) = toml::from_str::<Self>(&migrated) {
                return Err(LoadError::File {
                    path: system_path,
                    label: "System config",
                    err: Box::new(err),
                });
            }

            builder = builder.add_source(File::from_str(&migrated, config::FileFormat::Toml));
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
                let migrated = super::deprecation::check_and_migrate(
                    config_path,
                    &content,
                    true,
                    "User config",
                    None,
                    true, // show_brief_warning
                )
                .map(|result| result.migrated_content)
                .unwrap_or_else(|_| super::deprecation::migrate_content(&content));

                // Warn about unknown fields in the config file
                // (must check file content directly, not config.unknown, because
                // config.unknown includes env vars which shouldn't trigger warnings)
                super::deprecation::warn_unknown_fields::<UserConfig>(
                    config_path,
                    &find_unknown_keys(&content),
                    "User config",
                );

                // Pre-validate with the toml crate for rich line/col errors
                // (see system config comment above for the two-pass rationale).
                if let Err(err) = toml::from_str::<OverridableConfig>(&migrated) {
                    return Err(LoadError::File {
                        path: config_path.clone(),
                        label: "User config",
                        err: Box::new(err),
                    });
                }
                if let Err(err) = toml::from_str::<Self>(&migrated) {
                    return Err(LoadError::File {
                        path: config_path.clone(),
                        label: "User config",
                        err: Box::new(err),
                    });
                }

                // Feed migrated content from check_and_migrate to serde so deprecated
                // patterns parse correctly without reparsing the TOML here.
                builder = builder.add_source(File::from_str(&migrated, config::FileFormat::Toml));
            }
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
        // - try_parsing(true): coerce env-var strings into bool/i64/f64 so any
        //   non-String typed field (e.g. `list.timeout-ms: Option<u64>`) accepts
        //   overrides like `WORKTRUNK__LIST__TIMEOUT_MS=30`. String fields still
        //   round-trip through `into_string()` in the config deserializer, so
        //   `WORKTRUNK_WORKTREE_PATH=42` stringifies back to "42" as expected.
        //   Without this, a single typed override fails the whole config deserialize
        //   and silently falls back to defaults.
        // Example: WORKTRUNK_WORKTREE_PATH → worktree-path
        builder = builder.add_source(
            config::Environment::with_prefix("WORKTRUNK")
                .prefix_separator("_")
                .separator("__")
                .convert_case(Case::Kebab)
                .try_parsing(true),
        );

        // The config crate's `preserve_order` feature ensures TOML insertion order
        // is preserved (uses IndexMap instead of HashMap internally).
        // See: https://github.com/max-sixty/worktrunk/issues/737
        let config: Self = builder
            .build()
            .map_err(LoadError::Other)?
            .try_deserialize()
            .map_err(|err| {
                // Files were pre-validated above, so a deserialize failure here
                // is caused by env-var overrides.
                LoadError::Env {
                    err,
                    override_vars: collect_worktrunk_override_vars(),
                }
            })?;

        config.validate().map_err(LoadError::Other)?;

        Ok(config)
    }

    /// Load configuration from a TOML string for testing.
    #[cfg(test)]
    pub(crate) fn load_from_str(content: &str) -> Result<Self, ConfigError> {
        let migrated = crate::config::deprecation::migrate_content(content);
        let config: Self =
            toml::from_str(&migrated).map_err(|e| ConfigError::Message(e.to_string()))?;
        config.validate()?;
        Ok(config)
    }
}
