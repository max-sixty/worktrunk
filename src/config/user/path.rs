//! Config path management.
//!
//! Handles determining the user config file location across platforms,
//! with support for CLI overrides and environment variables.

use std::path::PathBuf;
use std::sync::OnceLock;

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

    // Priority 2: Environment variable (also used by tests for isolation)
    if let Ok(path) = std::env::var("WORKTRUNK_CONFIG_PATH") {
        return Some(PathBuf::from(path));
    }

    // Priority 3: Platform-specific default location
    let strategy = choose_base_strategy().ok()?;
    Some(strategy.config_dir().join("worktrunk").join("config.toml"))
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

    // Priority 2+3: Check XDG_CONFIG_DIRS (if set), otherwise platform defaults.
    // When XDG_CONFIG_DIRS is set, system_config_dirs() returns only those dirs
    // (per XDG spec, no fallback to platform defaults).
    for dir in &system_config_dirs() {
        let path = dir.join("worktrunk").join("config.toml");
        if path.exists() {
            return Some(path);
        }
    }

    None
}

/// The expected system config path for the current platform.
///
/// Used by `wt config show` to display where to put a system config file.
/// Mirrors the lookup order in `get_system_config_path()` so the displayed
/// path matches where the tool actually looks.
pub fn default_system_config_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("WORKTRUNK_SYSTEM_CONFIG_PATH") {
        return Some(PathBuf::from(path));
    }

    system_config_dirs()
        .first()
        .map(|dir| dir.join("worktrunk").join("config.toml"))
}

/// System config directories in priority order.
///
/// On Unix, checks `XDG_CONFIG_DIRS` first. When set, it defines the search
/// path exclusively (per XDG spec) — no fallback to platform defaults.
/// Otherwise, returns platform-specific defaults (macOS: `/Library/Application
/// Support`, Windows: `%PROGRAMDATA%`, Unix: `/etc/xdg`).
fn system_config_dirs() -> Vec<PathBuf> {
    #[cfg(unix)]
    if let Ok(dirs_str) = std::env::var("XDG_CONFIG_DIRS") {
        let dirs: Vec<PathBuf> = dirs_str
            .split(':')
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .collect();
        if !dirs.is_empty() {
            return dirs;
        }
    }

    platform_default_dirs()
}

/// Platform-specific default system config directories.
///
/// Returns directories in priority order — the first existing config file wins.
/// On macOS, the native `/Library/Application Support/` is checked before the
/// XDG fallback `/etc/xdg/`.
#[allow(clippy::vec_init_then_push)]
fn platform_default_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    #[cfg(target_os = "macos")]
    {
        // macOS native system-wide config location (checked first)
        dirs.push(PathBuf::from("/Library/Application Support"));
    }

    #[cfg(target_os = "windows")]
    {
        // Windows: %PROGRAMDATA% (typically C:\ProgramData)
        if let Ok(program_data) = std::env::var("PROGRAMDATA") {
            dirs.push(PathBuf::from(program_data));
        }
    }

    // XDG default: /etc/xdg (standard on Linux, fallback on macOS/other Unix)
    #[cfg(unix)]
    dirs.push(PathBuf::from("/etc/xdg"));

    dirs
}
