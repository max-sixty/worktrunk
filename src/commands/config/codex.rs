//! Codex plugin and marketplace management.

use anyhow::{Result, bail};
use color_print::cformat;
use worktrunk::styling::{eprintln, hint_message, progress_message, success_message};

use super::show::is_codex_available;
use crate::output::prompt::{PromptResponse, prompt_yes_no_preview};

const MARKETPLACE_SOURCE: &str = "max-sixty/worktrunk";
const MARKETPLACE_NAME: &str = "worktrunk";
/// `PLUGIN@MARKETPLACE` selectors `codex plugin add` / `remove` take.
///
/// The activity-marker hooks ship as their own plugin. Codex 0.164+ binds hook
/// approval to a fingerprint of the whole plugin directory, so hooks bundled
/// with the skills needed another review after every skill or docs change.
/// The hooks-only plugin's fingerprint moves only when its hook commands or
/// shims do.
const PLUGIN_SELECTOR: &str = "worktrunk@worktrunk";
const MARKERS_PLUGIN_SELECTOR: &str = "worktrunk-markers@worktrunk";

/// Handle `wt config plugins codex install`.
pub fn handle_codex_install(yes: bool) -> Result<()> {
    require_codex_cli()?;

    if !yes {
        match prompt_yes_no_preview(
            &cformat!("Install Worktrunk plugins for <bold>Codex</>?"),
            || {
                let commands = format!(
                    "codex plugin marketplace add {MARKETPLACE_SOURCE}\ncodex plugin add {PLUGIN_SELECTOR}\ncodex plugin add {MARKERS_PLUGIN_SELECTOR}"
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

    eprintln!("{}", progress_message("Installing plugins..."));
    super::run_plugin_cli("codex", &["plugin", "add", PLUGIN_SELECTOR])?;
    super::run_plugin_cli("codex", &["plugin", "add", MARKERS_PLUGIN_SELECTOR])?;

    eprintln!("{}", success_message("Codex plugins installed"));
    // The markers plugin ships activity-marker hooks inline in its manifest
    // (`hooks` key in plugins/worktrunk-markers/.codex-plugin/plugin.json),
    // using `Stop` to return 🤖 → 💬 and `SessionEnd` to clear the marker.
    // See plugins/worktrunk/AGENTS.md → "Plugin layout".
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
            &cformat!("Uninstall Worktrunk plugins from <bold>Codex</>?"),
            || {
                let commands = format!(
                    "codex plugin remove {MARKERS_PLUGIN_SELECTOR}\ncodex plugin remove {PLUGIN_SELECTOR}\ncodex plugin marketplace remove {MARKETPLACE_NAME}"
                );
                eprintln!("{}", worktrunk::styling::format_bash_with_gutter(&commands));
            },
        )? {
            PromptResponse::Accepted => {}
            PromptResponse::Declined => return Ok(()),
        }
    }

    // `codex plugin remove` exits 0 when the plugin is already gone, so these
    // steps need none of the tolerance the marketplace removal below does.
    // That also covers installs from before the markers plugin existed.
    eprintln!("{}", progress_message("Uninstalling plugins..."));
    super::run_plugin_cli("codex", &["plugin", "remove", MARKERS_PLUGIN_SELECTOR])?;
    super::run_plugin_cli("codex", &["plugin", "remove", PLUGIN_SELECTOR])?;

    eprintln!(
        "{}",
        progress_message("Removing Codex plugin marketplace...")
    );
    super::run_plugin_removal(
        "codex",
        &["plugin", "marketplace", "remove", MARKETPLACE_NAME],
        is_marketplace_configured,
    )?;

    eprintln!("{}", success_message("Codex plugins & marketplace removed"));

    Ok(())
}

fn require_codex_cli() -> Result<()> {
    if is_codex_available() {
        return Ok(());
    }

    bail!("codex CLI not found. Install Codex first: https://developers.openai.com/codex/cli/");
}

/// Whether Codex lists the worktrunk marketplace, or `None` where its answer
/// cannot be read.
///
/// `codex plugin marketplace list --json` nests its entries under
/// `marketplaces` where Claude Code prints a bare array.
///
/// Reading `config.toml` instead could not claim as much. `codex plugin
/// marketplace remove` deletes the whole `marketplaces` key along with the
/// last entry under it, so a config whose key had been renamed looked exactly
/// like one a successful removal had just emptied, and the only reading that
/// let a second `uninstall` succeed was the one that reported every genuine
/// failure as `Codex plugins & marketplace removed`. Codex's own list
/// separates the two.
///
/// It also drops wt's copy of Codex's `CODEX_HOME` resolution. The child
/// reads the variable itself, so a rule about where Codex keeps its config is
/// no longer duplicated here to drift from the real one.
pub(super) fn is_marketplace_configured() -> Option<bool> {
    let listed = super::harness_listing("codex", &["plugin", "marketplace", "list", "--json"])?;
    super::listing_names(
        listed.get("marketplaces")?.as_array()?,
        "name",
        MARKETPLACE_NAME,
    )
}
