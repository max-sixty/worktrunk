//! Config update command.
//!
//! Updates deprecated settings in user and project config files.

use std::fmt::Write as _;
use std::path::PathBuf;

use anyhow::Context;
use worktrunk::config::{
    DeprecationInfo, format_deprecation_warnings, format_migration_diff, get_config_path,
};
use worktrunk::git::Repository;
use worktrunk::styling::{eprintln, info_message, success_message};

use crate::output::prompt::{PromptResponse, prompt_yes_no_preview};

/// A config file that needs updating.
struct UpdateCandidate {
    /// Path to the original config file
    config_path: PathBuf,
    /// Path to the generated .new file
    new_path: PathBuf,
    /// Deprecation info for display
    info: DeprecationInfo,
}

/// Handle the `wt config update` command.
pub fn handle_config_update(yes: bool) -> anyhow::Result<()> {
    let mut candidates = Vec::new();

    // Check user config
    if let Some(candidate) = check_user_config()? {
        candidates.push(candidate);
    }

    // Check project config (if in a git repo)
    if let Some(candidate) = check_project_config()? {
        candidates.push(candidate);
    }

    if candidates.is_empty() {
        eprintln!("{}", info_message("No deprecated settings found"));
        return Ok(());
    }

    // Show what will be updated (warnings + diffs)
    for candidate in &candidates {
        eprint!("{}", format_update_preview(&candidate.info));
    }

    // Confirm unless --yes
    if !yes {
        let prompt_text = "Apply updates?".to_string();
        match prompt_yes_no_preview(&prompt_text, || {})? {
            PromptResponse::Accepted => {}
            PromptResponse::Declined => {
                eprintln!("{}", info_message("Update cancelled"));
                return Ok(());
            }
        }
    }

    // Apply updates
    for candidate in &candidates {
        std::fs::rename(&candidate.new_path, &candidate.config_path)
            .with_context(|| format!("Failed to update {}", candidate.info.label))?;
        eprintln!(
            "{}",
            success_message(format!("Updated {}", candidate.info.label.to_lowercase()))
        );
    }

    // Clear deprecation hint if we're in a repo
    if let Ok(repo) = Repository::current() {
        let _ = repo.clear_hint("deprecated-config");
    }

    Ok(())
}

/// Format update preview for display.
///
/// Shows deprecation warnings and diff, but omits the "mv" apply hint
/// since `config update` will apply automatically.
fn format_update_preview(info: &DeprecationInfo) -> String {
    let mut out = format_deprecation_warnings(info);

    // Show diff (without the "mv" apply hint that format_deprecation_details adds)
    if let Some(new_path) = &info.migration_path
        && let Some(diff) = format_migration_diff(&info.config_path, new_path)
    {
        let _ = writeln!(out, "{diff}");
    }

    out
}

/// Check user config for deprecations and generate .new file if needed.
fn check_user_config() -> anyhow::Result<Option<UpdateCandidate>> {
    let config_path = match get_config_path() {
        Some(path) => path,
        None => return Ok(None),
    };
    if !config_path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(&config_path).context("Failed to read user config")?;

    // Use check_and_migrate in silent mode (show_brief_warning=false) which:
    // - Detects deprecations
    // - Copies approved-commands to approvals.toml
    // - Writes the .new migration file
    let info = match worktrunk::config::check_and_migrate(
        &config_path,
        &content,
        true, // warn_and_migrate (write .new file)
        "User config",
        None,  // no repo context for user config
        false, // silent mode
    )? {
        Some(info) if info.has_deprecations() => info,
        _ => return Ok(None),
    };

    let new_path = match &info.migration_path {
        Some(path) => path.clone(),
        None => anyhow::bail!("Failed to write migration file for user config"),
    };

    Ok(Some(UpdateCandidate {
        config_path,
        new_path,
        info,
    }))
}

/// Check project config for deprecations and generate .new file if needed.
fn check_project_config() -> anyhow::Result<Option<UpdateCandidate>> {
    let repo = match Repository::current() {
        Ok(repo) => repo,
        Err(_) => return Ok(None),
    };

    let root = match repo.current_worktree().root() {
        Ok(root) => root,
        Err(_) => return Ok(None),
    };

    let config_path = root.join(".config").join("wt.toml");
    if !config_path.exists() {
        return Ok(None);
    }

    // Only update from main worktree (linked worktrees share the config)
    let is_linked = repo.current_worktree().is_linked().unwrap_or(true);
    if is_linked {
        eprintln!(
            "{}",
            info_message("Project config update must run from main worktree")
        );
        return Ok(None);
    }

    let content = std::fs::read_to_string(&config_path).context("Failed to read project config")?;

    let info = match worktrunk::config::check_and_migrate(
        &config_path,
        &content,
        true, // warn_and_migrate
        "Project config",
        Some(&repo),
        false, // silent mode
    )? {
        Some(info) if info.has_deprecations() => info,
        _ => return Ok(None),
    };

    let new_path = match &info.migration_path {
        Some(path) => path.clone(),
        None => anyhow::bail!("Failed to write migration file for project config"),
    };

    Ok(Some(UpdateCandidate {
        config_path,
        new_path,
        info,
    }))
}
