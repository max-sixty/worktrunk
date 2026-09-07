//! Plugin management commands for AI coding tools.

use std::path::PathBuf;

use anyhow::{Context, bail};
use color_print::cformat;
use worktrunk::styling::{eprintln, info_message, progress_message, success_message};

use super::show::{
    claude_config_dir, is_claude_available, is_marketplace_configured, is_plugin_installed,
    is_statusline_configured,
};
use crate::output::prompt::{PromptResponse, prompt_yes_no_preview};

/// Handle `wt config plugins claude install`
pub fn handle_claude_install(yes: bool) -> anyhow::Result<()> {
    require_claude_cli()?;

    if is_plugin_installed() {
        eprintln!("{}", info_message("Plugin already installed"));
        return Ok(());
    }

    if !yes {
        match prompt_yes_no_preview(
            &cformat!("Install Worktrunk plugin for <bold>Claude Code</>?"),
            || {
                let commands = "claude plugin marketplace add max-sixty/worktrunk\nclaude plugin install worktrunk@worktrunk";
                eprintln!("{}", worktrunk::styling::format_bash_with_gutter(commands));
            },
        )? {
            PromptResponse::Accepted => {}
            PromptResponse::Declined => return Ok(()),
        }
    }

    eprintln!("{}", progress_message("Adding plugin from marketplace..."));
    super::run_plugin_cli(
        "claude",
        &["plugin", "marketplace", "add", "max-sixty/worktrunk"],
    )?;

    eprintln!("{}", progress_message("Installing plugin..."));
    super::run_plugin_cli("claude", &["plugin", "install", "worktrunk@worktrunk"])?;

    eprintln!("{}", success_message("Plugin installed"));

    Ok(())
}

/// Handle `wt config plugins claude uninstall`
pub fn handle_claude_uninstall(yes: bool) -> anyhow::Result<()> {
    require_claude_cli()?;

    // The marketplace can outlive the plugin: an uninstall that removed the
    // plugin and then failed on the marketplace leaves exactly that. Asking
    // only about the plugin would report "not installed" and exit 0 with the
    // marketplace still there and no way left to finish the job, so the early
    // return needs both halves gone. Only a confident `Some(false)` counts as
    // gone, for the reason `run_plugin_removal` gives.
    let plugin_installed = is_plugin_installed();
    if !plugin_installed && is_marketplace_configured() == Some(false) {
        eprintln!("{}", info_message("Plugin not installed"));
        return Ok(());
    }

    // The marketplace removal always runs, so its own tolerance decides
    // whether an absent marketplace is a failure. Only the plugin step is
    // conditional, and the preview says so rather than naming a command that
    // will not run.
    let mut commands = Vec::new();
    if plugin_installed {
        commands.push("claude plugin uninstall worktrunk@worktrunk");
    }
    commands.push("claude plugin marketplace remove worktrunk");

    if !yes {
        match prompt_yes_no_preview(
            &cformat!("Uninstall Worktrunk plugin from <bold>Claude Code</>?"),
            || {
                eprintln!(
                    "{}",
                    worktrunk::styling::format_bash_with_gutter(&commands.join("\n"))
                );
            },
        )? {
            PromptResponse::Accepted => {}
            PromptResponse::Declined => return Ok(()),
        }
    }

    if plugin_installed {
        eprintln!("{}", progress_message("Uninstalling plugin..."));
        super::run_plugin_cli("claude", &["plugin", "uninstall", "worktrunk@worktrunk"])?;
    }

    eprintln!(
        "{}",
        progress_message("Removing Claude Code plugin marketplace...")
    );
    super::run_plugin_removal(
        "claude",
        &["plugin", "marketplace", "remove", "worktrunk"],
        is_marketplace_configured,
    )?;

    eprintln!("{}", success_message("Plugin & marketplace removed"));

    Ok(())
}

/// Handle `wt config plugins claude install-statusline`
pub fn handle_claude_install_statusline(yes: bool) -> anyhow::Result<()> {
    if is_statusline_configured() {
        eprintln!("{}", info_message("Statusline already configured"));
        return Ok(());
    }

    let settings_path = require_settings_path()?;

    if !yes {
        match prompt_yes_no_preview(
            &cformat!("Configure statusline for <bold>Claude Code</>?"),
            || {
                eprintln!(
                    "{}",
                    worktrunk::styling::format_with_gutter(
                        r#"{
  "statusLine": {
    "type": "command",
    "command": "wt list statusline --format=claude-code"
  }
}"#,
                        None,
                    )
                );
            },
        )? {
            PromptResponse::Accepted => {}
            PromptResponse::Declined => return Ok(()),
        }
    }

    // Ensure parent directory exists
    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent).context("Failed to create Claude Code config directory")?;
    }

    // Read existing settings or start with empty object
    let mut settings: serde_json::Map<String, serde_json::Value> = if settings_path.exists() {
        let content =
            std::fs::read_to_string(&settings_path).context("Failed to read settings.json")?;
        if content.trim().is_empty() {
            serde_json::Map::new()
        } else {
            serde_json::from_str(&content).context("Failed to parse settings.json")?
        }
    } else {
        serde_json::Map::new()
    };

    // Merge in the statusLine config
    settings.insert(
        "statusLine".to_string(),
        serde_json::json!({
            "type": "command",
            "command": "wt list statusline --format=claude-code"
        }),
    );

    let json = serde_json::to_string_pretty(&settings).context("Failed to serialize settings")?;
    worktrunk::utils::write_atomically(&settings_path, &(json + "\n"))
        .context("Failed to write settings.json")?;

    eprintln!("{}", success_message("Statusline configured"));

    Ok(())
}

/// Get the path to Claude Code's `settings.json` (under `CLAUDE_CONFIG_DIR` or
/// `~/.claude`), or bail if the config directory can't be determined
fn require_settings_path() -> anyhow::Result<PathBuf> {
    let Some(config_dir) = claude_config_dir() else {
        bail!("Could not determine Claude Code config directory");
    };
    Ok(config_dir.join("settings.json"))
}

/// Bail if `claude` CLI is not available
fn require_claude_cli() -> anyhow::Result<()> {
    if is_claude_available() {
        return Ok(());
    }
    bail!("claude CLI not found. Install Claude Code first: https://code.claude.com/docs/en/setup");
}
