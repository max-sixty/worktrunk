//! Plugin management commands for AI coding tools.

use anyhow::{Context, bail};
use color_print::cformat;
use worktrunk::shell_exec::Cmd;
use worktrunk::styling::{eprintln, info_message, progress_message, success_message};

use super::show::{is_claude_available, is_plugin_installed};
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

/// Bail if `claude` CLI is not available
fn require_claude_cli() -> anyhow::Result<()> {
    if is_claude_available() {
        return Ok(());
    }
    bail!(
        "claude CLI not found. Install Claude Code first: https://docs.anthropic.com/en/docs/claude-code/overview"
    );
}
