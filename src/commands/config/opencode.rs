//! OpenCode plugin installation.
//!
//! Installs the worktrunk activity tracking plugin for OpenCode.
//! The plugin source (`dev/opencode-plugin.ts`) is embedded in the binary via
//! `include_str!()` and written to `${OPENCODE_CONFIG_DIR:-~/.config/opencode}/plugins/worktrunk.ts`.

use std::path::PathBuf;

use anyhow::{Context, Result};
use color_print::cformat;
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{eprintln, hint_message, info_message, success_message};

use crate::output::prompt::{PromptResponse, prompt_yes_no_preview};

/// The plugin source, embedded at compile time.
const PLUGIN_SOURCE: &str = include_str!("../../../dev/opencode-plugin.ts");

/// Resolve the OpenCode plugins directory.
///
/// Uses `$OPENCODE_CONFIG_DIR/plugins/` if set, otherwise `~/.config/opencode/plugins/`.
fn opencode_plugins_dir() -> Result<PathBuf> {
    let config_dir = if let Ok(dir) = std::env::var("OPENCODE_CONFIG_DIR") {
        PathBuf::from(dir)
    } else {
        dirs::config_dir()
            .context("Could not determine config directory")?
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

/// Confirm an action via prompt, or accept automatically with `--yes`.
fn confirm_or_yes(yes: bool, prompt: &str, preview: impl Fn()) -> Result<bool> {
    Ok(yes || prompt_yes_no_preview(prompt, preview)? == PromptResponse::Accepted)
}

/// Handle `wt config plugins opencode install`.
pub fn handle_opencode_install(yes: bool) -> Result<()> {
    let target = plugin_path()?;
    let target_display = format_path_for_display(&target);

    // Check if already installed with current content
    if target.exists()
        && let Ok(existing) = std::fs::read_to_string(&target)
        && existing == PLUGIN_SOURCE
    {
        eprintln!(
            "{}",
            info_message(cformat!(
                "Plugin already installed @ <bold>{target_display}</>"
            ))
        );
        return Ok(());
    }

    let action = if target.exists() { "Update" } else { "Install" };
    let preview_msg = info_message(cformat!("Would write to <bold>{target_display}</>"));
    let preview = || eprintln!("{}", preview_msg);

    let confirmed = confirm_or_yes(
        yes,
        &cformat!("{action} OpenCode plugin @ <bold>{target_display}</>?"),
        preview,
    );
    if !confirmed? {
        return Ok(());
    }

    // Create parent directories if needed
    let parent = target
        .parent()
        .context("Plugin path has no parent directory")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create directory {}", parent.display()))?;

    // Write the plugin file
    std::fs::write(&target, PLUGIN_SOURCE)
        .with_context(|| format!("Failed to write plugin to {target_display}"))?;

    eprintln!(
        "{}",
        success_message(cformat!("Plugin installed @ <bold>{target_display}</>"))
    );
    eprintln!(
        "{}",
        hint_message(cformat!(
            "Activity markers (🤖/💬) will appear in <underline>wt list</>"
        ))
    );

    Ok(())
}

/// Handle `wt config plugins opencode uninstall`.
pub fn handle_opencode_uninstall(yes: bool) -> Result<()> {
    let target = plugin_path()?;
    let target_display = format_path_for_display(&target);

    if !target.exists() {
        eprintln!("{}", info_message("Plugin not installed"));
        return Ok(());
    }

    let preview_msg = info_message(cformat!("Would remove <bold>{target_display}</>"));
    let preview = || eprintln!("{}", preview_msg);

    let confirmed = confirm_or_yes(
        yes,
        &cformat!("Remove OpenCode plugin @ <bold>{target_display}</>?"),
        preview,
    );
    if !confirmed? {
        return Ok(());
    }

    std::fs::remove_file(&target)
        .with_context(|| format!("Failed to remove plugin at {target_display}"))?;

    eprintln!(
        "{}",
        success_message(cformat!("Plugin removed from <bold>{target_display}</>"))
    );

    Ok(())
}
