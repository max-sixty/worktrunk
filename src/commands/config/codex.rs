//! Codex plugin marketplace management.

use anyhow::{Context, Result, bail};
use color_print::cformat;
use worktrunk::shell_exec::Cmd;
use worktrunk::styling::{eprintln, hint_message, progress_message, success_message};

use super::show::is_codex_available;
use crate::output::prompt::{PromptResponse, prompt_yes_no_preview};

const MARKETPLACE_SOURCE: &str = "max-sixty/worktrunk";
const MARKETPLACE_NAME: &str = "worktrunk";

/// Handle `wt config plugins codex install`.
pub fn handle_codex_install(yes: bool) -> Result<()> {
    require_codex_cli()?;

    if !yes {
        match prompt_yes_no_preview(
            &cformat!("Add Worktrunk plugin marketplace to <bold>Codex</>?"),
            || {
                let commands = format!("codex plugin marketplace add {MARKETPLACE_SOURCE}");
                eprintln!("{}", worktrunk::styling::format_bash_with_gutter(&commands));
            },
        )? {
            PromptResponse::Accepted => {}
            PromptResponse::Declined => return Ok(()),
        }
    }

    eprintln!("{}", progress_message("Adding Codex plugin marketplace..."));
    let output = Cmd::new("codex")
        .args(["plugin", "marketplace", "add", MARKETPLACE_SOURCE])
        .run()
        .context("Failed to run codex CLI")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("codex plugin marketplace add failed: {}", stderr.trim());
    }

    eprintln!("{}", success_message("Codex marketplace configured"));
    eprintln!(
        "{}",
        hint_message("Next, run /plugins in Codex and install Worktrunk from the marketplace")
    );
    // The Codex plugin deliberately ships no activity-marker hooks: Codex's
    // HookEventNameWire vocabulary (codex-cli 0.130.0) has no `Stop`/turn-end
    // event, so a 🤖 set on UserPromptSubmit could never return to 💬 within a
    // session. Re-add the hooks (and restore the marker hints + docs) once
    // Codex exposes a turn-end hook event. See CLAUDE.md → "Codex Plugin".

    Ok(())
}

/// Handle `wt config plugins codex uninstall`.
pub fn handle_codex_uninstall(yes: bool) -> Result<()> {
    require_codex_cli()?;

    if !yes {
        match prompt_yes_no_preview(
            &cformat!("Remove Worktrunk plugin marketplace from <bold>Codex</>?"),
            || {
                eprintln!(
                    "{}",
                    worktrunk::styling::format_bash_with_gutter(
                        "codex plugin marketplace remove worktrunk"
                    )
                );
            },
        )? {
            PromptResponse::Accepted => {}
            PromptResponse::Declined => return Ok(()),
        }
    }

    eprintln!(
        "{}",
        progress_message("Removing Codex plugin marketplace...")
    );
    let output = Cmd::new("codex")
        .args(["plugin", "marketplace", "remove", MARKETPLACE_NAME])
        .run()
        .context("Failed to run codex CLI")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("codex plugin marketplace remove failed: {}", stderr.trim());
    }

    eprintln!("{}", success_message("Codex marketplace removed"));
    eprintln!("{}", hint_message("Installed plugins are left unchanged"));

    Ok(())
}

fn require_codex_cli() -> Result<()> {
    if is_codex_available() {
        return Ok(());
    }

    bail!("codex CLI not found. Install Codex first: https://developers.openai.com/codex/cli/");
}
