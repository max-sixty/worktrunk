//! Codex plugin and marketplace management.

use anyhow::{Result, bail};
use color_print::cformat;
use worktrunk::styling::{eprintln, hint_message, progress_message, success_message};

use super::show::is_codex_available;
use crate::output::prompt::{PromptResponse, prompt_yes_no_preview};

const MARKETPLACE_SOURCE: &str = "max-sixty/worktrunk";
const MARKETPLACE_NAME: &str = "worktrunk";
/// `PLUGIN@MARKETPLACE` selector `codex plugin add` / `remove` take.
const PLUGIN_SELECTOR: &str = "worktrunk@worktrunk";

/// Handle `wt config plugins codex install`.
pub fn handle_codex_install(yes: bool) -> Result<()> {
    require_codex_cli()?;

    if !yes {
        match prompt_yes_no_preview(
            &cformat!("Install Worktrunk plugin for <bold>Codex</>?"),
            || {
                let commands = format!(
                    "codex plugin marketplace add {MARKETPLACE_SOURCE}\ncodex plugin add {PLUGIN_SELECTOR}"
                );
                eprintln!("{}", worktrunk::styling::format_bash_with_gutter(&commands));
            },
        )? {
            PromptResponse::Accepted => {}
            PromptResponse::Declined => return Ok(()),
        }
    }

    eprintln!("{}", progress_message("Adding Codex plugin marketplace..."));
    super::run_plugin_cli(
        "codex",
        &["plugin", "marketplace", "add", MARKETPLACE_SOURCE],
    )?;

    eprintln!("{}", progress_message("Installing plugin..."));
    super::run_plugin_cli("codex", &["plugin", "add", PLUGIN_SELECTOR])?;

    eprintln!("{}", success_message("Codex plugin installed"));
    // The Codex plugin ships activity-marker hooks inline in its manifest
    // (`hooks` key in .codex-plugin/plugin.json), using `Stop` to return
    // 🤖 → 💬 and `SessionEnd` to clear the marker. See CLAUDE.md → "Plugin
    // Layout".
    eprintln!(
        "{}",
        hint_message(cformat!(
            "Activity markers appear in <underline>wt list</> once a Codex session runs"
        ))
    );

    Ok(())
}

/// Handle `wt config plugins codex uninstall`.
pub fn handle_codex_uninstall(yes: bool) -> Result<()> {
    require_codex_cli()?;

    if !yes {
        match prompt_yes_no_preview(
            &cformat!("Uninstall Worktrunk plugin from <bold>Codex</>?"),
            || {
                let commands = format!(
                    "codex plugin remove {PLUGIN_SELECTOR}\ncodex plugin marketplace remove {MARKETPLACE_NAME}"
                );
                eprintln!("{}", worktrunk::styling::format_bash_with_gutter(&commands));
            },
        )? {
            PromptResponse::Accepted => {}
            PromptResponse::Declined => return Ok(()),
        }
    }

    eprintln!("{}", progress_message("Uninstalling plugin..."));
    super::run_plugin_cli("codex", &["plugin", "remove", PLUGIN_SELECTOR])?;

    eprintln!(
        "{}",
        progress_message("Removing Codex plugin marketplace...")
    );
    super::run_plugin_cli(
        "codex",
        &["plugin", "marketplace", "remove", MARKETPLACE_NAME],
    )?;

    eprintln!("{}", success_message("Codex plugin & marketplace removed"));

    Ok(())
}

fn require_codex_cli() -> Result<()> {
    if is_codex_available() {
        return Ok(());
    }

    bail!("codex CLI not found. Install Codex first: https://developers.openai.com/codex/cli/");
}
