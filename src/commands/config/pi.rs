//! Pi (earendil-works/pi) activity-extension installation.
//!
//! Installs the embedded extension factory under Pi's user agent directory at
//! `extensions/worktrunk.ts`, which Pi scans on startup. `$PI_CODING_AGENT_DIR`
//! replaces the default `~/.pi/agent` outright — Pi has no profile concept.
//!
//! oh-my-pi (`omp`) is a separate agent, with its own config root and hook
//! API; it has its own command — see [`super::omp`].

use std::path::PathBuf;

use anyhow::{Context, Result};
use color_print::cformat;
use worktrunk::styling::{eprintln, hint_message};

/// The extension source, embedded at compile time.
const EXTENSION_SOURCE: &str = include_str!("../../../dev/pi-extension.ts");

fn pi_agent_dir() -> Result<PathBuf> {
    if let Some(path) = std::env::var("PI_CODING_AGENT_DIR")
        .ok()
        .filter(|value| !value.is_empty())
    {
        return Ok(PathBuf::from(path));
    }

    let home = worktrunk::path::home_dir().context("Could not determine home directory")?;
    Ok(home.join(".pi").join("agent"))
}

pub fn plugin_path() -> Result<PathBuf> {
    Ok(pi_agent_dir()?.join("extensions").join("worktrunk.ts"))
}

pub fn is_plugin_installed() -> bool {
    plugin_path()
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .is_some_and(|content| content == EXTENSION_SOURCE)
}

pub fn plugin_file_exists() -> bool {
    plugin_path().map(|p| p.exists()).unwrap_or(false)
}

pub fn handle_pi_install(yes: bool) -> Result<()> {
    let target = plugin_path()?;
    super::install_file_plugin("Pi", &target, EXTENSION_SOURCE, yes)
}

pub fn handle_pi_uninstall(yes: bool) -> Result<()> {
    let target = plugin_path()?;
    // Until this split, `wt config plugins pi uninstall` removed the oh-my-pi
    // hook. Someone repeating that command finds nothing at Pi's path, so the
    // bare "Plugin not installed" reads as a completed removal while their
    // hook sits untouched under oh-my-pi. Name the command that removes it.
    let omp_hook_left_behind = !target.exists() && super::omp::plugin_file_exists();
    super::uninstall_file_plugin("Pi", &target, yes)?;
    if omp_hook_left_behind {
        eprintln!(
            "{}",
            hint_message(cformat!(
                "An oh-my-pi hook is installed; to remove it, run <underline>wt config plugins omp uninstall</>"
            ))
        );
    }
    Ok(())
}
