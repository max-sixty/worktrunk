//! Plugin management commands for AI coding tools.

use std::path::PathBuf;

use anyhow::{Context, bail};
use color_print::cformat;
use worktrunk::styling::{eprintln, info_message, progress_message, success_message};

use super::show::{claude_config_dir, is_claude_available, is_statusline_configured};
use crate::output::prompt::{PromptResponse, prompt_yes_no_preview};

const MARKETPLACE_SOURCE: &str = "max-sixty/worktrunk";
const MARKETPLACE_NAME: &str = "worktrunk";
/// `PLUGIN@MARKETPLACE` selector `claude plugin install` / `uninstall` take,
/// and the `id` Claude Code lists the plugin under.
const PLUGIN_SELECTOR: &str = "worktrunk@worktrunk";

/// Handle `wt config plugins claude install`
pub fn handle_claude_install(yes: bool) -> anyhow::Result<()> {
    require_claude_cli()?;

    // Only a confident `Some(true)` short-circuits. Both commands below are
    // idempotent — Claude Code answers an already-installed plugin and an
    // already-added marketplace with a success — so an answer wt could not
    // read costs a redundant run rather than a wrong one.
    if is_plugin_installed() == Some(true) {
        eprintln!("{}", info_message("Plugin already installed"));
        return Ok(());
    }

    if !yes {
        match prompt_yes_no_preview(
            &cformat!("Install Worktrunk plugin for <bold>Claude Code</>?"),
            || {
                let commands = format!(
                    "claude plugin marketplace add {MARKETPLACE_SOURCE}\nclaude plugin install {PLUGIN_SELECTOR}"
                );
                eprintln!("{}", worktrunk::styling::format_bash_with_gutter(&commands));
            },
        )? {
            PromptResponse::Accepted => {}
            PromptResponse::Declined => return Ok(()),
        }
    }

    eprintln!("{}", progress_message("Adding plugin from marketplace..."));
    super::run_plugin_cli(
        "claude",
        &["plugin", "marketplace", "add", MARKETPLACE_SOURCE],
    )?;

    eprintln!("{}", progress_message("Installing plugin..."));
    super::run_plugin_cli("claude", &["plugin", "install", PLUGIN_SELECTOR])?;

    eprintln!("{}", success_message("Plugin installed"));

    Ok(())
}

/// Handle `wt config plugins claude uninstall`
pub fn handle_claude_uninstall(yes: bool) -> anyhow::Result<()> {
    require_claude_cli()?;

    if !yes {
        match prompt_yes_no_preview(
            &cformat!("Uninstall Worktrunk plugin from <bold>Claude Code</>?"),
            || {
                let commands = format!(
                    "claude plugin uninstall {PLUGIN_SELECTOR}\nclaude plugin marketplace remove {MARKETPLACE_NAME}"
                );
                eprintln!("{}", worktrunk::styling::format_bash_with_gutter(&commands));
            },
        )? {
            PromptResponse::Accepted => {}
            PromptResponse::Declined => return Ok(()),
        }
    }

    // Both steps run unconditionally, each tolerating only the absence Claude
    // Code itself reports. Skipping a step on a reader's say-so instead would
    // put the decision before the command rather than after it, and the two
    // halves come apart: an uninstall that removed the plugin and then failed
    // on the marketplace leaves one gone and one behind.
    eprintln!("{}", progress_message("Uninstalling plugin..."));
    super::run_plugin_removal(
        "claude",
        &["plugin", "uninstall", PLUGIN_SELECTOR],
        is_plugin_installed,
    )?;

    eprintln!(
        "{}",
        progress_message("Removing Claude Code plugin marketplace...")
    );
    super::run_plugin_removal(
        "claude",
        &["plugin", "marketplace", "remove", MARKETPLACE_NAME],
        is_marketplace_configured,
    )?;

    eprintln!("{}", success_message("Plugin & marketplace removed"));

    Ok(())
}

/// Whether Claude Code lists the worktrunk plugin, or `None` where its answer
/// cannot be read.
///
/// `claude plugin list --json` prints a bare array of plugin objects whose
/// `id` is the same `PLUGIN@MARKETPLACE` selector the install and uninstall
/// commands take.
pub(super) fn is_plugin_installed() -> Option<bool> {
    let listed = super::harness_listing("claude", &["plugin", "list", "--json"])?;
    super::listing_names(listed.as_array()?, "id", PLUGIN_SELECTOR)
}

/// Whether Claude Code lists the worktrunk marketplace, or `None` where its
/// answer cannot be read.
///
/// `claude plugin marketplace list --json` prints a bare array of marketplace
/// objects where Codex nests its own under `marketplaces`.
pub(super) fn is_marketplace_configured() -> Option<bool> {
    let listed = super::harness_listing("claude", &["plugin", "marketplace", "list", "--json"])?;
    super::listing_names(listed.as_array()?, "name", MARKETPLACE_NAME)
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
