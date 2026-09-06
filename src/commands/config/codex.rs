//! Codex plugin marketplace management.

use anyhow::{Result, bail};
use color_print::cformat;
use worktrunk::styling::{
    eprintln, hint_message, progress_message, success_message, warning_message,
};

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
    // `codex plugin remove` fails when no Worktrunk plugin is installed — the
    // state an install that stopped at the marketplace leaves behind, which is
    // what this command did before it installed the plugin too. Removing the
    // marketplace is what uninstall has always done, so a failed plugin
    // removal warns rather than aborting before that happens.
    let plugin_removed =
        match super::run_plugin_cli("codex", &["plugin", "remove", PLUGIN_SELECTOR]) {
            Ok(()) => true,
            Err(err) => {
                log::debug!("codex plugin remove failed: {err:#}");
                eprintln!(
                    "{}",
                    warning_message(
                        "Worktrunk plugin not removed from Codex; removing the marketplace anyway"
                    )
                );
                eprintln!(
                    "{}",
                    hint_message(cformat!(
                        "To see the error, run <underline>codex plugin remove {PLUGIN_SELECTOR}</>"
                    ))
                );
                false
            }
        };

    eprintln!(
        "{}",
        progress_message("Removing Codex plugin marketplace...")
    );
    super::run_plugin_cli(
        "codex",
        &["plugin", "marketplace", "remove", MARKETPLACE_NAME],
    )?;

    eprintln!(
        "{}",
        success_message(if plugin_removed {
            "Codex plugin & marketplace removed"
        } else {
            "Codex marketplace removed"
        })
    );

    Ok(())
}

fn require_codex_cli() -> Result<()> {
    if is_codex_available() {
        return Ok(());
    }

    bail!("codex CLI not found. Install Codex first: https://developers.openai.com/codex/cli/");
}
