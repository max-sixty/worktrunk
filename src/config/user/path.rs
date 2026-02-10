//! Config path management.
//!
//! Handles determining the user config file location across platforms,
//! with support for CLI overrides and environment variables.

use std::path::PathBuf;
use std::sync::OnceLock;

#[cfg(not(test))]
use etcetera::base_strategy::{BaseStrategy, choose_base_strategy};

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
/// 3. Platform-specific default location
pub fn get_config_path() -> Option<PathBuf> {
    // Priority 1: CLI --config flag
    if let Some(path) = CONFIG_PATH.get() {
        return Some(path.clone());
    }

    // Priority 2: Environment variable (also used by tests)
    if let Ok(path) = std::env::var("WORKTRUNK_CONFIG_PATH") {
        return Some(PathBuf::from(path));
    }

    // In test builds, WORKTRUNK_CONFIG_PATH must be set to prevent polluting user config
    #[cfg(test)]
    panic!(
        "WORKTRUNK_CONFIG_PATH not set in test. Tests must use TestRepo which sets this automatically, \
        or set it manually to an isolated test config path."
    );

    // Production: use standard config location
    // choose_base_strategy uses:
    // - XDG on Linux (respects XDG_CONFIG_HOME, falls back to ~/.config)
    // - XDG on macOS (~/.config instead of ~/Library/Application Support)
    // - Windows conventions on Windows (%APPDATA%)
    #[cfg(not(test))]
    {
        let strategy = choose_base_strategy().ok()?;
        Some(strategy.config_dir().join("worktrunk").join("config.toml"))
    }
}

/// Get the system-wide config file path, if one exists.
///
/// System config provides organization-wide defaults that user config overrides.
/// Returns the first existing config file found in the system config directories.
///
/// Priority:
/// 1. WORKTRUNK_SYSTEM_CONFIG_PATH environment variable (for testing/overrides)
/// 2. Each directory in $XDG_CONFIG_DIRS (colon-separated, checked in order)
/// 3. Platform-specific default:
///    - Linux: /etc/xdg/worktrunk/config.toml (XDG default)
///    - macOS: /Library/Application Support/worktrunk/config.toml
///    - Windows: %PROGRAMDATA%\worktrunk\config.toml
pub fn get_system_config_path() -> Option<PathBuf> {
    // Priority 1: Explicit environment variable override
    if let Ok(path) = std::env::var("WORKTRUNK_SYSTEM_CONFIG_PATH") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
        return None;
    }

    // In test builds, only use WORKTRUNK_SYSTEM_CONFIG_PATH (prevents reading host system config)
    #[cfg(test)]
    return None;

    // Priority 2: Check $XDG_CONFIG_DIRS directories
    #[cfg(not(test))]
    {
        if let Ok(dirs) = std::env::var("XDG_CONFIG_DIRS") {
            for dir in dirs.split(':').filter(|d| !d.is_empty()) {
                let path = PathBuf::from(dir)
                    .join("worktrunk")
                    .join("config.toml");
                if path.exists() {
                    return Some(path);
                }
            }
            // XDG_CONFIG_DIRS was set but no config found in any directory
            return None;
        }

        // Priority 3: Platform-specific defaults (only when XDG_CONFIG_DIRS is not set)
        for dir in platform_system_config_dirs() {
            let path = dir.join("worktrunk").join("config.toml");
            if path.exists() {
                return Some(path);
            }
        }

        None
    }
}

/// Returns the candidate directories where system config might be found.
///
/// Used by `wt config show` to display the expected system config path
/// even when no config file exists yet.
pub fn system_config_search_dirs() -> Vec<PathBuf> {
    if let Ok(path) = std::env::var("WORKTRUNK_SYSTEM_CONFIG_PATH") {
        // When explicitly set, the search directory is the parent
        let path = PathBuf::from(path);
        if let Some(parent) = path.parent().and_then(|p| p.parent()) {
            return vec![parent.to_path_buf()];
        }
        return vec![];
    }

    #[cfg(test)]
    return vec![];

    #[cfg(not(test))]
    {
        if let Ok(dirs) = std::env::var("XDG_CONFIG_DIRS") {
            return dirs
                .split(':')
                .filter(|d| !d.is_empty())
                .map(PathBuf::from)
                .collect();
        }

        platform_system_config_dirs()
    }
}

/// Platform-specific default system config directories.
///
/// These are used when $XDG_CONFIG_DIRS is not set.
#[cfg(not(test))]
#[allow(clippy::vec_init_then_push)]
fn platform_system_config_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    #[cfg(target_os = "macos")]
    {
        // macOS native system-wide config location
        dirs.push(PathBuf::from("/Library/Application Support"));
    }

    #[cfg(target_os = "windows")]
    {
        // Windows: %PROGRAMDATA% (typically C:\ProgramData)
        if let Ok(program_data) = std::env::var("PROGRAMDATA") {
            dirs.push(PathBuf::from(program_data));
        }
    }

    // XDG default: /etc/xdg (standard on Linux, also works on macOS/other Unix)
    #[cfg(unix)]
    dirs.push(PathBuf::from("/etc/xdg"));

    dirs
}
