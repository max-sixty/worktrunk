//! Config update command.
//!
//! Computes user- and project-config migrations in memory. The default mode
//! previews and applies them atomically; output mode writes one migration
//! artifact to the named destination instead. No other command writes a
//! migration to disk.

use std::fmt::Write as _;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use color_print::cformat;
use worktrunk::config::{
    ConfigFileKind, DeprecationInfo, DeprecationKind, compute_migrated_content, config_path,
    copy_approved_commands_to_approvals_file, ensure_config_parses, format_deprecation_warnings,
    format_migration_diff_block,
};
use worktrunk::git::{Repository, resolve_input_path};
use worktrunk::path::{format_path_for_display, paths_match};
use worktrunk::styling::{
    eprint, eprintln, format_bash_with_gutter, hint_message, info_message, print, success_message,
    suggest_command_in_dir, warning_message,
};
use worktrunk::utils::{write_atomically, write_new_atomically};

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

impl UpdateCandidate {
    /// Compute the migration, refusing one whose content wt could not load.
    ///
    /// Checking here rather than at the write covers both destinations — the
    /// in-place update and `--output` — and fails before the preview asks for
    /// confirmation.
    fn new(config_path: PathBuf, original: String, info: DeprecationInfo) -> anyhow::Result<Self> {
        let migrated = compute_migrated_content(&original);
        ensure_config_parses(&migrated)
            .with_context(|| format!("Failed to migrate {}", info.label().to_lowercase()))?;
        Ok(Self {
            config_path,
            original,
            migrated,
            info,
        })
    }
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
        write_migrated_output(&output, &candidates, yes)?;
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
        // Separate the prompt from the previews above; prompt_yes_no_preview
        // emits no leading blank of its own.
        eprintln!();
        match prompt_yes_no_preview("Apply updates?", || {})? {
            PromptResponse::Accepted => {}
            PromptResponse::Declined => {
                eprintln!("{}", info_message("Update cancelled"));
                return Ok(());
            }
        }
    }

    // A preview authorizes migrating the content it was computed from. An edit
    // that completes while the prompt is open would be replaced by that earlier
    // snapshot, so re-read each config and fail rather than publish a stale
    // migration. Every candidate is checked before any is written, so one
    // superseded config can't leave the other half-applied. The window between
    // this check and the rename stays open to a non-cooperating editor, the same
    // boundary the shell-config writes accept.
    for candidate in &candidates {
        let current = std::fs::read_to_string(&candidate.config_path).with_context(|| {
            format!(
                "Failed to re-read {}",
                candidate.info.label().to_lowercase()
            )
        })?;
        if current != candidate.original {
            bail!(cformat!(
                "{} changed @ <bold>{}</> since the preview; to migrate the current contents, re-run <bold>wt config update</>",
                candidate.info.label(),
                format_path_for_display(&candidate.config_path)
            ));
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

        write_atomically(&candidate.config_path, &candidate.migrated)
            .with_context(|| format!("Failed to update {}", candidate.info.label()))?;
        eprintln!(
            "{}",
            success_message(format!("Updated {}", candidate.info.label().to_lowercase()))
        );
    }

    Ok(())
}

/// Write the migration artifact to a path, or to stdout when the path is `-`.
///
/// The two destinations differ in what they can carry, so they run as separate
/// paths rather than one path testing `-` at each step. Stdout labels and
/// concatenates every candidate and needs no confirmation — the artifact is
/// right there. A file takes exactly one migration, so the checks below and
/// the success line can name the config it came from.
///
/// A file destination replaces nothing without consent. The config being
/// migrated is refused outright: rewriting it is the in-place update's job,
/// which previews the diff, re-reads the file after the prompt, and moves
/// `approved-commands` to approvals.toml. Any other existing file is replaced
/// only after a prompt, which `--yes` answers in advance; with no terminal to
/// prompt on, the command fails instead. A destination that was absent is
/// created without clobbering, so a file that appears before the write lands
/// survives it.
fn write_migrated_output(
    output: &Path,
    candidates: &[UpdateCandidate],
    yes: bool,
) -> anyhow::Result<()> {
    if output == Path::new("-") {
        for candidate in candidates {
            eprint!("{}", format_dropped_approvals_warning(candidate));
        }
        print!("{}", format_migrated_output(candidates));
        return Ok(());
    }

    if candidates.is_empty() {
        eprintln!("{}", info_message("No deprecated settings found"));
        return Ok(());
    }

    let [candidate] = candidates else {
        bail!(cformat!(
            "Cannot write <bold>user config</> and <bold>project config</> migrations to one file; to inspect both, use <bold>--output=-</>; to apply them in place, run <bold>wt config update</>"
        ));
    };

    let output = resolve_input_path(output);
    let label = candidate.info.label().to_lowercase();
    if paths_match(&output, &candidate.config_path) {
        bail!(cformat!(
            "Cannot overwrite <bold>{label}</> with <bold>--output</>; to apply the migration in place, run <bold>wt config update</>"
        ));
    }
    let display_path = format_path_for_display(&output);

    let approvals_warning = format_dropped_approvals_warning(candidate);
    eprint!("{approvals_warning}");

    let replace = output.exists();
    if replace && !yes {
        if !std::io::stdin().is_terminal() {
            bail!(cformat!(
                "{display_path} already exists; to overwrite it with the {label} migration, add <bold>--yes</>"
            ));
        }
        if !approvals_warning.is_empty() {
            eprintln!();
        }
        let prompt = format!("Overwrite {display_path} with the {label} migration?");
        match prompt_yes_no_preview(&prompt, || {})? {
            PromptResponse::Accepted => {}
            PromptResponse::Declined => {
                eprintln!("{}", info_message("Update cancelled"));
                return Ok(());
            }
        }
    }

    let artifact = format_migrated_output(candidates);
    let written = if replace {
        write_atomically(&output, &artifact)
    } else {
        write_new_atomically(&output, &artifact)
    };
    written.with_context(|| format!("Failed to write output @ {display_path}"))?;
    eprintln!(
        "{}",
        success_message(format!("Wrote {label} migration @ {display_path}"))
    );
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

fn format_dropped_approvals_warning(candidate: &UpdateCandidate) -> String {
    if !drops_approved_commands(candidate) {
        return String::new();
    }
    format!(
        "{}\n",
        warning_message(cformat!(
            "Output omits deprecated <bold>approved-commands</>; to migrate them to approvals.toml, run <bold>wt config update</>"
        ))
    )
}

fn drops_approved_commands(candidate: &UpdateCandidate) -> bool {
    candidate
        .info
        .deprecations
        .iter()
        .any(|kind| matches!(kind, DeprecationKind::ApprovedCommands))
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
    out.push_str(&format_migration_diff_block(
        &candidate.original,
        &candidate.migrated,
        &label,
    ));
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

    UpdateCandidate::new(config_path, original, info).map(Some)
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

    UpdateCandidate::new(config_path, original, info).map(Some)
}
