//! Pi / oh-my-pi activity-hook installation.
//!
//! Installs the embedded hook factory under Pi's profile-aware user agent
//! directory at `hooks/pre/worktrunk.ts`.

use std::path::PathBuf;

use anyhow::{Context, Result};
use color_print::cformat;
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{eprintln, hint_message, info_message, success_message};

use crate::output::prompt::{PromptResponse, prompt_yes_no_preview};

const PLUGIN_SOURCE: &str = include_str!("../../../dev/pi-plugin.ts");

fn active_profile() -> Option<String> {
    let value = std::env::var("OMP_PROFILE")
        .ok()
        .or_else(|| std::env::var("PI_PROFILE").ok())?;
    let profile = value.trim();
    (!profile.is_empty() && profile != "default").then(|| profile.to_owned())
}

fn pi_agent_dir() -> Result<PathBuf> {
    if let Some(path) = std::env::var("PI_CODING_AGENT_DIR")
        .ok()
        .filter(|value| !value.is_empty())
        .filter(|_| active_profile().is_none())
    {
        return Ok(PathBuf::from(path));
    }

    let home = worktrunk::path::home_dir().context("Could not determine home directory")?;
    let config_dir = std::env::var("PI_CONFIG_DIR")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| ".omp".to_owned());
    let root = home.join(config_dir);

    Ok(match active_profile() {
        Some(profile) => root.join("profiles").join(profile).join("agent"),
        None => root.join("agent"),
    })
}

pub fn plugin_path() -> Result<PathBuf> {
    Ok(pi_agent_dir()?
        .join("hooks")
        .join("pre")
        .join("worktrunk.ts"))
}

fn confirm_or_yes(yes: bool, prompt: &str, preview: impl Fn()) -> Result<bool> {
    Ok(yes || prompt_yes_no_preview(prompt, preview)? == PromptResponse::Accepted)
}

pub fn handle_pi_install(yes: bool) -> Result<()> {
    let target = plugin_path()?;
    let target_display = format_path_for_display(&target);

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
    if !confirm_or_yes(
        yes,
        &cformat!("{action} Pi plugin @ <bold>{target_display}</>?"),
        preview,
    )? {
        return Ok(());
    }

    let parent = target
        .parent()
        .context("Plugin path has no parent directory")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create directory {}", parent.display()))?;
    worktrunk::utils::write_atomically(&target, PLUGIN_SOURCE)
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

pub fn handle_pi_uninstall(yes: bool) -> Result<()> {
    let target = plugin_path()?;
    let target_display = format_path_for_display(&target);

    if !target.exists() {
        eprintln!("{}", info_message("Plugin not installed"));
        return Ok(());
    }

    let preview_msg = info_message(cformat!("Would remove <bold>{target_display}</>"));
    let preview = || eprintln!("{}", preview_msg);
    if !confirm_or_yes(
        yes,
        &cformat!("Remove Pi plugin @ <bold>{target_display}</>?"),
        preview,
    )? {
        return Ok(());
    }

    std::fs::remove_file(&target)
        .with_context(|| format!("Failed to remove plugin @ {target_display}"))?;
    eprintln!(
        "{}",
        success_message(cformat!("Plugin removed @ <bold>{target_display}</>"))
    );
    Ok(())
}
