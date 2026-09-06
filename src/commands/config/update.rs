//! Config update command.
//!
//! Computes user- and project-config migrations in memory. The default mode
//! previews and applies them atomically; output mode writes one migration
//! artifact to the named destination instead. The previous `.new` file flow
//! was removed — nothing writes to disk outside this command.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use color_print::cformat;
use worktrunk::config::{
    Approvals, ConfigFileKind, DeprecationInfo, DeprecationKind, compute_migrated_content,
    config_path, copy_approved_commands_to_approvals_file, format_deprecation_warnings,
    format_migration_diff,
};
use worktrunk::git::{Repository, resolve_input_path};
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{
    eprint, eprintln, format_bash_with_gutter, format_with_gutter, hint_message, info_message,
    print, success_message, suggest_command_in_dir, warning_message,
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
pub fn handle_config_update(yes: bool, output: Option<PathBuf>) -> anyhow::Result<()> {
    let mut candidates = Vec::new();
    let read_only = output.is_some();

    if let Some(candidate) = check_user_config()? {
        candidates.push(candidate);
    }
    if let Some(candidate) = check_project_config(read_only)? {
        candidates.push(candidate);
    }

    if let Some(output) = output {
        write_migrated_output(&output, &candidates)?;
        return Ok(());
    }

    if candidates.is_empty() {
        eprintln!("{}", info_message("No deprecated settings found"));
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
        // drops them; approvals.toml becomes the authoritative source). Abort
        // the whole update if the copy fails — rewriting config.toml first
        // would silently lose the legacy approvals.
        if candidate
            .info
            .deprecations
            .iter()
            .any(|k| matches!(k, DeprecationKind::ApprovedCommands))
            && let Some(approvals_path) =
                copy_approved_commands_to_approvals_file(&candidate.config_path)?
        {
            let filename = approvals_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            eprintln!(
                "{}",
                info_message(cformat!("Copied approved commands to <bold>{filename}</>"))
            );
        }

        worktrunk::utils::write_atomically(&candidate.config_path, &candidate.migrated)
            .with_context(|| format!("Failed to update {}", candidate.info.label()))?;
        eprintln!(
            "{}",
            success_message(format!("Updated {}", candidate.info.label().to_lowercase()))
        );
    }

    Ok(())
}

/// Write the migration artifact to a path, or to stdout when the path is `-`.
fn write_migrated_output(output: &Path, candidates: &[UpdateCandidate]) -> anyhow::Result<()> {
    let stdout = output == Path::new("-");

    if candidates.is_empty() {
        if !stdout {
            eprintln!("{}", info_message("No deprecated settings found"));
        }
        return Ok(());
    }

    if !stdout && candidates.len() > 1 {
        bail!(cformat!(
            "Cannot write <bold>user config</> and <bold>project config</> migrations to one file; use <bold>--output=-</> to inspect both or run <bold>wt config update</> to apply them in place"
        ));
    }

    for candidate in candidates {
        eprint!("{}", format_dropped_approvals_warning(candidate));
    }

    let artifact = format_migrated_output(candidates);
    if stdout {
        print!("{artifact}");
        return Ok(());
    }

    let output = resolve_input_path(output);
    worktrunk::utils::write_atomically(&output, &artifact).with_context(|| {
        format!(
            "Failed to write output @ {}",
            format_path_for_display(&output)
        )
    })?;
    Ok(())
}

/// Format the migration artifact shared by file and stdout destinations.
fn format_migrated_output(candidates: &[UpdateCandidate]) -> String {
    let mut artifact = String::new();
    let multi = candidates.len() > 1;

    for (idx, candidate) in candidates.iter().enumerate() {
        if multi {
            if idx > 0 {
                artifact.push('\n');
            }
            let _ = writeln!(
                artifact,
                "# {} ({})",
                candidate.info.label(),
                candidate.config_path.display()
            );
        }
        artifact.push_str(&candidate.migrated);
    }

    artifact
}

/// Warn that output artifacts drop `approved-commands` without preserving them.
///
/// The migration moves those arrays to `approvals.toml`, and the in-place path
/// copies them there before rewriting the config. Output mode writes no
/// `approvals.toml`, so replacing a config with its artifact would silently
/// lose every approval it named. Goes to stderr so stdout stays pipeable.
fn format_dropped_approvals_warning(candidate: &UpdateCandidate) -> String {
    if !candidate
        .info
        .deprecations
        .iter()
        .any(|k| matches!(k, DeprecationKind::ApprovedCommands))
    {
        return String::new();
    }
    let Ok(approvals) = Approvals::load_from_config_file(&candidate.config_path) else {
        return String::new();
    };
    let entries: Vec<&str> = approvals.projects().map(|(id, _)| id).collect();
    if entries.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    let plural = if entries.len() == 1 {
        "entry"
    } else {
        "entries"
    };
    let _ = writeln!(
        out,
        "{}",
        warning_message(cformat!(
            "Output config drops <bold>approved-commands</> from {} <bold>[projects]</> {plural}; --output writes no approvals.toml",
            entries.len()
        ))
    );
    let _ = writeln!(out, "{}", format_with_gutter(&entries.join("\n"), None));
    let _ = writeln!(
        out,
        "{}",
        hint_message(cformat!(
            "To migrate them to approvals.toml, run <underline>wt config update</>"
        ))
    );
    out
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
        ConfigFileKind::User,
        None,  // no repo context for user config
        false, // emit_inline_warnings — we render the diff ourselves
    )?;

    let Some(info) = result.info.filter(DeprecationInfo::has_deprecations) else {
        return Ok(None);
    };

    let migrated = compute_migrated_content(&original, ConfigFileKind::User);
    Ok(Some(UpdateCandidate {
        config_path,
        original,
        migrated,
        info,
    }))
}

fn check_project_config(read_only: bool) -> anyhow::Result<Option<UpdateCandidate>> {
    let repo = match Repository::current() {
        Ok(repo) => repo,
        Err(_) => return Ok(None),
    };

    let config_path = match repo.project_config_path()? {
        Some(path) => path,
        None => return Ok(None),
    };
    if !config_path.exists() {
        return Ok(None);
    }

    let is_linked = repo.current_worktree().is_linked().unwrap_or(true);
    let actionable = read_only || !is_linked;

    let original =
        std::fs::read_to_string(&config_path).context("Failed to read project config")?;

    let result = worktrunk::config::check_and_migrate(
        &config_path,
        &original,
        actionable,
        ConfigFileKind::Project,
        Some(&repo),
        false,
    )?;

    let Some(info) = result.info.filter(DeprecationInfo::has_deprecations) else {
        return Ok(None);
    };

    if !actionable {
        let cmd = suggest_command_in_dir(repo.repo_path()?, "config", &["update"], &[]);
        eprintln!("{}", hint_message("To update project config:"));
        eprintln!("{}", format_bash_with_gutter(&cmd));
        return Ok(None);
    }

    let migrated = compute_migrated_content(&original, ConfigFileKind::Project);
    Ok(Some(UpdateCandidate {
        config_path,
        original,
        migrated,
        info,
    }))
}
