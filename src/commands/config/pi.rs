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
    super::uninstall_file_plugin("Pi", &target, yes)
}
