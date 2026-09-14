//! OpenCode plugin installation.
//!
//! Installs the worktrunk activity tracking plugin for OpenCode.
//! The plugin source (`dev/opencode-plugin.ts`) is embedded in the binary via
//! `include_str!()` and written under the OpenCode global-config directory (see
//! `opencode_plugins_dir` for the precedence rules) at `…/plugins/worktrunk.ts`.
//! That one file exports both plugin shapes OpenCode has shipped, so the same
//! install works on OpenCode 1 and OpenCode 2.

use std::path::PathBuf;

use anyhow::{Context, Result};

/// The plugin source, embedded at compile time.
const PLUGIN_SOURCE: &str = include_str!("../../../dev/opencode-plugin.ts");

/// Resolve the OpenCode plugins directory.
///
/// Mirrors OpenCode's own global-config precedence (see <https://opencode.ai/docs/config>):
/// `$OPENCODE_CONFIG_DIR` > `$XDG_CONFIG_HOME/opencode` > `~/.config/opencode`. The macOS
/// `~/Library/Application Support/opencode/` path is reserved for *managed* settings and is
/// not where OpenCode looks for user plugins, so we deliberately avoid `dirs::config_dir()`
/// here — it would put the plugin in the wrong place on macOS.
///
/// Both overrides read an exported-but-empty value as unset, matching
/// `CLAUDE_CONFIG_DIR` in `config::show` and `PI_CONFIG_DIR` in `config::pi`.
/// An empty value taken at face value yields the relative path `plugins/`, so
/// the install writes the plugin into whatever directory `wt` was run from.
/// `$XDG_CONFIG_HOME` gets the stricter XDG rule via
/// [`worktrunk::path::xdg_base_dir`] — a relative value is invalid there too,
/// and lands the plugin in the same wrong place an empty one would.
fn opencode_plugins_dir() -> Result<PathBuf> {
    let config_dir = if let Some(dir) = std::env::var("OPENCODE_CONFIG_DIR")
        .ok()
        .filter(|s| !s.is_empty())
    {
        PathBuf::from(dir)
    } else if let Some(xdg) = worktrunk::path::xdg_base_dir("XDG_CONFIG_HOME") {
        xdg.join("opencode")
    } else {
        worktrunk::path::home_dir()
            .context("Could not determine home directory")?
            .join(".config")
            .join("opencode")
    };
    Ok(config_dir.join("plugins"))
}

/// Get the target path for the plugin file.
pub fn plugin_path() -> Result<PathBuf> {
    Ok(opencode_plugins_dir()?.join("worktrunk.ts"))
}

/// Check if the plugin is already installed with current content.
pub fn is_plugin_installed() -> bool {
    plugin_path()
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .is_some_and(|content| content == PLUGIN_SOURCE)
}

/// Check if a plugin file exists (possibly outdated).
pub fn plugin_file_exists() -> bool {
    plugin_path().map(|p| p.exists()).unwrap_or(false)
}

/// Handle `wt config plugins opencode install`.
pub fn handle_opencode_install(yes: bool) -> Result<()> {
    let target = plugin_path()?;
    super::install_file_plugin("OpenCode", &target, PLUGIN_SOURCE, yes)
}

/// Handle `wt config plugins opencode uninstall`.
pub fn handle_opencode_uninstall(yes: bool) -> Result<()> {
    let target = plugin_path()?;
    super::uninstall_file_plugin("OpenCode", &target, yes)
}
