//! Step commands for the merge workflow.
//!
//! This module contains the individual steps that make up `wt merge`:
//! - `step_commit` - Commit working tree changes
//! - `handle_squash` - Squash commits into one
//! - `step_show_squash_prompt` - Show squash prompt without executing
//! - `handle_rebase` - Rebase onto target branch

use anyhow::Context;
use color_print::cformat;
use worktrunk::HookType;
use worktrunk::config::WorktrunkConfig;
use worktrunk::git::Repository;
use worktrunk::styling::{
    format_with_gutter, hint_message, info_message, progress_message, success_message,
};

use super::commit::{CommitGenerator, CommitOptions};
use super::context::CommandEnv;
use super::hooks::{HookFailureStrategy, run_hook_with_filter};
use super::repository_ext::RepositoryCliExt;

/// Handle `wt step commit` command
pub fn step_commit(
    yes: bool,
    no_verify: bool,
    stage_mode: super::commit::StageMode,
    show_prompt: bool,
) -> anyhow::Result<()> {
    use super::command_approval::approve_hooks;

    // Handle --show-prompt early: just build and output the prompt
    if show_prompt {
        let config = WorktrunkConfig::load().context("Failed to load config")?;
        let prompt = crate::llm::build_commit_prompt(&config.commit_generation)?;
        crate::output::stdout(prompt)?;
        return Ok(());
    }

    let env = CommandEnv::for_action("commit")?;
    let ctx = env.context(yes);

    // "Approve at the Gate": approve pre-commit hooks upfront (unless --no-verify)
    // Shadow no_verify: if user declines approval, skip hooks but continue commit
    let no_verify = if !no_verify {
        let approved = approve_hooks(&ctx, &[HookType::PreCommit])?;
        if !approved {
            crate::output::print(worktrunk::styling::info_message(
                "Commands declined, committing without hooks",
            ))?;
            true // Skip hooks
        } else {
            false // Run hooks
        }
    } else {
        true // --no-verify was passed
    };

    let mut options = CommitOptions::new(&ctx);
    options.no_verify = no_verify;
    options.stage_mode = stage_mode;
    options.show_no_squash_note = false;
    // Only warn about untracked if we're staging all
    options.warn_about_untracked = stage_mode == super::commit::StageMode::All;

    options.commit()
}

/// Result of a squash operation
#[derive(Debug, Clone)]
pub enum SquashResult {
    /// Squash or commit occurred
    Squashed,
    /// Nothing to squash: no commits ahead of target branch
    NoCommitsAhead(String),
    /// Nothing to squash: already a single commit
    AlreadySingleCommit,
    /// Squash attempted but resulted in no net changes (commits canceled out)
    NoNetChanges,
}

/// Handle shared squash workflow (used by `wt step squash` and `wt merge`)
///
/// # Arguments
/// * `skip_pre_commit` - If true, skip all pre-commit hooks (both user and project)
/// * `stage_mode` - What to stage before committing (All or Tracked; None not supported for squash)
pub fn handle_squash(
    target: Option<&str>,
    yes: bool,
    skip_pre_commit: bool,
    stage_mode: super::commit::StageMode,
) -> anyhow::Result<SquashResult> {
    use super::commit::StageMode;

    let env = CommandEnv::for_action("squash")?;
    let repo = &env.repo;
    // Squash requires being on a branch (can't squash in detached HEAD)
    let current_branch = env.require_branch("squash")?.to_string();
    let ctx = env.context(yes);
    let generator = CommitGenerator::new(&env.config.commit_generation);

    // Get target branch (default to default branch if not provided)
    let target_branch = repo.resolve_target_branch(target)?;

    // Auto-stage changes before running pre-commit hooks so both beta and merge paths behave identically
    match stage_mode {
        StageMode::All => {
            repo.warn_if_auto_staging_untracked()?;
            repo.run_command(&["add", "-A"])
                .context("Failed to stage changes")?;
        }
        StageMode::Tracked => {
            repo.run_command(&["add", "-u"])
                .context("Failed to stage tracked changes")?;
        }
        StageMode::None => {
            // Stage nothing - use what's already staged
        }
    }

    // Run pre-commit hooks unless explicitly skipped
    let project_config = repo.load_project_config()?;
    let has_project_pre_commit = project_config
        .as_ref()
        .map(|c| c.hooks.pre_commit.is_some())
        .unwrap_or(false);
    let has_user_pre_commit = ctx.config.hooks.pre_commit.is_some();
    let has_any_pre_commit = has_project_pre_commit || has_user_pre_commit;

    if skip_pre_commit && has_any_pre_commit {
        crate::output::print(info_message(cformat!(
            "Skipping pre-commit hooks (<bright-black>--no-verify</>)"
        )))?;
    }

    // Run pre-commit hooks (user first, then project)
    if !skip_pre_commit {
        let extra_vars = [("target", target_branch.as_str())];
        run_hook_with_filter(
            &ctx,
            ctx.config.hooks.pre_commit.as_ref(),
            project_config
                .as_ref()
                .and_then(|c| c.hooks.pre_commit.as_ref()),
            HookType::PreCommit,
            &extra_vars,
            HookFailureStrategy::FailFast,
            None,
            crate::output::pre_hook_display_path(ctx.worktree_path),
        )
        .map_err(worktrunk::git::add_hook_skip_hint)?;
    }

    // Get merge base with target branch
    let merge_base = repo.merge_base("HEAD", &target_branch)?;

    // Count commits since merge base
    let commit_count = repo.count_commits(&merge_base, "HEAD")?;

    // Check if there are staged changes in addition to commits
    let has_staged = repo.has_staged_changes()?;

    // Handle different scenarios
    if commit_count == 0 && !has_staged {
        // No commits and no staged changes - nothing to squash
        return Ok(SquashResult::NoCommitsAhead(target_branch));
    }

    if commit_count == 0 && has_staged {
        // Just staged changes, no commits - commit them directly (no squashing needed)
        generator.commit_staged_changes(true, stage_mode)?;
        return Ok(SquashResult::Squashed);
    }

    if commit_count == 1 && !has_staged {
        // Single commit, no staged changes - already squashed
        return Ok(SquashResult::AlreadySingleCommit);
    }

    // Either multiple commits OR single commit with staged changes - squash them
    // Get diff stats early for display in progress message
    let range = format!("{}..HEAD", merge_base);

    let commit_text = if commit_count == 1 {
        "commit"
    } else {
        "commits"
    };

    // Get total stats (commits + any working tree changes)
    let total_stats = if has_staged {
        repo.diff_stats_summary(&["diff", "--shortstat", &merge_base, "--cached"])
    } else {
        repo.diff_stats_summary(&["diff", "--shortstat", &range])
    };

    let with_changes = if has_staged {
        match stage_mode {
            super::commit::StageMode::Tracked => " & tracked changes",
            _ => " & working tree changes",
        }
    } else {
        ""
    };

    // Build parenthesized content: stats only (stage mode is in message text)
    let parts = total_stats;

    let squash_progress = if parts.is_empty() {
        format!("Squashing {commit_count} {commit_text}{with_changes} into a single commit...")
    } else {
        // Gray parenthetical with separate cformat for closing paren (avoids optimizer)
        let parts_str = parts.join(", ");
        let paren_close = cformat!("<bright-black>)</>");
        cformat!(
            "Squashing {commit_count} {commit_text}{with_changes} into a single commit <bright-black>({parts_str}</>{paren_close}..."
        )
    };
    crate::output::print(progress_message(squash_progress))?;

    // Create safety backup before potentially destructive reset if there are working tree changes
    if has_staged {
        let backup_message = format!("{} → {} (squash)", current_branch, target_branch);
        let sha = repo.create_safety_backup(&backup_message)?;
        crate::output::print(hint_message(format!("Backup created @ {sha}")))?;
    }

    // Get commit subjects for the squash message
    let subjects = repo.commit_subjects(&range)?;

    // Generate squash commit message
    crate::output::print(progress_message("Generating squash commit message..."))?;

    generator.emit_hint_if_needed()?;

    // Get current branch and repo name for template variables
    let repo_root = repo.worktree_root()?;
    let repo_name = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo");

    let commit_message = crate::llm::generate_squash_message(
        &target_branch,
        &merge_base,
        &subjects,
        &current_branch,
        repo_name,
        &env.config.commit_generation,
    )?;

    // Display the generated commit message
    let formatted_message = generator.format_message_for_display(&commit_message);
    crate::output::print(format_with_gutter(&formatted_message, None))?;

    // Reset to merge base (soft reset stages all changes, including any already-staged uncommitted changes)
    repo.run_command(&["reset", "--soft", &merge_base])
        .context("Failed to reset to merge base")?;

    // Check if there are actually any changes to commit
    if !repo.has_staged_changes()? {
        crate::output::print(info_message(format!(
            "No changes after squashing {commit_count} {commit_text}"
        )))?;
        return Ok(SquashResult::NoNetChanges);
    }

    // Commit with the generated message
    repo.run_command(&["commit", "-m", &commit_message])
        .context("Failed to create squash commit")?;

    // Get commit hash for display
    let commit_hash = repo
        .run_command(&["rev-parse", "--short", "HEAD"])?
        .trim()
        .to_string();

    // Show success immediately after completing the squash
    crate::output::print(success_message(cformat!(
        "Squashed @ <dim>{commit_hash}</>"
    )))?;

    Ok(SquashResult::Squashed)
}

/// Handle `wt step squash --show-prompt`
///
/// Builds and outputs the squash prompt without running the LLM or squashing.
pub fn step_show_squash_prompt(
    target: Option<&str>,
    config: &worktrunk::config::CommitGenerationConfig,
) -> anyhow::Result<()> {
    let repo = Repository::current();

    // Get target branch (default to default branch if not provided)
    let target_branch = repo.resolve_target_branch(target)?;

    // Get current branch
    let current_branch = repo.current_branch()?.unwrap_or("HEAD");

    // Get merge base with target branch
    let merge_base = repo.merge_base("HEAD", &target_branch)?;

    // Get commit subjects for the squash message
    let range = format!("{}..HEAD", merge_base);
    let subjects = repo.commit_subjects(&range)?;

    // Get repo name from directory
    let repo_root = repo.worktree_root()?;
    let repo_name = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo");

    let prompt = crate::llm::build_squash_prompt(
        &target_branch,
        &merge_base,
        &subjects,
        current_branch,
        repo_name,
        config,
    )?;
    crate::output::stdout(prompt)?;
    Ok(())
}

/// Result of a rebase operation
pub enum RebaseResult {
    /// Rebase occurred (either true rebase or fast-forward)
    Rebased,
    /// Already up-to-date with target branch
    UpToDate(String),
}

/// Handle shared rebase workflow (used by `wt step rebase` and `wt merge`)
pub fn handle_rebase(target: Option<&str>) -> anyhow::Result<RebaseResult> {
    use super::repository_ext::RepositoryCliExt;

    let repo = Repository::current();

    // Get target branch (default to default branch if not provided)
    let target_branch = repo.resolve_target_branch(target)?;

    // Check if already up-to-date (linear extension of target, no merge commits)
    if repo.is_rebased_onto(&target_branch)? {
        return Ok(RebaseResult::UpToDate(target_branch));
    }

    // Check if this is a fast-forward or true rebase
    let merge_base = repo.merge_base("HEAD", &target_branch)?;
    let head_sha = repo.run_command(&["rev-parse", "HEAD"])?.trim().to_string();
    let is_fast_forward = merge_base == head_sha;

    // Only show progress for true rebases (fast-forwards are instant)
    if !is_fast_forward {
        crate::output::print(progress_message(cformat!(
            "Rebasing onto <bold>{target_branch}</>..."
        )))?;
    }

    let rebase_result = repo.run_command(&["rebase", &target_branch]);

    // If rebase failed, check if it's due to conflicts
    if let Err(e) = rebase_result {
        if let Some(state) = repo.worktree_state()?
            && state.starts_with("REBASING")
        {
            // Extract git's stderr output from the error
            let git_output = e.to_string();
            return Err(worktrunk::git::GitError::RebaseConflict {
                target_branch: target_branch.clone(),
                git_output,
            }
            .into());
        }
        // Not a rebase conflict, return original error
        return Err(worktrunk::git::GitError::Other {
            message: format!("Failed to rebase onto '{}': {}", target_branch, e),
        }
        .into());
    }

    // Verify rebase completed successfully (safety check for edge cases)
    if let Some(state) = repo.worktree_state()? {
        let _ = state; // used for diagnostics
        return Err(worktrunk::git::GitError::RebaseConflict {
            target_branch: target_branch.clone(),
            git_output: String::new(),
        }
        .into());
    }

    // Success
    if is_fast_forward {
        crate::output::print(success_message(cformat!(
            "Fast-forwarded to <bold>{target_branch}</>"
        )))?;
    } else {
        crate::output::print(success_message(cformat!(
            "Rebased onto <bold>{target_branch}</>"
        )))?;
    }

    Ok(RebaseResult::Rebased)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_squash_result_variants() {
        // Test Debug implementation
        let result = SquashResult::Squashed;
        let debug = format!("{:?}", result);
        assert!(debug.contains("Squashed"));

        let result = SquashResult::NoCommitsAhead("main".to_string());
        let debug = format!("{:?}", result);
        assert!(debug.contains("NoCommitsAhead"));
        assert!(debug.contains("main"));

        let result = SquashResult::AlreadySingleCommit;
        let debug = format!("{:?}", result);
        assert!(debug.contains("AlreadySingleCommit"));

        let result = SquashResult::NoNetChanges;
        let debug = format!("{:?}", result);
        assert!(debug.contains("NoNetChanges"));
    }

    #[test]
    fn test_squash_result_clone() {
        let original = SquashResult::NoCommitsAhead("develop".to_string());
        let cloned = original.clone();
        assert!(matches!(cloned, SquashResult::NoCommitsAhead(ref s) if s == "develop"));
    }

    #[test]
    fn test_rebase_result_variants() {
        // RebaseResult doesn't derive Debug/Clone by default, just test matching
        let result = RebaseResult::Rebased;
        assert!(matches!(result, RebaseResult::Rebased));

        let result = RebaseResult::UpToDate("main".to_string());
        assert!(matches!(result, RebaseResult::UpToDate(ref s) if s == "main"));
    }

    #[test]
    fn test_rebase_result_up_to_date_branch_extraction() {
        let result = RebaseResult::UpToDate("feature-branch".to_string());
        if let RebaseResult::UpToDate(branch) = result {
            assert_eq!(branch, "feature-branch");
        } else {
            panic!("Expected UpToDate variant");
        }
    }
}
