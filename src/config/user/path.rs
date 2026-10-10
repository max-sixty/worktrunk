//! Config path management.
//!
//! Handles determining the user config file location across platforms,
//! with support for CLI overrides and environment variables.

use std::path::PathBuf;
use std::sync::OnceLock;

use etcetera::base_strategy::{BaseStrategy, choose_base_strategy};

use crate::config::ConfigError;
use crate::git::resolve_input_path;

/// Override for user config path, set via --config CLI flag
static CONFIG_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Set the user config path override (called from CLI --config flag)
pub fn set_config_path(path: PathBuf) {
    CONFIG_PATH.set(path).ok();
}

/// Check if the config path was explicitly specified via --config CLI flag.
///
/// Returns true only if --config flag was used. Environment variable
/// (WORKTRUNK_CONFIG_PATH) is not considered "explicit" because it's commonly
/// used for test/CI isolation with intentionally non-existent paths.
pub fn is_config_path_explicit() -> bool {
    CONFIG_PATH.get().is_some()
}

/// Get the user config file path.
///
/// Priority:
/// 1. CLI --config flag (set via `set_config_path`)
/// 2. WORKTRUNK_CONFIG_PATH environment variable
/// 3. Platform-specific default location (via `default_config_path`)
///
/// The first two are supplied by the user, so a relative one resolves against
/// `-C` (see [`resolve_input_path`]). The third is an absolute XDG location.
///
/// Priority 3 is absent under `#[cfg(test)]`: it resolves the developer's own
/// config, which callers both read (`prewarm_user_config`) and — via
/// `set_skip_shell_integration_prompt` / `set_skip_commit_generation_prompt` —
/// write. An in-process test has no way to set priority 2 for itself
/// (`std::env::set_var` is `unsafe`, and this crate forbids `unsafe`), so a test
/// that reaches here wanted an explicit path, not the developer's.
///
/// `None` rather than the `panic!` [`crate::config::approvals_path`] uses: that
/// one guards a mutation target, where a silent absence would let a test believe
/// it saved something. This is a lookup whose absent state is already meaningful
/// and already handled — `require_config_path` turns it into an error, so a
/// config *write* still fails loudly, while a best-effort *read* like the
/// prewarm cache simply preloads nothing.
///
/// The guard fires for lib-crate tests only; a bin-crate test links this crate
/// in non-test mode, so `src/commands/` and `src/output/` stay uncovered. See
/// `tests/CLAUDE.md`.
pub fn config_path() -> Option<PathBuf> {
    // Priority 1: CLI --config flag
    if let Some(path) = CONFIG_PATH.get() {
        return Some(resolve_input_path(path));
    }

    // Priority 2: Environment variable (also used by tests for isolation)
    if let Ok(path) = std::env::var("WORKTRUNK_CONFIG_PATH") {
        return Some(resolve_input_path(path));
    }

    // Priority 3: Platform-specific default location
    #[cfg(test)]
    return None;

    #[cfg(not(test))]
    default_config_path()
}

/// Resolve the user config path, erroring when no location can be determined.
///
/// The `Result`-returning counterpart of [`config_path`], for callers that
/// must produce a concrete path (config mutation) rather than tolerate its
/// absence.
pub fn require_config_path() -> Result<PathBuf, ConfigError> {
    config_path().ok_or_else(|| {
        ConfigError("Cannot determine config directory. Set $HOME or $XDG_CONFIG_HOME".to_string())
    })
}

/// Resolve the user config path for display, formatted with `~` and falling
/// back to the canonical location when none can be determined.
///
/// The display counterpart of [`config_path`]: user-facing messages that name
/// "the config file wt would load or write" route through this, so the
/// `--config` / `WORKTRUNK_CONFIG_PATH` / `$XDG_CONFIG_HOME` resolution and the
/// fallback literal live in one place. Use [`require_config_path`] for the
/// actual mutation; this is display-only.
pub fn config_path_for_display() -> String {
    config_path()
        .map(|p| crate::path::format_path_for_display(&p))
        .unwrap_or_else(|| "~/.config/worktrunk/config.toml".to_string())
}

/// Platform-specific default config path, without CLI or env var overrides.
///
/// Returns the etcetera-based platform default. Called by `config_path()`
/// as the final fallback when no CLI or env var override is set.
///
/// `etcetera::choose_base_strategy` follows the CLI convention of using XDG
/// on every Unix platform (including macOS) and the native APPDATA strategy on
/// Windows. Concretely:
/// - Unix (Linux + macOS): `$XDG_CONFIG_HOME/worktrunk/config.toml`
///   (default `~/.config/worktrunk/config.toml`)
/// - Windows: `%APPDATA%\worktrunk\config.toml`
pub fn default_config_path() -> Option<PathBuf> {
    let strategy = choose_base_strategy().ok()?;
    Some(strategy.config_dir().join("worktrunk").join("config.toml"))
}

/// The system-wide config file's location, whether or not the file exists.
///
/// System config provides organization-wide defaults that user config overrides.
/// `WORKTRUNK_SYSTEM_CONFIG_PATH` overrides the location; otherwise it is one
/// fixed path per platform, as git's `--system` file is:
/// - macOS: `/Library/Application Support/worktrunk/config.toml`
/// - Windows: `%PROGRAMDATA%\worktrunk\config.toml`
/// - other Unix: `/etc/xdg/worktrunk/config.toml`
///
/// `XDG_CONFIG_DIRS` is deliberately not consulted: an administrator deploys to
/// the fixed path, and a search path read from the environment would bring the
/// spec's parsing rules with it for a case no one has asked for.
///
/// Deliberately unguarded, unlike `config_path()`: this resolves a machine-wide
/// file rather than the developer's own config.
pub fn system_config_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("WORKTRUNK_SYSTEM_CONFIG_PATH") {
        return Some(resolve_input_path(path));
    }

    #[cfg(target_os = "macos")]
    let dir = Some(PathBuf::from("/Library/Application Support"));
    #[cfg(windows)]
    let dir = std::env::var_os("PROGRAMDATA").map(PathBuf::from);
    #[cfg(not(any(target_os = "macos", windows)))]
    let dir = Some(PathBuf::from("/etc/xdg"));

    Some(dir?.join("worktrunk").join("config.toml"))
}
