//! oh-my-pi (`omp`) activity-hook installation.
//!
//! oh-my-pi is a Pi-derived agent that loads user hooks from its
//! profile-aware agent directory at `hooks/pre/worktrunk.ts`. earendil-works
//! Pi is a separate agent with a different loader and API — for that one see
//! [`super::pi`].

use std::path::PathBuf;

use anyhow::{Context, Result};

/// The hook source, embedded at compile time.
const HOOK_SOURCE: &str = include_str!("../../../dev/omp-hook.ts");

fn active_profile() -> Option<String> {
    let value = std::env::var("OMP_PROFILE")
        .ok()
        .or_else(|| std::env::var("PI_PROFILE").ok())?;
    let profile = value.trim();
    (!profile.is_empty() && profile != "default").then(|| profile.to_owned())
}

fn omp_agent_dir() -> Result<PathBuf> {
    if let Some(path) = std::env::var("PI_CODING_AGENT_DIR")
        .ok()
        .filter(|value| !value.is_empty())
        // Named profiles ignore `PI_CODING_AGENT_DIR` upstream, so the
        // override applies to the default profile only.
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
    Ok(omp_agent_dir()?
        .join("hooks")
        .join("pre")
        .join("worktrunk.ts"))
}

pub fn is_plugin_installed() -> bool {
    plugin_path()
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .is_some_and(|content| content == HOOK_SOURCE)
}

pub fn plugin_file_exists() -> bool {
    plugin_path().map(|p| p.exists()).unwrap_or(false)
}

pub fn handle_omp_install(yes: bool) -> Result<()> {
    let target = plugin_path()?;
    super::install_file_plugin("oh-my-pi", &target, HOOK_SOURCE, yes)
}

pub fn handle_omp_uninstall(yes: bool) -> Result<()> {
    let target = plugin_path()?;
    super::uninstall_file_plugin("oh-my-pi", &target, yes)
}
