//! Step command handlers for jj repositories.
//!
//! jj equivalents of `step commit`, `step squash`, `step rebase`, and `step push`.
//! Reuses helpers from [`super::handle_merge_jj`] where possible.

use std::path::Path;

use anyhow::Context;
use color_print::cformat;
use worktrunk::config::UserConfig;
use worktrunk::styling::{eprintln, progress_message, success_message};
use worktrunk::workspace::{JjWorkspace, Workspace};

use super::handle_merge_jj::{get_feature_tip, push_bookmark, squash_into_trunk};
use super::step_commands::{RebaseResult, SquashResult};

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
    let project_id = workspace_project_id(&workspace);
    let commit_config = config.commit_generation(project_id.as_deref());

    // Handle --show-prompt: build and output the prompt without committing
    if show_prompt {
        if commit_config.is_configured() {
            let ws_name = workspace_name(&workspace, &cwd);
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
    workspace.run_in_dir(&cwd, &["describe", "-m", &commit_message])?;
    workspace.run_in_dir(&cwd, &["new"])?;

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

    // Get the feature tip
    let feature_tip = get_feature_tip(&workspace, &cwd)?;

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
    let ws_name = workspace_name(&workspace, &cwd);

    eprintln!(
        "{}",
        progress_message(cformat!(
            "Squashing {commit_count} commit{} into trunk...",
            if commit_count == 1 { "" } else { "s" }
        ))
    );

    squash_into_trunk(&workspace, &cwd, &feature_tip, &ws_name, target)?;

    eprintln!(
        "{}",
        success_message(cformat!("Squashed onto <bold>{target}</>"))
    );

    Ok(SquashResult::Squashed)
}

/// Handle `wt step rebase` for jj repositories.
///
/// Rebases the current feature onto trunk.
pub fn handle_rebase_jj(target: Option<&str>) -> anyhow::Result<RebaseResult> {
    let workspace = JjWorkspace::from_current_dir()?;
    let cwd = std::env::current_dir()?;

    // Detect trunk bookmark
    let detected_target = workspace.trunk_bookmark()?;
    let target = target.unwrap_or(detected_target.as_str());

    let feature_tip = get_feature_tip(&workspace, &cwd)?;

    // Check if already rebased: is target an ancestor of feature tip?
    if is_ancestor_of(&workspace, &cwd, target, &feature_tip)? {
        return Ok(RebaseResult::UpToDate(target.to_string()));
    }

    eprintln!(
        "{}",
        progress_message(cformat!("Rebasing onto <bold>{target}</>..."))
    );

    // Rebase using the bookmark name directly (not trunk() revset)
    workspace.run_in_dir(&cwd, &["rebase", "-b", "@", "-d", target])?;

    eprintln!(
        "{}",
        success_message(cformat!("Rebased onto <bold>{target}</>"))
    );

    Ok(RebaseResult::Rebased)
}

/// Handle `wt step push` for jj repositories.
///
/// Moves the target bookmark to the feature tip and pushes to remote.
pub fn handle_push_jj(target: Option<&str>) -> anyhow::Result<()> {
    let workspace = JjWorkspace::from_current_dir()?;
    let cwd = std::env::current_dir()?;

    // Detect trunk bookmark
    let detected_target = workspace.trunk_bookmark()?;
    let target = target.unwrap_or(detected_target.as_str());

    // Get the feature tip
    let feature_tip = get_feature_tip(&workspace, &cwd)?;

    // Guard: target must be an ancestor of (or equal to) the feature tip.
    // This prevents moving the bookmark sideways or backward (which would lose commits).
    // Note: we intentionally don't short-circuit when feature_tip == target — after
    // `step squash`, the local bookmark is already moved but the remote needs pushing.
    if !is_ancestor_of(&workspace, &cwd, target, &feature_tip)? {
        anyhow::bail!(
            "Cannot push: feature is not ahead of {target}. Rebase first with `wt step rebase`."
        );
    }

    // Move bookmark to feature tip (no-op if already there, e.g., after squash)
    workspace.run_in_dir(&cwd, &["bookmark", "set", target, "-r", &feature_tip])?;

    // Push (best-effort — may not have a git remote)
    push_bookmark(&workspace, &cwd, target);

    Ok(())
}

// ============================================================================
// Helpers
// ============================================================================

/// Check if `target` (a bookmark name) is an ancestor of `descendant` (a change ID).
fn is_ancestor_of(
    workspace: &JjWorkspace,
    cwd: &Path,
    target: &str,
    descendant: &str,
) -> anyhow::Result<bool> {
    // jj resolves bookmark names directly in revsets, so we can check
    // ancestry in a single command: "target & ::descendant" is non-empty
    // iff target is an ancestor of (or equal to) descendant.
    let check = workspace.run_in_dir(
        cwd,
        &[
            "log",
            "-r",
            &format!("{target} & ::{descendant}"),
            "--no-graph",
            "-T",
            r#""x""#,
        ],
    )?;
    Ok(!check.trim().is_empty())
}

/// Get the workspace name for the current directory.
fn workspace_name(workspace: &JjWorkspace, cwd: &Path) -> String {
    workspace
        .current_workspace(cwd)
        .map(|ws| ws.name)
        .unwrap_or_else(|_| "default".to_string())
}

/// Get a project identifier from the jj workspace root directory name.
fn workspace_project_id(workspace: &JjWorkspace) -> Option<String> {
    workspace
        .root()
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
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
        let ws_name = workspace_name(workspace, cwd);
        let repo_name = workspace_project_id(workspace);
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
