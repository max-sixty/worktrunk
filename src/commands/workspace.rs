//! Workspace (zellij) command handler.
//!
//! Implements `wt ui` for entering/creating a zellij workspace for the repository.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use color_print::cformat;
use worktrunk::git::Repository;
use worktrunk::zellij::{
    SessionStatus, ZellijContext, attach_session, create_session, delete_session, detect_context,
    is_zellij_available, session_name_for_repo, session_status,
};

use crate::output;

/// Zellij config directory (~/.config/zellij, even on macOS).
fn zellij_config_dir() -> PathBuf {
    dirs::home_dir().unwrap().join(".config").join("zellij")
}

/// Path to the zellij config file.
fn zellij_config_path() -> PathBuf {
    zellij_config_dir().join("config.kdl")
}

/// Path to the installed wt-bridge plugin.
pub fn plugin_path() -> PathBuf {
    zellij_config_dir().join("plugins").join("wt-bridge.wasm")
}

/// Path to the zellij layout file.
pub fn layout_path() -> PathBuf {
    zellij_config_dir().join("layouts").join("worktrunk.kdl")
}

/// Check if the plugin is installed.
pub fn is_plugin_installed() -> bool {
    plugin_path().exists()
}

/// The zellij layout for worktrunk workspaces.
///
/// Layout with a terminal pane and tab bar for visibility.
/// The wt-bridge plugin is loaded in the background via load_plugins in config.kdl.
fn layout_content() -> &'static str {
    r#"layout {
    default_tab_template {
        pane size=1 borderless=true {
            plugin location="compact-bar"
        }
        children
    }
    tab name="main" {
        pane
    }
}
"#
}

/// Add wt-bridge to zellij's load_plugins configuration.
///
/// This ensures the plugin loads in the background on session start,
/// with zellij handling the permission dialog automatically.
fn add_plugin_to_config() -> anyhow::Result<bool> {
    let config_path = zellij_config_path();
    let plugin = plugin_path();
    let plugin_entry = format!("\"file:{}\"", plugin.display());

    // Read existing config or start fresh
    let content = if config_path.exists() {
        fs::read_to_string(&config_path)?
    } else {
        String::new()
    };

    // Check if plugin is already configured
    if content.contains(&plugin_entry) || content.contains("wt-bridge.wasm") {
        return Ok(false); // Already configured
    }

    // Build the new config content
    let new_content = if content.contains("load_plugins {") {
        // Add to existing load_plugins block
        content.replace(
            "load_plugins {",
            &format!("load_plugins {{\n    {}", plugin_entry),
        )
    } else {
        // Append new load_plugins block
        let block = format!(
            "\n// Worktrunk workspace plugin (added by wt ui setup)\nload_plugins {{\n    {}\n}}\n",
            plugin_entry
        );
        if content.is_empty() {
            block.trim_start().to_string()
        } else {
            format!("{}{}", content.trim_end(), block)
        }
    };

    fs::create_dir_all(config_path.parent().unwrap())?;
    fs::write(&config_path, new_content)?;
    Ok(true)
}

/// Handle `wt ui setup`: Install the wt-bridge plugin.
///
/// This builds the plugin from source and installs it to `~/.config/zellij/`.
pub fn handle_setup() -> anyhow::Result<()> {
    // Find the wt-bridge source directory
    let bridge_dir = find_bridge_source()?;

    output::progress("Building wt-bridge plugin...")?;

    // Build the plugin for wasm32-wasip1
    let output = Command::new("cargo")
        .args(["build", "--target", "wasm32-wasip1", "--release"])
        .current_dir(&bridge_dir)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to build wt-bridge plugin:\n{}", stderr);
    }

    // Verify the built file exists (binary name uses hyphen)
    let built_wasm = bridge_dir
        .join("target")
        .join("wasm32-wasip1")
        .join("release")
        .join("wt-bridge.wasm");

    if !built_wasm.exists() {
        anyhow::bail!(
            "Build succeeded but plugin not found at {}",
            built_wasm.display()
        );
    }

    // Install plugin to ~/.config/zellij/plugins/
    let dest = plugin_path();
    fs::create_dir_all(dest.parent().unwrap())?;
    fs::copy(&built_wasm, &dest)?;

    output::success(cformat!("Installed plugin to <bold>{}</>", dest.display()))?;

    // Add plugin to zellij config.kdl for background loading
    if add_plugin_to_config()? {
        output::success(cformat!(
            "Added plugin to <bold>{}</>",
            zellij_config_path().display()
        ))?;
    } else {
        output::info("Plugin already configured in config.kdl")?;
    }

    // Create layout at ~/.config/zellij/layouts/
    let layout = layout_path();
    fs::create_dir_all(layout.parent().unwrap())?;
    fs::write(&layout, layout_content())?;

    output::success(cformat!("Created layout at <bold>{}</>", layout.display()))?;

    output::hint("Run 'wt ui' to enter the workspace")?;

    Ok(())
}

/// Find the wt-bridge source directory.
fn find_bridge_source() -> anyhow::Result<PathBuf> {
    // Try relative to the current directory (development)
    let candidates = [
        PathBuf::from("wt-bridge"),
        PathBuf::from("../wt-bridge"),
        // Try from the executable location
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.join("../wt-bridge")))
            .unwrap_or_default(),
    ];

    for candidate in candidates {
        if candidate.join("Cargo.toml").exists() {
            return Ok(candidate.canonicalize()?);
        }
    }

    anyhow::bail!(
        "Could not find wt-bridge source directory. \
         Run 'wt ui setup' from the worktrunk repository root."
    )
}

/// Create a new workspace session.
fn create_workspace(session_name: &str, repo_root: &std::path::Path) -> anyhow::Result<()> {
    let layout = if is_plugin_installed() {
        Some(layout_path())
    } else {
        output::warning("Plugin not installed. Run 'wt ui setup' for full workspace support.")?;
        None
    };

    output::progress(cformat!("Creating workspace <bold>{session_name}</>..."))?;
    output::flush()?;
    create_session(session_name, repo_root, layout.as_deref())?;
    Ok(())
}

/// Handle the `wt ui` command.
///
/// Attaches to or creates a dedicated zellij session for this repository.
pub fn handle_ui() -> anyhow::Result<()> {
    // Check if zellij is available
    if !is_zellij_available() {
        return Err(worktrunk::git::GitError::Other {
            message: "zellij is not installed or not in PATH. Install from https://zellij.dev"
                .into(),
        }
        .into());
    }

    let repo = Repository::current();
    let repo_root = repo.worktree_base()?;
    let session_name = session_name_for_repo(&repo_root);

    // Detect current context
    let context = detect_context(&repo_root);

    match context {
        ZellijContext::Outside => {
            // Not in zellij - check session status
            match session_status(&session_name) {
                SessionStatus::Running => {
                    output::progress(cformat!(
                        "Attaching to workspace <bold>{session_name}</>..."
                    ))?;
                    output::flush()?;
                    attach_session(&session_name)?;
                }
                SessionStatus::Exited => {
                    // Zombie session - delete and create fresh
                    output::warning(cformat!(
                        "Cleaning up exited session <bold>{session_name}</>"
                    ))?;
                    delete_session(&session_name)?;
                    create_workspace(&session_name, &repo_root)?;
                }
                SessionStatus::NotFound => {
                    create_workspace(&session_name, &repo_root)?;
                }
            }
        }

        ZellijContext::InsideWorkspace { session_name } => {
            // Already in the correct workspace
            output::info(cformat!("Already in workspace <bold>{session_name}</>"))?;
        }

        ZellijContext::InsideOtherWorkspace {
            current_session,
            expected_session,
        } => {
            // Inside a worktrunk session, but for a different repo
            return Err(worktrunk::git::GitError::Other {
                message: format!(
                    "Inside workspace {} for a different repository (expected {}). \
                     Detach first (Ctrl+O, D), then run 'wt ui' from a normal shell",
                    current_session, expected_session
                ),
            }
            .into());
        }

        ZellijContext::InsideOtherSession { session_name } => {
            // Inside a non-worktrunk zellij session
            return Err(worktrunk::git::GitError::Other {
                message: format!(
                    "Inside zellij session '{}' (not managed by worktrunk). \
                     Detach first (Ctrl+O, D), then run 'wt ui' from a normal shell",
                    session_name
                ),
            }
            .into());
        }
    }

    Ok(())
}

/// Handle the `wt ui status` command.
///
/// Shows current workspace context and setup status for debugging.
pub fn handle_status() -> anyhow::Result<()> {
    use crate::output;
    use worktrunk::zellij::{ZellijContext, detect_context};

    let repo = worktrunk::git::Repository::current();
    let repo_root = repo.worktree_base()?;

    // Show context
    output::info("Context")?;
    match detect_context(&repo_root) {
        ZellijContext::InsideWorkspace { session_name } => {
            output::success(format!("Inside workspace: {}", session_name))?;
        }
        ZellijContext::Outside => {
            let name = worktrunk::zellij::session_name_for_repo(&repo_root);
            output::print(format!("Outside zellij (session would be: {})", name))?;
        }
        ZellijContext::InsideOtherWorkspace {
            expected_session,
            current_session,
        } => {
            output::warning(format!(
                "Inside different workspace: {} (expected: {})",
                current_session, expected_session
            ))?;
        }
        ZellijContext::InsideOtherSession { session_name } => {
            output::warning(format!("Inside non-worktrunk session: {}", session_name))?;
        }
    }

    // Show setup status
    output::blank()?;
    output::info("Setup")?;

    let plugin = plugin_path();
    let layout = layout_path();
    let config = zellij_config_path();

    // Plugin installed?
    if plugin.exists() {
        output::success(format!("Plugin: {}", plugin.display()))?;
    } else {
        output::warning("Plugin: not installed")?;
    }

    // Layout installed?
    if layout.exists() {
        output::success(format!("Layout: {}", layout.display()))?;
    } else {
        output::warning("Layout: not installed")?;
    }

    // Config has load_plugins entry?
    let config_ok = config.exists()
        && fs::read_to_string(&config)
            .map(|s| s.contains("wt-bridge.wasm"))
            .unwrap_or(false);

    if config_ok {
        output::success(format!("Config: {} (has load_plugins)", config.display()))?;
    } else if config.exists() {
        output::warning(format!(
            "Config: {} (missing load_plugins entry)",
            config.display()
        ))?;
    } else {
        output::warning("Config: not found")?;
    }

    // Show hints
    output::blank()?;
    if !plugin.exists() || !config_ok {
        output::hint("Run 'wt ui setup' to install the plugin")?;
    } else if detect_context(&repo_root) == ZellijContext::Outside {
        output::hint("Run 'wt ui' to enter the workspace")?;
    } else if matches!(
        detect_context(&repo_root),
        ZellijContext::InsideWorkspace { .. }
    ) {
        output::hint("Test with: zellij pipe --name wt -- 'select|/tmp'")?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    // Tests would require mocking zellij commands
    // For now, behavior is tested via integration tests
}
