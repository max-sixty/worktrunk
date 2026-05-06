//! Config update command.
//!
//! Updates deprecated settings in user and project config files by
//! re-migrating in memory and overwriting the file. The previous `.new` file
//! flow was removed — nothing writes to disk outside this command.

use std::fmt::Write as _;
use std::path::PathBuf;

use anyhow::Context;
use worktrunk::config::{
    DeprecationInfo, compute_migrated_content, config_path,
    copy_approved_commands_to_approvals_file, format_deprecation_warnings, format_migration_diff,
};
use worktrunk::git::Repository;
use worktrunk::styling::{
    eprintln, format_bash_with_gutter, hint_message, info_message, success_message,
    suggest_command_in_dir,
};

use crate::output::prompt::{PromptResponse, prompt_yes_no_preview};

/// A config file that needs updating.
struct UpdateCandidate {
    /// Path to the config file
    config_path: PathBuf,
    /// Current on-disk content
    original: String,
    /// Migrated content to write
    migrated: String,
    /// Detected deprecations for display
    info: DeprecationInfo,
}

/// Handle the `wt config update` command.
pub fn handle_config_update(yes: bool, print: bool) -> anyhow::Result<()> {
    let mut candidates = Vec::new();

    if let Some(candidate) = check_user_config()? {
        candidates.push(candidate);
    }
    if let Some(candidate) = check_project_config()? {
        candidates.push(candidate);
    }

    if candidates.is_empty() {
        if print {
            // --print on a clean config is a no-op; stay quiet on stdout.
            return Ok(());
        }
        eprintln!("{}", info_message("No deprecated settings found"));
        return Ok(());
    }

    if print {
        // Emit migrated content to stdout. Multiple configs → separate with a
        // labeled header so the output is still parseable. `--print` is for
        // piping, so stderr stays empty.
        let multi = candidates.len() > 1;
        for (idx, candidate) in candidates.iter().enumerate() {
            if multi {
                if idx > 0 {
                    println!();
                }
                println!(
                    "# {} ({})",
                    candidate.info.label,
                    candidate.config_path.display()
                );
            }
            print!("{}", candidate.migrated);
        }
        return Ok(());
    }

    for candidate in &candidates {
        eprint!("{}", format_update_preview(candidate));
    }

    if !yes {
        match prompt_yes_no_preview("Apply updates?", || {})? {
            PromptResponse::Accepted => {}
            PromptResponse::Declined => {
                eprintln!("{}", info_message("Update cancelled"));
                return Ok(());
            }
        }
    }

    for candidate in &candidates {
        // Preserve approved-commands before rewriting config (migrated content
        // drops them; approvals.toml becomes the authoritative source).
        if candidate.info.deprecations.approved_commands
            && let Some(approvals_path) =
                copy_approved_commands_to_approvals_file(&candidate.config_path)
        {
            let filename = approvals_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            eprintln!(
                "{}",
                info_message(format!("Copied approved commands to {filename}"))
            );
        }

        std::fs::write(&candidate.config_path, &candidate.migrated)
            .with_context(|| format!("Failed to update {}", candidate.info.label))?;
        eprintln!(
            "{}",
            success_message(format!("Updated {}", candidate.info.label.to_lowercase()))
        );
    }

    Ok(())
}

/// Format update preview for display.
///
/// Renders the per-pattern deprecation warnings followed by the diff. The
/// `wt config update` hint that normally accompanies prewarm-time warnings
/// is dropped here — the prompt below the preview is the action.
fn format_update_preview(candidate: &UpdateCandidate) -> String {
    let mut out = String::new();

    out.push_str(&format_deprecation_warnings(&candidate.info));

    let label = candidate
        .config_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config".to_string());
    if let Some(diff) = format_migration_diff(&candidate.original, &candidate.migrated, &label) {
        let _ = writeln!(out, "{}", info_message("Proposed diff:"));
        let _ = writeln!(out, "{diff}");
    }
    out
}

fn check_user_config() -> anyhow::Result<Option<UpdateCandidate>> {
    let config_path = match config_path() {
        Some(path) => path,
        None => return Ok(None),
    };
    if !config_path.exists() {
        return Ok(None);
    }

    let original = std::fs::read_to_string(&config_path).context("Failed to read user config")?;

    let result = worktrunk::config::check_and_migrate(
        &config_path,
        &original,
        true, // warn_and_migrate — user config always actionable
        "User config",
        None,  // no repo context for user config
        false, // emit_inline_warnings — we render the diff ourselves
    )?;

    let Some(info) = result.info.filter(DeprecationInfo::has_deprecations) else {
        return Ok(None);
    };

    let migrated = compute_migrated_content(&original);
    Ok(Some(UpdateCandidate {
        config_path,
        original,
        migrated,
        info,
    }))
}

fn check_project_config() -> anyhow::Result<Option<UpdateCandidate>> {
    let repo = match Repository::current() {
        Ok(repo) => repo,
        Err(_) => return Ok(None),
    };

    let config_path = match repo.project_config_path() {
        Ok(Some(path)) => path,
        _ => return Ok(None),
    };
    if !config_path.exists() {
        return Ok(None);
    }

    let is_linked = repo.current_worktree().is_linked().unwrap_or(true);

    let original =
        std::fs::read_to_string(&config_path).context("Failed to read project config")?;

    let result = worktrunk::config::check_and_migrate(
        &config_path,
        &original,
        !is_linked, // only actionable from main worktree
        "Project config",
        Some(&repo),
        false,
    )?;

    let Some(info) = result.info.filter(DeprecationInfo::has_deprecations) else {
        return Ok(None);
    };

    if is_linked {
        let cmd = suggest_command_in_dir(repo.repo_path()?, "config", &["update"], &[]);
        eprintln!("{}", hint_message("To update project config:"));
        eprintln!("{}", format_bash_with_gutter(&cmd));
        return Ok(None);
    }

    let migrated = compute_migrated_content(&original);
    Ok(Some(UpdateCandidate {
        config_path,
        original,
        migrated,
        info,
    }))
}
