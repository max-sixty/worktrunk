//! Step commands for jj workflows.
//!
//! This module contains the individual steps for jj operations:
//! - `handle_rebase_jj` - Rebase onto target bookmark
//! - `handle_squash_jj` - Squash commits into parent
//! - `handle_commit_jj` - Describe/commit changes
//! - `handle_merge_jj` - Full merge workflow

use anyhow::Context;
use color_print::cformat;
use worktrunk::config::UserConfig;
use worktrunk::jj::Repository;
use worktrunk::styling::{
    eprintln, format_with_gutter, hint_message, info_message, progress_message, success_message,
};

// ============================================================================
// Rebase
// ============================================================================

/// Result of a rebase operation
pub enum RebaseResult {
    /// Rebase occurred
    Rebased,
    /// Already up-to-date with target
    UpToDate(String),
}

/// Handle jj rebase workflow
///
/// Rebases the current commit (and descendants) onto the target bookmark.
pub fn handle_rebase_jj(target: Option<&str>) -> anyhow::Result<RebaseResult> {
    let repo = Repository::current()?;

    // Get target bookmark (default to main/master)
    let target_bookmark = match target {
        Some(t) => t.to_string(),
        None => repo
            .default_bookmark()
            .ok_or_else(|| anyhow::anyhow!("No target specified and no default bookmark found"))?,
    };

    // Check if already rebased (current commit is descendant of target)
    let is_rebased = repo
        .run_command_check(&[
            "log",
            "-r",
            &format!("ancestors(@) & {}", target_bookmark),
            "--no-graph",
            "-T",
            "commit_id",
        ])?;

    if is_rebased {
        // Check if target is in our history - if so, we're already rebased
        let output = repo.run_command(&[
            "log",
            "-r",
            &format!("ancestors(@) & {}", target_bookmark),
            "--no-graph",
            "-T",
            "commit_id",
        ])?;
        if !output.trim().is_empty() {
            return Ok(RebaseResult::UpToDate(target_bookmark));
        }
    }

    eprintln!(
        "{}",
        progress_message(cformat!("Rebasing onto <bold>{target_bookmark}</>..."))
    );

    // Rebase current commit onto target
    let result = repo.run_command(&["rebase", "-d", &target_bookmark]);

    match result {
        Ok(_) => {
            eprintln!(
                "{}",
                success_message(cformat!("Rebased onto <bold>{target_bookmark}</>"))
            );
            Ok(RebaseResult::Rebased)
        }
        Err(e) => {
            // Check for conflicts
            let ws = repo.current_workspace();
            if ws.has_conflicts()? {
                Err(anyhow::anyhow!(
                    "Rebase conflict. Resolve conflicts and run `jj squash` to continue."
                ))
            } else {
                Err(e)
            }
        }
    }
}

// ============================================================================
// Squash
// ============================================================================

/// Result of a squash operation
#[derive(Debug, Clone)]
pub enum SquashResult {
    /// Squash occurred
    Squashed,
    /// Nothing to squash: no commits ahead of target
    NoCommitsAhead(String),
    /// Nothing to squash: already a single commit
    AlreadySingleCommit,
    /// Squash attempted but resulted in no net changes
    NoNetChanges,
}

/// Handle jj squash workflow
///
/// In jj, squashing works differently than git:
/// - `jj squash` squashes current commit into its parent
/// - To squash multiple commits, we need to repeatedly squash or use `jj squash --from`
///
/// This function squashes all commits between current and the target bookmark's base.
pub fn handle_squash_jj(
    target: Option<&str>,
    _yes: bool,
    _stage: Option<crate::commands::commit::StageMode>,
) -> anyhow::Result<SquashResult> {
    let repo = Repository::current()?;

    // Get target bookmark
    let target_bookmark = match target {
        Some(t) => t.to_string(),
        None => repo
            .default_bookmark()
            .ok_or_else(|| anyhow::anyhow!("No target specified and no default bookmark found"))?,
    };

    // Count commits between current and target
    // In jj, we use revsets: "@ ~ ancestors(target)"
    let commits_output = repo.run_command(&[
        "log",
        "-r",
        &format!("ancestors(@) ~ ancestors({})", target_bookmark),
        "--no-graph",
        "-T",
        "commit_id ++ \"\\n\"",
    ])?;

    let commit_count = commits_output.lines().filter(|l| !l.is_empty()).count();

    if commit_count == 0 {
        return Ok(SquashResult::NoCommitsAhead(target_bookmark));
    }

    if commit_count == 1 {
        // Check if there are uncommitted changes
        let ws = repo.current_workspace();
        if !ws.is_dirty()? {
            return Ok(SquashResult::AlreadySingleCommit);
        }
    }

    let commit_text = if commit_count == 1 {
        "commit"
    } else {
        "commits"
    };

    eprintln!(
        "{}",
        progress_message(format!(
            "Squashing {commit_count} {commit_text} into parent..."
        ))
    );

    // In jj, to squash multiple commits, we need to squash into parent repeatedly
    // or use `jj squash --from` with a revset
    // Let's use the simpler approach: squash into parent
    if commit_count > 1 {
        // Squash all commits from the range into the first one after target
        // jj squash --from 'ancestors(@) ~ ancestors(target)' squashes into parent
        let revset = format!("ancestors(@) ~ ancestors({})", target_bookmark);
        repo.run_command(&["squash", "--from", &revset])
            .context("Failed to squash commits")?;
    } else {
        // Single commit with changes - just describe it or squash into parent
        repo.run_command(&["squash"])
            .context("Failed to squash")?;
    }

    // Get new commit hash
    let commit_hash = repo
        .run_command(&["log", "-r", "@", "--no-graph", "-T", "commit_id.short()"])?
        .trim()
        .to_string();

    eprintln!(
        "{}",
        success_message(cformat!("Squashed @ <dim>{commit_hash}</>"))
    );

    Ok(SquashResult::Squashed)
}

// ============================================================================
// Commit / Describe
// ============================================================================

/// Handle jj commit workflow
///
/// In jj, the working copy is always part of a commit. This function:
/// 1. Generates a commit message using LLM (if configured)
/// 2. Uses `jj commit` to finalize the current commit and start a new one
pub fn handle_commit_jj(message: Option<&str>, _yes: bool) -> anyhow::Result<()> {
    let repo = Repository::current()?;
    let ws = repo.current_workspace();

    // Check if there are changes
    if !ws.is_dirty()? {
        eprintln!("{}", info_message("No changes to commit"));
        return Ok(());
    }

    // Get or generate commit message
    let commit_message = match message {
        Some(msg) => msg.to_string(),
        None => {
            // Generate using LLM or prompt for message
            eprintln!("{}", progress_message("Generating commit message..."));

            // Get diff for context
            let diff = ws.run_command(&["diff"])?;

            // Simple message generation (LLM integration can be added later)
            if diff.is_empty() {
                "Update files".to_string()
            } else {
                // Extract first changed file for basic message
                let first_file = diff
                    .lines()
                    .find(|l| l.starts_with("Modified") || l.starts_with("Added"))
                    .map(|l| l.split_whitespace().last().unwrap_or("files"))
                    .unwrap_or("files");
                format!("Update {}", first_file)
            }
        }
    };

    // Use jj commit which describes the current commit and creates a new working copy
    ws.run_command(&["commit", "-m", &commit_message])
        .context("Failed to commit")?;

    // Get the commit hash of the just-committed change (parent of current)
    let commit_hash = ws
        .run_command(&["log", "-r", "@-", "--no-graph", "-T", "commit_id.short()"])?
        .trim()
        .to_string();

    eprintln!(
        "{}",
        success_message(cformat!("Committed @ <dim>{commit_hash}</>"))
    );
    eprintln!("{}", format_with_gutter(&commit_message, None));

    Ok(())
}

// ============================================================================
// Merge workflow
// ============================================================================

/// Options for the jj merge command
pub struct MergeOptions<'a> {
    pub target: Option<&'a str>,
    pub squash: Option<bool>,
    pub rebase: Option<bool>,
    pub remove: Option<bool>,
    pub yes: bool,
}

/// Handle the full jj merge workflow
///
/// This orchestrates:
/// 1. Commit any pending changes
/// 2. Squash commits (if enabled)
/// 3. Rebase onto target (if enabled)
/// 4. Push/update the target bookmark
/// 5. Remove workspace (if enabled)
pub fn handle_merge_jj(opts: MergeOptions<'_>) -> anyhow::Result<()> {
    let MergeOptions {
        target,
        squash,
        rebase,
        remove,
        yes,
    } = opts;

    let repo = Repository::current()?;
    let config = UserConfig::load().context("Failed to load config")?;

    // Get current bookmark
    let current_bookmark = repo
        .current_workspace()
        .bookmark()?
        .ok_or_else(|| anyhow::anyhow!("Not on a bookmark. Cannot merge."))?;

    // Get target bookmark
    let target_bookmark = match target {
        Some(t) => t.to_string(),
        None => repo
            .default_bookmark()
            .ok_or_else(|| anyhow::anyhow!("No target specified and no default bookmark found"))?,
    };

    // Default options
    let squash_enabled = squash.unwrap_or(true);
    let rebase_enabled = rebase.unwrap_or(true);
    let remove_enabled = remove.unwrap_or(true);

    // Don't remove if we're on the target bookmark
    let on_target = current_bookmark == target_bookmark;
    let remove_effective = remove_enabled && !on_target;

    // Step 1: Squash if enabled
    let squashed = if squash_enabled {
        match handle_squash_jj(Some(&target_bookmark), yes, None)? {
            SquashResult::Squashed => true,
            _ => false,
        }
    } else {
        false
    };

    // Step 2: Rebase if enabled
    let rebased = if rebase_enabled {
        match handle_rebase_jj(Some(&target_bookmark))? {
            RebaseResult::Rebased => true,
            RebaseResult::UpToDate(_) => false,
        }
    } else {
        false
    };

    // Step 3: Update target bookmark to include our changes
    // In jj, we can move the bookmark to point to our commit
    eprintln!(
        "{}",
        progress_message(cformat!(
            "Updating <bold>{target_bookmark}</> bookmark..."
        ))
    );

    // Set the target bookmark to point to our current commit
    repo.set_bookmark(&target_bookmark, Some("@"))?;

    // Build status message
    let mut actions = Vec::new();
    if squashed {
        actions.push("squashed");
    }
    if rebased {
        actions.push("rebased");
    }

    let action_str = if actions.is_empty() {
        "Merged".to_string()
    } else {
        format!("Merged ({})", actions.join(", "))
    };

    eprintln!(
        "{}",
        success_message(cformat!(
            "{action_str} to <bold>{target_bookmark}</>"
        ))
    );

    // Step 4: Remove workspace if enabled
    if remove_effective {
        // Get current workspace name before removal
        let workspace_info = repo.current_workspace_info()?;
        if let Some(ws) = workspace_info {
            let destination = repo.home_path()?;

            eprintln!(
                "{}",
                progress_message(cformat!(
                    "Removing workspace <bold>{}</>...",
                    ws.name
                ))
            );

            // Forget the workspace and optionally delete the directory
            repo.remove_workspace(&ws.name, true)?;

            eprintln!(
                "{}",
                success_message(cformat!("Removed workspace <bold>{}</>", ws.name))
            );

            // Hint about changing directory
            eprintln!(
                "{}",
                hint_message(cformat!(
                    "Run <bright-black>cd {}</> to return to main workspace",
                    destination.display()
                ))
            );
        }
    } else {
        let message = if on_target {
            "Workspace preserved (already on target bookmark)"
        } else {
            "Workspace preserved (--no-remove)"
        };
        eprintln!("{}", info_message(message));
    }

    Ok(())
}

// ============================================================================
// Push
// ============================================================================

/// Handle pushing changes to a remote
///
/// In jj, pushing is done via `jj git push` for git-backed repos
pub fn handle_push_jj(bookmark: Option<&str>) -> anyhow::Result<()> {
    let repo = Repository::current()?;

    let bookmark_to_push = match bookmark {
        Some(b) => b.to_string(),
        None => repo
            .current_workspace()
            .bookmark()?
            .ok_or_else(|| anyhow::anyhow!("Not on a bookmark. Specify a bookmark to push."))?,
    };

    eprintln!(
        "{}",
        progress_message(cformat!(
            "Pushing <bold>{bookmark_to_push}</> to remote..."
        ))
    );

    // Push using jj git push
    repo.run_command(&["git", "push", "--bookmark", &bookmark_to_push])
        .context("Failed to push")?;

    eprintln!(
        "{}",
        success_message(cformat!("Pushed <bold>{bookmark_to_push}</> to remote"))
    );

    Ok(())
}
