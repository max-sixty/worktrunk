//! Plugin management commands for AI coding tools.

use std::path::PathBuf;

use anyhow::{Context, bail};
use color_print::cformat;
use worktrunk::shell_exec::Cmd;
use worktrunk::styling::{eprintln, info_message, progress_message, success_message};

use super::show::{home_dir, is_claude_available, is_plugin_installed, is_statusline_configured};
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
    let output = Cmd::new("claude")
        .args(["plugin", "marketplace", "add", "max-sixty/worktrunk"])
        .run()
        .context("Failed to run claude CLI")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("claude plugin marketplace add failed: {}", stderr.trim());
    }

    eprintln!("{}", progress_message("Installing plugin..."));
    let output = Cmd::new("claude")
        .args(["plugin", "install", "worktrunk@worktrunk"])
        .run()
        .context("Failed to run claude CLI")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("claude plugin install failed: {}", stderr.trim());
    }

    eprintln!("{}", success_message("Plugin installed"));

    Ok(())
}

/// Handle `wt config plugins claude uninstall`
pub fn handle_claude_uninstall(yes: bool) -> anyhow::Result<()> {
    require_claude_cli()?;

    if !is_plugin_installed() {
        eprintln!("{}", info_message("Plugin not installed"));
        return Ok(());
    }

    if !yes {
        match prompt_yes_no_preview(
            &cformat!("Uninstall Worktrunk plugin from <bold>Claude Code</>?"),
            || {
                eprintln!(
                    "{}",
                    worktrunk::styling::format_bash_with_gutter(
                        "claude plugin uninstall worktrunk@worktrunk"
                    )
                );
            },
        )? {
            PromptResponse::Accepted => {}
            PromptResponse::Declined => return Ok(()),
        }
    }

    eprintln!("{}", progress_message("Uninstalling plugin..."));
    let output = Cmd::new("claude")
        .args(["plugin", "uninstall", "worktrunk@worktrunk"])
        .run()
        .context("Failed to run claude CLI")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("claude plugin uninstall failed: {}", stderr.trim());
    }

    eprintln!("{}", success_message("Plugin uninstalled"));

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
        std::fs::create_dir_all(parent).context("Failed to create .claude directory")?;
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
    std::fs::write(&settings_path, json + "\n").context("Failed to write settings.json")?;

    eprintln!("{}", success_message("Statusline configured"));

    Ok(())
}

/// Get the path to `~/.claude/settings.json`, or bail if home dir is unavailable
fn require_settings_path() -> anyhow::Result<PathBuf> {
    let Some(home) = home_dir() else {
        bail!("Could not determine home directory");
    };
    Ok(home.join(".claude/settings.json"))
}

/// Bail if `claude` CLI is not available
fn require_claude_cli() -> anyhow::Result<()> {
    if is_claude_available() {
        return Ok(());
    }
    bail!(
        "claude CLI not found. Install Claude Code first: https://docs.anthropic.com/en/docs/claude-code/overview"
    );
}
