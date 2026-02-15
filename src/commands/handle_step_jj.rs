//! Step command handlers for jj repositories.
//!
//! jj equivalent of `step commit`. Squash and push are handled by unified
//! implementations via the [`Workspace`] trait.

use anyhow::Context;
use color_print::cformat;
use worktrunk::config::UserConfig;
use worktrunk::styling::{eprintln, success_message};
use worktrunk::workspace::{JjWorkspace, Workspace};

/// Handle `wt step commit` for jj repositories.
///
/// jj auto-snapshots the working copy, so "commit" means describing the
/// current change and starting a new one:
///
/// 1. Check if there are changes to commit (`jj diff`)
/// 2. Generate a commit message (LLM or fallback)
/// 3. `jj describe -m "{message}"` — set the description
/// 4. `jj new` — start a new change
pub fn step_commit_jj() -> anyhow::Result<()> {
    let workspace = JjWorkspace::from_current_dir()?;
    let cwd = std::env::current_dir()?;

    // Check if there are changes to commit
    let (diff, diff_stat) = workspace.committable_diff_for_prompt(&cwd)?;
    if diff.trim().is_empty() {
        anyhow::bail!("Nothing to commit (working copy is empty)");
    }

    let config = UserConfig::load().context("Failed to load config")?;
    let project_id = workspace.project_identifier().ok();
    let commit_config = config.commit_generation(project_id.as_deref());

    let commit_message =
        generate_jj_commit_message(&workspace, &cwd, &diff, &diff_stat, &commit_config)?;

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

// ============================================================================
// Helpers
// ============================================================================

/// Generate a commit message for jj changes.
///
/// Uses LLM if configured, otherwise falls back to the existing jj description
/// or a message based on changed files (shared fallback with git via `generate_commit_message`).
fn generate_jj_commit_message(
    workspace: &JjWorkspace,
    cwd: &std::path::Path,
    diff: &str,
    diff_stat: &str,
    config: &worktrunk::config::CommitGenerationConfig,
) -> anyhow::Result<String> {
    // jj-specific: check existing description first (jj may already have one)
    if !config.is_configured() {
        let description = workspace.run_in_dir(
            cwd,
            &["log", "-r", "@", "--no-graph", "-T", "self.description()"],
        )?;
        let description = description.trim();

        if !description.is_empty() {
            return Ok(description.to_string());
        }
    }

    // Shared path: build CommitInput and use unified generate_commit_message
    let ws_name = workspace
        .current_name(cwd)?
        .unwrap_or_else(|| "default".to_string());
    let repo_name = workspace.project_identifier().ok();
    let repo_name_str = repo_name.as_deref().unwrap_or("repo");

    let input = crate::llm::CommitInput {
        diff,
        diff_stat,
        branch: &ws_name,
        repo_name: repo_name_str,
        recent_commits: None,
    };
    crate::llm::generate_commit_message(&input, config)
}
