use worktrunk::config::{ProjectConfig, WorktrunkConfig};
use worktrunk::git::{GitError, Repository};
use worktrunk::styling::{
    AnstyleStyle, CYAN, CYAN_BOLD, eprint, eprintln, format_with_gutter, println,
};

use super::command_executor::{CommandContext, prepare_project_commands};
use super::worktree::handle_push;
use super::worktree::handle_remove;
use crate::output::execute_command_in_worktree;

pub fn handle_merge(
    target: Option<&str>,
    squash: bool,
    keep: bool,
    message: Option<&str>,
    no_hooks: bool,
    force: bool,
) -> Result<(), GitError> {
    let repo = Repository::current();

    // Show progress for initial validation
    crate::output::progress(format!("🔄 {CYAN}Validating merge...{CYAN:#}"))
        .map_err(|e| GitError::CommandFailed(e.to_string()))?;

    // Get current branch
    let current_branch = repo.current_branch()?.ok_or(GitError::DetachedHead)?;

    // Get target branch (default to default branch if not provided)
    let target_branch = target.map_or_else(|| repo.default_branch(), |b| Ok(b.to_string()))?;

    // Check if already on target branch
    if current_branch == target_branch {
        let bold = AnstyleStyle::new().bold();
        crate::output::success(format!(
            "Already on {bold}{target_branch}{bold:#}, nothing to merge"
        ))
        .map_err(|e| GitError::CommandFailed(e.to_string()))?;
        return Ok(());
    }

    // Load config for LLM integration
    let config = WorktrunkConfig::load()
        .map_err(|e| GitError::CommandFailed(format!("Failed to load config: {}", e)))?;

    // Run pre-merge checks unless --no-hooks was specified
    // Do this BEFORE committing so we fail fast if checks won't pass
    if !no_hooks && let Ok(Some(project_config)) = ProjectConfig::load(&repo.worktree_root()?) {
        let worktree_path = std::env::current_dir().map_err(|e| {
            GitError::CommandFailed(format!("Failed to get current directory: {}", e))
        })?;
        run_pre_merge_checks(
            &project_config,
            &current_branch,
            &target_branch,
            &worktree_path,
            &repo,
            &config,
            force,
        )?;
    }

    // Auto-commit uncommitted changes if they exist
    // Only do this after pre-merge checks pass
    if repo.is_dirty()? {
        handle_commit_changes(message, &config.commit_generation)?;
    }

    // Squash commits if requested
    if squash {
        handle_squash(&target_branch)?;
    }

    // Rebase onto target
    crate::output::progress(format!(
        "🔄 {CYAN}Rebasing onto {CYAN_BOLD}{target_branch}{CYAN_BOLD:#}...{CYAN:#}"
    ))
    .map_err(|e| GitError::CommandFailed(e.to_string()))?;

    let rebase_result = repo.run_command(&["rebase", &target_branch]);

    // If rebase failed, check if it's due to conflicts
    if let Err(e) = rebase_result {
        if let Some(state) = repo.worktree_state()?
            && state.starts_with("REBASING")
        {
            return Err(GitError::RebaseConflict {
                state,
                target_branch: target_branch.to_string(),
            });
        }
        // Not a rebase conflict, return original error
        return Err(GitError::CommandFailed(format!(
            "Failed to rebase onto '{}': {}",
            target_branch, e
        )));
    }

    // Verify rebase completed successfully (safety check for edge cases)
    if let Some(state) = repo.worktree_state()? {
        return Err(GitError::RebaseConflict {
            state,
            target_branch: target_branch.to_string(),
        });
    }

    // Fast-forward push to target branch (reuse handle_push logic)
    handle_push(Some(&target_branch), false)?;

    // Execute post-merge commands in the main worktree
    let main_worktree_path = repo.main_worktree_root()?;
    execute_post_merge_commands(
        &main_worktree_path,
        &repo,
        &config,
        &current_branch,
        &target_branch,
        force,
    )?;

    // Finish worktree unless --keep was specified
    if !keep {
        crate::output::progress(format!("🔄 {CYAN}Cleaning up worktree...{CYAN:#}"))
            .map_err(|e| GitError::CommandFailed(e.to_string()))?;

        // Get primary worktree path before finishing (while we can still run git commands)
        let primary_worktree_dir = repo.main_worktree_root()?;

        let result = handle_remove(None)?;

        // Set directory for shell integration (but don't print separate success message)
        if let super::worktree::RemoveResult::RemovedWorktree { primary_path } = &result {
            crate::output::change_directory(primary_path)
                .map_err(|e| GitError::CommandFailed(e.to_string()))?;
        }

        // Check if we need to switch to target branch
        let primary_repo = Repository::at(&primary_worktree_dir);
        let new_branch = primary_repo.current_branch()?;
        if new_branch.as_deref() != Some(&target_branch) {
            crate::output::progress(format!(
                "🔄 {CYAN}Switching to {CYAN_BOLD}{target_branch}{CYAN_BOLD:#}...{CYAN:#}"
            ))
            .map_err(|e| GitError::CommandFailed(e.to_string()))?;
            primary_repo
                .run_command(&["switch", &target_branch])
                .map_err(|e| {
                    GitError::CommandFailed(format!(
                        "Failed to switch to '{}': {}",
                        target_branch, e
                    ))
                })?;
        }

        // Print comprehensive summary
        println!();
        handle_merge_summary_output(Some(&primary_worktree_dir))?;
    } else {
        // Print comprehensive summary (worktree preserved)
        println!();
        handle_merge_summary_output(None)?;
    }

    Ok(())
}

/// Format the merge summary message
fn format_merge_summary(primary_path: Option<&std::path::Path>) -> String {
    // Show where we ended up
    if let Some(path) = primary_path {
        format!("Returned to primary at {}", path.display())
    } else {
        "Kept worktree (use 'wt remove' to clean up)".to_string()
    }
}

/// Handle output for merge summary using global output context
fn handle_merge_summary_output(primary_path: Option<&std::path::Path>) -> Result<(), GitError> {
    let message = format_merge_summary(primary_path);

    // Show success message (formatting added by OutputContext)
    crate::output::success(message).map_err(|e| GitError::CommandFailed(e.to_string()))?;

    // Flush output
    crate::output::flush().map_err(|e| GitError::CommandFailed(e.to_string()))?;

    Ok(())
}

/// Format a commit message with the first line in bold, ready for gutter display
fn format_commit_message_for_display(message: &str) -> String {
    let bold = AnstyleStyle::new().bold();
    let lines: Vec<&str> = message.lines().collect();

    if lines.is_empty() {
        return String::new();
    }

    // Format first line in bold
    let mut result = format!("{bold}{}{bold:#}", lines[0]);

    // Add remaining lines without bold
    if lines.len() > 1 {
        for line in &lines[1..] {
            result.push('\n');
            result.push_str(line);
        }
    }

    result
}

/// Commit uncommitted changes with LLM-generated message
fn handle_commit_changes(
    custom_instruction: Option<&str>,
    commit_generation_config: &worktrunk::config::CommitGenerationConfig,
) -> Result<(), GitError> {
    let repo = Repository::current();

    crate::output::progress(format!(
        "🔄 {CYAN}Committing uncommitted changes...{CYAN:#}"
    ))
    .map_err(|e| GitError::CommandFailed(e.to_string()))?;

    // Stage all changes including untracked files
    repo.run_command(&["add", "-A"])
        .map_err(|e| GitError::CommandFailed(format!("Failed to stage changes: {}", e)))?;

    // Generate commit message
    crate::output::progress(format!("🔄 {CYAN}Generating commit message...{CYAN:#}"))
        .map_err(|e| GitError::CommandFailed(e.to_string()))?;

    let commit_message =
        crate::llm::generate_commit_message(custom_instruction, commit_generation_config)?;

    // Display the generated commit message
    let formatted_message = format_commit_message_for_display(&commit_message);
    print!("{}", format_with_gutter(&formatted_message, "", None));

    // Commit
    repo.run_command(&["commit", "-m", &commit_message])
        .map_err(|e| GitError::CommandFailed(format!("Failed to commit: {}", e)))?;

    crate::output::success("Committed changes")
        .map_err(|e| GitError::CommandFailed(e.to_string()))?;

    Ok(())
}

fn handle_squash(target_branch: &str) -> Result<Option<usize>, GitError> {
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
        let dim = AnstyleStyle::new().dimmed();
        crate::output::progress(format!(
            "{dim}No commits to squash - already at merge base{dim:#}"
        ))
        .map_err(|e| GitError::CommandFailed(e.to_string()))?;
        return Ok(None);
    }

    if commit_count == 0 && has_staged {
        // Just staged changes, no commits - would need to commit but this shouldn't happen in merge flow
        return Err(GitError::StagedChangesWithoutCommits);
    }

    if commit_count == 1 && !has_staged {
        // Single commit, no staged changes - nothing to do
        let dim = AnstyleStyle::new().dimmed();
        crate::output::progress(format!(
            "{dim}Only 1 commit since {CYAN_BOLD}{target_branch}{CYAN_BOLD:#} - no squashing needed{dim:#}"
        ))
        .map_err(|e| GitError::CommandFailed(e.to_string()))?;
        return Ok(None);
    }

    // One or more commits (possibly with staged changes) - squash them
    crate::output::progress(format!(
        "🔄 {CYAN}Squashing {commit_count} commits into one...{CYAN:#}"
    ))
    .map_err(|e| GitError::CommandFailed(e.to_string()))?;

    // Get commit subjects for the squash message
    let range = format!("{}..HEAD", merge_base);
    let subjects = repo.commit_subjects(&range)?;

    // Load config and generate commit message
    crate::output::progress(format!(
        "🔄 {CYAN}Generating squash commit message...{CYAN:#}"
    ))
    .map_err(|e| GitError::CommandFailed(e.to_string()))?;

    let config = WorktrunkConfig::load()
        .map_err(|e| GitError::CommandFailed(format!("Failed to load config: {}", e)))?;
    let commit_message =
        crate::llm::generate_squash_message(target_branch, &subjects, &config.commit_generation)
            .map_err(|e| {
                GitError::CommandFailed(format!("Failed to generate commit message: {}", e))
            })?;

    // Display the generated commit message
    let formatted_message = format_commit_message_for_display(&commit_message);
    print!("{}", format_with_gutter(&formatted_message, "", None));

    // Reset to merge base (soft reset stages all changes)
    repo.run_command(&["reset", "--soft", &merge_base])
        .map_err(|e| GitError::CommandFailed(format!("Failed to reset to merge base: {}", e)))?;

    // Commit with the generated message
    repo.run_command(&["commit", "-m", &commit_message])
        .map_err(|e| GitError::CommandFailed(format!("Failed to create squash commit: {}", e)))?;

    // Show success immediately after completing the squash
    crate::output::success(format!("Squashed {commit_count} commits into one"))
        .map_err(|e| GitError::CommandFailed(e.to_string()))?;

    Ok(Some(commit_count))
}

/// Run pre-merge checks sequentially (blocking, fail-fast)
fn run_pre_merge_checks(
    project_config: &ProjectConfig,
    current_branch: &str,
    target_branch: &str,
    worktree_path: &std::path::Path,
    repo: &Repository,
    config: &WorktrunkConfig,
    force: bool,
) -> Result<(), GitError> {
    let Some(pre_merge_config) = &project_config.pre_merge_check else {
        return Ok(());
    };

    let ctx = CommandContext::new(repo, config, current_branch, worktree_path, force);
    let commands = prepare_project_commands(
        pre_merge_config,
        "cmd",
        &ctx,
        false,
        &[("target", target_branch)],
        "Pre-merge checks",
        |_, command| {
            let dim = AnstyleStyle::new().dimmed();
            eprintln!("{dim}Skipping pre-merge check: {command}{dim:#}");
        },
    )?;
    for prepared in commands {
        use std::io::Write;
        use worktrunk::styling;

        eprintln!(
            "🔄 {CYAN}Running pre-merge check '{name}':{CYAN:#}",
            name = prepared.name
        );
        eprint!("{}", format_with_gutter(&prepared.expanded, "", None)); // Gutter at column 0
        let _ = styling::stderr().flush();

        if let Err(e) = execute_command_in_worktree(worktree_path, &prepared.expanded) {
            return Err(GitError::PreMergeCheckFailed {
                check_name: prepared.name.clone(),
                error: e.to_string(),
            });
        }

        // No need to flush here - the redirect in execute_command_in_worktree ensures ordering
    }

    Ok(())
}

/// Load project configuration with proper error conversion
fn load_project_config(repo: &Repository) -> Result<Option<ProjectConfig>, GitError> {
    let repo_root = repo.worktree_root()?;
    ProjectConfig::load(&repo_root)
        .map_err(|e| GitError::CommandFailed(format!("Failed to load project config: {}", e)))
}

/// Execute post-merge commands sequentially in the main worktree (blocking)
fn execute_post_merge_commands(
    main_worktree_path: &std::path::Path,
    repo: &Repository,
    config: &WorktrunkConfig,
    branch: &str,
    target_branch: &str,
    force: bool,
) -> Result<(), GitError> {
    use worktrunk::styling::WARNING;

    let project_config = match load_project_config(repo)? {
        Some(cfg) => cfg,
        None => return Ok(()),
    };

    let Some(post_merge_config) = &project_config.post_merge_command else {
        return Ok(());
    };

    let ctx = CommandContext::new(repo, config, branch, main_worktree_path, force);
    let commands = prepare_project_commands(
        post_merge_config,
        "cmd",
        &ctx,
        false,
        &[("target", target_branch)],
        "Post-merge commands",
        |_, command| {
            let dim = AnstyleStyle::new().dimmed();
            eprintln!("{dim}Skipping command: {command}{dim:#}");
        },
    )?;

    if commands.is_empty() {
        return Ok(());
    }

    // Execute each command sequentially in the main worktree
    for prepared in commands {
        use std::io::Write;
        eprintln!("🔄 {CYAN}Executing (post-merge):{CYAN:#}");
        eprint!("{}", format_with_gutter(&prepared.expanded, "", None));
        let _ = std::io::stderr().flush();

        if let Err(e) = execute_command_in_worktree(main_worktree_path, &prepared.expanded) {
            use worktrunk::styling::WARNING_EMOJI;
            let warning_bold = WARNING.bold();
            eprintln!(
                "{WARNING_EMOJI} {WARNING}Command {warning_bold}{name}{warning_bold:#} failed: {e}{WARNING:#}",
                name = prepared.name,
            );
            // Continue with other commands even if one fails
        }
    }

    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();

    Ok(())
}
