use worktrunk::config::WorktrunkConfig;
use worktrunk::git::{GitError, Repository};
use worktrunk::styling::{AnstyleStyle, ERROR, ERROR_EMOJI, println};

use super::worktree::handle_push;
use super::worktree::handle_remove;

pub fn handle_merge(
    target: Option<&str>,
    squash: bool,
    keep: bool,
    internal: bool,
) -> Result<(), GitError> {
    let repo = Repository::current();

    // Get current branch
    let current_branch = repo.current_branch()?.ok_or_else(|| {
        GitError::CommandFailed(format!(
            "{ERROR_EMOJI} {ERROR}Not on a branch (detached HEAD){ERROR:#}"
        ))
    })?;

    // Get target branch (default to default branch if not provided)
    let target_branch = target.map_or_else(|| repo.default_branch(), |b| Ok(b.to_string()))?;

    // Check if already on target branch
    if current_branch == target_branch {
        let bold = AnstyleStyle::new().bold();
        println!("Already on {bold}{target_branch}{bold:#}, nothing to merge");
        return Ok(());
    }

    // Check for uncommitted changes
    repo.ensure_clean_working_tree()?;

    // Squash commits if requested
    if squash {
        handle_squash(&target_branch)?;
    }

    // Rebase onto target
    let bold = AnstyleStyle::new().bold();
    println!("Rebasing onto {bold}{target_branch}{bold:#}...");

    repo.run_command(&["rebase", &target_branch]).map_err(|e| {
        GitError::CommandFailed(format!("Failed to rebase onto '{}': {}", target_branch, e))
    })?;

    // Fast-forward push to target branch (reuse handle_push logic)
    println!("Fast-forwarding {bold}{target_branch}{bold:#} to current HEAD...");
    handle_push(Some(&target_branch), false)?;

    // Finish worktree unless --keep was specified
    if !keep {
        println!("Cleaning up worktree...");

        // Get primary worktree path before finishing (while we can still run git commands)
        let primary_worktree_dir = repo.repo_root()?;

        let result = handle_remove()?;

        // Display output based on mode
        if internal {
            if let Some(output) = result.format_internal_output() {
                println!("{}", output);
            }
        } else if let Some(output) = result.format_user_output() {
            println!("{}", output);
        }

        // Check if we need to switch to target branch
        let primary_repo = Repository::at(&primary_worktree_dir);
        let new_branch = primary_repo.current_branch()?;
        if new_branch.as_deref() != Some(&target_branch) {
            let bold = AnstyleStyle::new().bold();
            println!("Switching to {bold}{target_branch}{bold:#}...");
            primary_repo
                .run_command(&["switch", &target_branch])
                .map_err(|e| {
                    GitError::CommandFailed(format!(
                        "Failed to switch to '{}': {}",
                        target_branch, e
                    ))
                })?;
        }
    } else {
        let bold = AnstyleStyle::new().bold();
        println!("Successfully merged to {bold}{target_branch}{bold:#} (worktree preserved)");
    }

    Ok(())
}

fn handle_squash(target_branch: &str) -> Result<(), GitError> {
    let repo = Repository::current();

    // Get merge base with target branch
    let merge_base = repo.merge_base("HEAD", target_branch)?;

    // Count commits since merge base
    let commit_count = repo.count_commits(&merge_base, "HEAD")?;

    // Check if there are staged changes
    let has_staged = repo.has_staged_changes()?;

    // Handle different scenarios
    if commit_count == 0 && !has_staged {
        // No commits and no staged changes - nothing to squash
        println!("No commits to squash - already at merge base");
        return Ok(());
    }

    if commit_count == 0 && has_staged {
        // Just staged changes, no commits - would need to commit but this shouldn't happen in merge flow
        return Err(GitError::CommandFailed(format!(
            "{ERROR_EMOJI} {ERROR}Staged changes without commits - please commit them first{ERROR:#}"
        )));
    }

    if commit_count == 1 && !has_staged {
        // Single commit, no staged changes - nothing to do
        let bold = AnstyleStyle::new().bold();
        println!("Only 1 commit since {bold}{target_branch}{bold:#} - no squashing needed");
        return Ok(());
    }

    // One or more commits (possibly with staged changes) - squash them
    println!("Squashing {} commits into one...", commit_count);

    // Get commit subjects for the squash message
    let range = format!("{}..HEAD", merge_base);
    let subjects = repo.commit_subjects(&range)?;

    // Load config and generate commit message
    let config = WorktrunkConfig::load()
        .map_err(|e| GitError::CommandFailed(format!("Failed to load config: {}", e)))?;
    let commit_message = crate::llm::generate_squash_message(target_branch, &subjects, &config.llm);

    // Reset to merge base (soft reset stages all changes)
    repo.run_command(&["reset", "--soft", &merge_base])
        .map_err(|e| GitError::CommandFailed(format!("Failed to reset to merge base: {}", e)))?;

    // Commit with the generated message
    repo.run_command(&["commit", "-m", &commit_message])
        .map_err(|e| GitError::CommandFailed(format!("Failed to create squash commit: {}", e)))?;

    println!("Successfully squashed {} commits into one", commit_count);
    Ok(())
}
