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

use std::path::{Path, PathBuf};

use super::ConfigError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// Re-export public types
pub use merge::Merge;
pub use path::{
    config_path, default_config_path, default_system_config_path, set_config_path,
    system_config_path,
};
pub use resolved::ResolvedConfig;
pub use schema::valid_user_config_keys;
pub use sections::{
    CommitConfig, CommitGenerationConfig, CopyIgnoredConfig, ListConfig, MergeConfig, StageMode,
    StepConfig, SwitchConfig, SwitchPickerConfig, UserProjectOverrides,
};

/// Describes a problem encountered during config loading. Each variant
/// identifies which layer failed so callers can emit targeted diagnostics
/// (file errors with line/col vs env-var attribution).
///
/// Used as an error by [`UserConfig::load_with_cause()`] (first issue is
/// fatal) and as warnings by [`UserConfig::load_with_warnings()`] (issues
/// are collected, best-effort config returned).
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
    /// `vars` lists the exact `WORKTRUNK_*` env vars that were parsed
    /// as `(name, value)` pairs.
    Env {
        err: String,
        vars: Vec<(String, String)>,
    },
    /// Validation errors (e.g. empty worktree-path).
    Validation(String),
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
            LoadError::Validation(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for LoadError {}

// ---- Env-var overlay ----

/// A single parsed WORKTRUNK_* env var with both typed and string representations.
struct EnvVar {
    /// Original env var name (e.g., `WORKTRUNK__LIST__TIMEOUT_MS`)
    name: String,
    /// TOML path segments (e.g., `["list", "timeout-ms"]`)
    segments: Vec<String>,
    /// Typed TOML value (bool/int/float/string coercion via [`try_parse_value`])
    typed_value: toml::Value,
    /// Raw string value, kept for fallback when the typed form doesn't match
    /// the target field's type (e.g., `WORKTRUNK_WORKTREE_PATH=42` needs
    /// `String`, not `Integer`).
    raw_value: String,
}

/// Read `WORKTRUNK_*` env vars and parse each into an [`EnvVar`].
///
/// Env-var convention (matches the config crate's prior behavior):
/// - `WORKTRUNK_WORKTREE_PATH=foo` → `worktree-path = "foo"`
/// - `WORKTRUNK__LIST__TIMEOUT_MS=30` → `[list]\ntimeout-ms = 30`
/// - `WORKTRUNK_COMMIT__GENERATION__COMMAND=cmd` → `[commit.generation]\ncommand = "cmd"`
///
/// Infrastructure vars (`_CONFIG_PATH`, `_SYSTEM_CONFIG_PATH`,
/// `_APPROVALS_PATH`) and test vars (`_TEST_*`) are excluded.
fn parse_worktrunk_env_vars() -> Vec<EnvVar> {
    const INFRA_VARS: &[&str] = &[
        "WORKTRUNK_CONFIG_PATH",
        "WORKTRUNK_SYSTEM_CONFIG_PATH",
        "WORKTRUNK_APPROVALS_PATH",
    ];

    let mut env_vars: Vec<_> = std::env::vars()
        .filter(|(k, _)| k.starts_with("WORKTRUNK_"))
        .filter(|(k, _)| !INFRA_VARS.contains(&k.as_str()))
        .filter(|(k, _)| !k.starts_with("WORKTRUNK_TEST_"))
        .collect();
    env_vars.sort_by(|a, b| a.0.cmp(&b.0));

    env_vars
        .into_iter()
        .filter_map(|(key, value)| {
            // Strip WORKTRUNK_ prefix, split by __ for nesting, convert to kebab-case
            let stripped = &key["WORKTRUNK_".len()..];
            let segments: Vec<String> = stripped
                .split("__")
                .map(|s| {
                    s.to_lowercase()
                        .replace('_', "-")
                        .trim_start_matches('-')
                        .to_string()
                })
                .filter(|s| !s.is_empty())
                .collect();

            if segments.is_empty() {
                return None;
            }

            Some(EnvVar {
                name: key,
                segments,
                typed_value: try_parse_value(&value),
                raw_value: value,
            })
        })
        .collect()
}

/// For each env var, probe whether its typed or string representation
/// deserializes correctly against the file config, then build a single
/// overlay table with the correct representation per var.
///
/// Each var is tested independently against the file table (not against other
/// env vars). This lets serde itself decide the correct type — no schema
/// walking or guessing needed. O(N) deserializations where N is the number
/// of env vars (tiny in practice).
fn resolve_env_overlay(file_table: &toml::Table, vars: &[EnvVar]) -> toml::Table {
    let mut overlay = toml::Table::new();
    for var in vars {
        // Typed probe: merge just this var's typed value into the file table
        let mut probe = file_table.clone();
        set_nested_value(&mut probe, &var.segments, var.typed_value.clone());
        if toml::Value::Table(probe).try_into::<UserConfig>().is_ok() {
            set_nested_value(&mut overlay, &var.segments, var.typed_value.clone());
        } else {
            // Typed form doesn't fit the target field — use raw string.
            // If this is also wrong (e.g., "not-a-bool" for a bool field),
            // the final deserialize will catch it and surface LoadError::Env.
            set_nested_value(
                &mut overlay,
                &var.segments,
                toml::Value::String(var.raw_value.clone()),
            );
        }
    }
    overlay
}

/// Try to coerce a string into a typed TOML value (bool → i64 → f64 → string).
fn try_parse_value(s: &str) -> toml::Value {
    if s.eq_ignore_ascii_case("true") {
        return toml::Value::Boolean(true);
    }
    if s.eq_ignore_ascii_case("false") {
        return toml::Value::Boolean(false);
    }
    if let Ok(n) = s.parse::<i64>() {
        return toml::Value::Integer(n);
    }
    if let Ok(n) = s.parse::<f64>() {
        return toml::Value::Float(n);
    }
    toml::Value::String(s.to_string())
}

/// Set a value at a nested path in a TOML table, creating intermediate tables.
fn set_nested_value(table: &mut toml::Table, path: &[String], value: toml::Value) {
    if path.len() == 1 {
        table.insert(path[0].clone(), value);
        return;
    }
    let entry = table
        .entry(&path[0])
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if let toml::Value::Table(inner) = entry {
        set_nested_value(inner, &path[1..], value);
    }
}

/// Recursively merge `overlay` into `base`. Overlay values win for scalars;
/// nested tables merge recursively.
fn deep_merge_table(base: &mut toml::Table, overlay: toml::Table) {
    for (key, value) in overlay {
        match (base.get_mut(&key), &value) {
            (Some(toml::Value::Table(base_t)), toml::Value::Table(overlay_t)) => {
                deep_merge_table(base_t, overlay_t.clone());
            }
            _ => {
                base.insert(key, value);
            }
        }
    }
}

/// Load and validate a single config file. Returns the parsed table for
/// merging and validates via `toml::from_str::<UserConfig>` for rich errors.
fn load_config_file(
    path: &Path,
    migrated: &str,
    label: &'static str,
) -> Result<toml::Table, LoadError> {
    // Validate by deserializing — gives rich line/col errors.
    if let Err(err) = toml::from_str::<UserConfig>(migrated) {
        return Err(LoadError::File {
            path: path.to_path_buf(),
            label,
            err: Box::new(err),
        });
    }
    // Parse as table for merging. Infallible after from_str::<UserConfig>
    // succeeds — valid UserConfig TOML is always valid TOML.
    Ok(migrated
        .parse::<toml::Table>()
        .expect("valid TOML after UserConfig parse"))
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
/// worktree-path = ".worktrees/{{ branch | sanitize }}"
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

    /// Hooks configuration (top-level keys like pre-merge, post-switch, etc.)
    #[serde(flatten, default)]
    pub hooks: crate::config::HooksConfig,

    /// Worktree path template
    #[serde(
        rename = "worktree-path",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub worktree_path: Option<String>,

    /// Configuration for the `wt list` command
    #[serde(default, skip_serializing_if = "super::is_default")]
    pub list: sections::ListConfig,

    /// Configuration for the `wt step commit` command (also used by merge)
    #[serde(default, skip_serializing_if = "super::is_default")]
    pub commit: sections::CommitConfig,

    /// Configuration for the `wt merge` command
    #[serde(default, skip_serializing_if = "super::is_default")]
    pub merge: sections::MergeConfig,

    /// Configuration for the `wt switch` command
    #[serde(default, skip_serializing_if = "super::is_default")]
    pub switch: sections::SwitchConfig,

    /// Configuration for `wt step` subcommands
    #[serde(default, skip_serializing_if = "super::is_default")]
    pub step: sections::StepConfig,

    /// Command aliases for `wt step <name>`
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub aliases: std::collections::BTreeMap<String, crate::config::commands::CommandConfig>,

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
        Self::load_with_cause().map_err(|e| ConfigError(e.to_string()))
    }

    /// Like [`load()`](Self::load), but returns a [`LoadError`] that
    /// distinguishes file-level parse failures (with line/col) from
    /// env-var override failures.
    pub(crate) fn load_with_cause() -> Result<Self, LoadError> {
        let (config, warnings) = Self::load_with_warnings();
        if let Some(err) = warnings.into_iter().next() {
            return Err(err);
        }
        Ok(config)
    }

    /// Load the best config achievable, collecting issues as warnings.
    ///
    /// Each config layer (system file, user file, env-var overlay) degrades
    /// independently — a failure in one preserves data from earlier layers
    /// instead of falling back to defaults. This means:
    /// - Bad system config → skipped, user config + env vars still apply
    /// - Bad user config → skipped, system config + env vars still apply
    /// - Bad env vars → ignored, file-based config preserved
    /// - Validation failure → warning emitted, defaults used (invalid config
    ///   causes bad behavior if applied, e.g. empty worktree-path template)
    pub(crate) fn load_with_warnings() -> (Self, Vec<LoadError>) {
        let mut warnings = Vec::new();
        let mut merged_table = toml::Table::new();

        // 1. System config (lowest priority)
        if let Some(system_path) = path::system_config_path()
            && let Ok(content) = std::fs::read_to_string(&system_path)
        {
            super::deprecation::warn_unknown_fields::<UserConfig>(
                &content,
                &system_path,
                "System config",
            );
            let migrated = super::deprecation::migrate_content(&content);
            match load_config_file(&system_path, &migrated, "System config") {
                Ok(table) => deep_merge_table(&mut merged_table, table),
                Err(e) => warnings.push(e),
            }
        }

        // 2. User config (overrides system)
        let config_path = config_path();
        if let Some(config_path) = config_path.as_ref()
            && config_path.exists()
        {
            if let Ok(content) = std::fs::read_to_string(config_path) {
                let migrated = super::deprecation::check_and_migrate(
                    config_path,
                    &content,
                    true,
                    "User config",
                    None,
                    true,
                )
                .map(|result| result.migrated_content)
                .unwrap_or_else(|_| super::deprecation::migrate_content(&content));

                super::deprecation::warn_unknown_fields::<UserConfig>(
                    &content,
                    config_path,
                    "User config",
                );

                match load_config_file(config_path, &migrated, "User config") {
                    Ok(table) => deep_merge_table(&mut merged_table, table),
                    Err(e) => warnings.push(e),
                }
            }
        } else if let Some(config_path) = config_path.as_ref()
            && path::is_config_path_explicit()
        {
            crate::styling::eprintln!(
                "{}",
                crate::styling::warning_message(format!(
                    "Config file not found: {}",
                    crate::path::format_path_for_display(config_path)
                ))
            );
        }

        // 3. Env-var overrides (highest priority)
        let env_vars = parse_worktrunk_env_vars();

        if env_vars.is_empty() {
            return Self::finalize(merged_table, warnings);
        }

        // Resolve each env var's type independently: probe typed form against
        // the file table, fall back to string if typed doesn't fit the target
        // field. This handles mixed cases (e.g., WORKTRUNK__LIST__TIMEOUT_MS=100
        // needs Integer for u64, WORKTRUNK_WORKTREE_PATH=42 needs String).
        let file_table = merged_table.clone();
        let env_overlay = resolve_env_overlay(&file_table, &env_vars);
        deep_merge_table(&mut merged_table, env_overlay);

        match toml::Value::Table(merged_table).try_into::<Self>() {
            Ok(config) => match config.validate() {
                Ok(()) => (config, warnings),
                Err(e) => {
                    warnings.push(LoadError::Validation(e.0));
                    (Self::default(), warnings)
                }
            },
            Err(err) => {
                warnings.push(LoadError::Env {
                    err: err.to_string(),
                    vars: env_vars
                        .iter()
                        .map(|v| (v.name.clone(), v.raw_value.clone()))
                        .collect(),
                });
                // Env overlay broke deserialization — fall back to file-only config.
                // Each file was individually validated by load_config_file(), so the
                // merged table should deserialize cleanly. If it doesn't, finalize()
                // falls back to defaults.
                Self::finalize(file_table, warnings)
            }
        }
    }

    /// Deserialize a merged table into `UserConfig`, validate, and collect
    /// any issues as warnings. Shared by the no-env and env-fallback paths.
    fn finalize(table: toml::Table, mut warnings: Vec<LoadError>) -> (Self, Vec<LoadError>) {
        match toml::Value::Table(table).try_into::<Self>() {
            Ok(config) => match config.validate() {
                Ok(()) => (config, warnings),
                Err(e) => {
                    // Validation means the config is semantically wrong (e.g.,
                    // worktree-path=""). Using it causes bad behavior, so fall
                    // back to defaults rather than applying the broken config.
                    warnings.push(LoadError::Validation(e.0));
                    (Self::default(), warnings)
                }
            },
            Err(err) => {
                warnings.push(LoadError::Validation(err.to_string()));
                (Self::default(), warnings)
            }
        }
    }

    /// Load configuration from a TOML string for testing.
    #[cfg(test)]
    pub(crate) fn load_from_str(content: &str) -> Result<Self, ConfigError> {
        let migrated = crate::config::deprecation::migrate_content(content);
        let config: Self = toml::from_str(&migrated).map_err(|e| ConfigError(e.to_string()))?;
        config.validate()?;
        Ok(config)
    }
}
