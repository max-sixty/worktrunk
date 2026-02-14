//! Step command handlers for jj repositories.
//!
//! jj equivalents of `step commit`, `step squash`, and `step push`.
//! Reuses helpers from [`super::handle_merge_jj`] where possible.
//!
//! `step rebase` is handled by the unified [`super::step_commands::handle_rebase`]
//! via the [`Workspace`] trait.

use std::path::Path;

use anyhow::Context;
use color_print::cformat;
use worktrunk::config::UserConfig;
use worktrunk::styling::{eprintln, progress_message, success_message};
use worktrunk::workspace::{JjWorkspace, Workspace};

use super::step_commands::SquashResult;

/// Handle `wt step commit` for jj repositories.
///
/// jj auto-snapshots the working copy, so "commit" means describing the
/// current change and starting a new one:
///
/// 1. Check if there are changes to commit (`jj diff`)
/// 2. Generate a commit message (LLM or fallback)
/// 3. `jj describe -m "{message}"` — set the description
/// 4. `jj new` — start a new change
pub fn step_commit_jj(show_prompt: bool) -> anyhow::Result<()> {
    let workspace = JjWorkspace::from_current_dir()?;
    let cwd = std::env::current_dir()?;

    // Check if there are changes to commit (use jj diff, not --stat, to avoid
    // the "0 files changed" summary line that --stat always emits)
    let diff_full = workspace.run_in_dir(&cwd, &["diff", "-r", "@"])?;
    if diff_full.trim().is_empty() {
        anyhow::bail!("Nothing to commit (working copy is empty)");
    }

    // Get stat summary for commit message generation
    let diff = workspace.run_in_dir(&cwd, &["diff", "-r", "@", "--stat"])?;

    let config = UserConfig::load().context("Failed to load config")?;
    let project_id = workspace.project_identifier().ok();
    let commit_config = config.commit_generation(project_id.as_deref());

    // Handle --show-prompt: build and output the prompt without committing
    if show_prompt {
        if commit_config.is_configured() {
            let ws_name = workspace
                .current_name(&cwd)?
                .unwrap_or_else(|| "default".to_string());
            let repo_name = project_id.as_deref().unwrap_or("repo");
            let prompt = crate::llm::build_jj_commit_prompt(
                &diff_full,
                &diff,
                &ws_name,
                repo_name,
                &commit_config,
            )?;
            println!("{}", prompt);
        } else {
            println!("(no LLM configured — would use fallback message from changed files)");
        }
        return Ok(());
    }

    let commit_message =
        generate_jj_commit_message(&workspace, &cwd, &diff_full, &diff, &commit_config)?;

    // Describe the current change and start a new one
    workspace.commit(&commit_message, &cwd)?;

    // Show commit message first line (more useful than change ID)
    let first_line = commit_message.lines().next().unwrap_or(&commit_message);
    eprintln!(
        "{}",
        success_message(cformat!("Committed: <dim>{}</>", first_line))
    );

    Ok(())
}

/// Handle `wt step squash` for jj repositories.
///
/// Squashes all feature commits into a single commit on trunk.
pub fn handle_squash_jj(target: Option<&str>) -> anyhow::Result<SquashResult> {
    let workspace = JjWorkspace::from_current_dir()?;
    let cwd = std::env::current_dir()?;

    // Detect trunk bookmark
    let detected_target = workspace.trunk_bookmark()?;
    let target = target.unwrap_or(detected_target.as_str());

    let feature_tip = workspace.feature_tip(&cwd)?;

    // Check if already integrated (use target bookmark, not trunk() revset,
    // because trunk() only resolves with remote tracking branches)
    if workspace.is_integrated(&feature_tip, target)?.is_some() {
        return Ok(SquashResult::NoCommitsAhead(target.to_string()));
    }

    // Count commits ahead of target
    // (is_integrated already handles the 0-commit case — if feature_tip is not
    // in target's ancestry, target..feature_tip must contain at least feature_tip)
    let revset = format!("{target}..{feature_tip}");
    let count_output = workspace.run_in_dir(
        &cwd,
        &["log", "-r", &revset, "--no-graph", "-T", r#""x\n""#],
    )?;
    let commit_count = count_output.lines().filter(|l| !l.is_empty()).count();

    // Check if already a single commit and @ is empty (nothing to squash)
    let at_empty = workspace.run_in_dir(
        &cwd,
        &[
            "log",
            "-r",
            "@",
            "--no-graph",
            "-T",
            r#"if(self.empty(), "empty", "content")"#,
        ],
    )?;
    if commit_count == 1 && at_empty.trim() == "empty" {
        return Ok(SquashResult::AlreadySingleCommit);
    }

    // Get workspace name for the squash message
    let ws_name = workspace
        .current_name(&cwd)?
        .unwrap_or_else(|| "default".to_string());

    eprintln!(
        "{}",
        progress_message(cformat!(
            "Squashing {commit_count} commit{} into trunk...",
            if commit_count == 1 { "" } else { "s" }
        ))
    );

    let message = collect_squash_message(&workspace, &cwd, &feature_tip, &ws_name, target)?;
    workspace.squash_commits(target, &message, &cwd)?;

    eprintln!(
        "{}",
        success_message(cformat!("Squashed onto <bold>{target}</>"))
    );

    Ok(SquashResult::Squashed)
}

// ============================================================================
// Helpers
// ============================================================================

/// Collect descriptions from feature commits for a squash message.
///
/// Used by both `step squash` and `merge` to generate the commit message
/// for squash operations.
pub(crate) fn collect_squash_message(
    workspace: &JjWorkspace,
    ws_path: &Path,
    feature_tip: &str,
    ws_name: &str,
    target: &str,
) -> anyhow::Result<String> {
    let from_revset = format!("{target}..{feature_tip}");
    let descriptions = workspace.run_in_dir(
        ws_path,
        &[
            "log",
            "-r",
            &from_revset,
            "--no-graph",
            "-T",
            r#"self.description() ++ "\n""#,
        ],
    )?;

    let message = descriptions.trim();
    if message.is_empty() {
        Ok(format!("Merge workspace {ws_name}"))
    } else {
        Ok(message.to_string())
    }
}

/// Generate a commit message for jj changes.
///
/// Uses LLM if configured, otherwise falls back to a message based on changed files.
fn generate_jj_commit_message(
    workspace: &JjWorkspace,
    cwd: &Path,
    diff_full: &str,
    diff_stat: &str,
    config: &worktrunk::config::CommitGenerationConfig,
) -> anyhow::Result<String> {
    if config.is_configured() {
        let ws_name = workspace
            .current_name(cwd)?
            .unwrap_or_else(|| "default".to_string());
        let repo_name = workspace.project_identifier().ok();
        let repo_name = repo_name.as_deref().unwrap_or("repo");
        let prompt =
            crate::llm::build_jj_commit_prompt(diff_full, diff_stat, &ws_name, repo_name, config)?;
        let command = config.command.as_ref().unwrap();
        return crate::llm::execute_llm_command(command, &prompt);
    }

    // Fallback: use the existing jj description or generate from changed files
    let description = workspace.run_in_dir(
        cwd,
        &["log", "-r", "@", "--no-graph", "-T", "self.description()"],
    )?;
    let description = description.trim();

    if !description.is_empty() {
        return Ok(description.to_string());
    }

    // Generate from changed files in the diff stat
    let files: Vec<&str> = diff_stat
        .lines()
        .filter(|l| l.contains('|'))
        .map(|l| l.split('|').next().unwrap_or("").trim())
        .filter(|s| !s.is_empty())
        .map(|path| path.rsplit('/').next().unwrap_or(path))
        .collect();

    let message = match files.len() {
        0 => "WIP: Changes".to_string(),
        1 => format!("Changes to {}", files[0]),
        2 => format!("Changes to {} & {}", files[0], files[1]),
        3 => format!("Changes to {}, {} & {}", files[0], files[1], files[2]),
        n => format!("Changes to {} files", n),
    };

    Ok(message)
}
