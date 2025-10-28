use worktrunk::config::{ProjectConfig, WorktrunkConfig};
use worktrunk::git::{GitError, Repository};
use worktrunk::styling::{
    AnstyleStyle, CYAN, CYAN_BOLD, ERROR, ERROR_EMOJI, GREEN, GREEN_BOLD, HINT, HINT_EMOJI,
    SUCCESS_EMOJI, eprint, eprintln, format_with_gutter, println,
};

use super::command_executor::{CommandContext, prepare_project_commands};
use super::worktree::handle_push;
use super::worktree::handle_remove;
use crate::output::{execute_command_in_worktree, handle_remove_output};

pub fn handle_merge(
    target: Option<&str>,
    squash: bool,
    keep: bool,
    message: Option<&str>,
    no_verify: bool,
    force: bool,
) -> Result<(), GitError> {
    let repo = Repository::current();

    // Get current branch
    let current_branch = repo.current_branch()?.ok_or_else(|| {
        eprintln!("{ERROR_EMOJI} {ERROR}Not on a branch (detached HEAD){ERROR:#}");
        eprintln!();
        eprintln!("{HINT_EMOJI} {HINT}You are in detached HEAD state{HINT:#}");
        GitError::CommandFailed(String::new())
    })?;

    // Get target branch (default to default branch if not provided)
    let target_branch = target.map_or_else(|| repo.default_branch(), |b| Ok(b.to_string()))?;

    // Check if already on target branch
    if current_branch == target_branch {
        println!(
            "{SUCCESS_EMOJI} {GREEN}Already on {GREEN_BOLD}{target_branch}{GREEN_BOLD:#}, nothing to merge{GREEN:#}"
        );
        return Ok(());
    }

    // Load config for LLM integration
    let config = WorktrunkConfig::load()
        .map_err(|e| GitError::CommandFailed(format!("Failed to load config: {}", e)))?;

    // Run pre-merge checks unless --no-verify was specified
    // Do this BEFORE committing so we fail fast if checks won't pass
    if !no_verify && let Ok(Some(project_config)) = ProjectConfig::load(&repo.worktree_root()?) {
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

    // Track operations for summary
    let mut squashed_count: Option<usize> = None;

    // Squash commits if requested
    if squash {
        squashed_count = handle_squash(&target_branch)?;
    }

    // Rebase onto target
    crate::output::progress(format!(
        "🔄 {CYAN}Rebasing onto {CYAN_BOLD}{target_branch}{CYAN_BOLD:#}...{CYAN:#}"
    ))
    .map_err(|e| GitError::CommandFailed(e.to_string()))?;

    repo.run_command(&["rebase", &target_branch]).map_err(|e| {
        GitError::CommandFailed(format!("Failed to rebase onto '{}': {}", target_branch, e))
    })?;

    // Fast-forward push to target branch (reuse handle_push logic)
    handle_push(Some(&target_branch), false)?;

    // Finish worktree unless --keep was specified
    if !keep {
        crate::output::progress(format!("🔄 {CYAN}Cleaning up worktree...{CYAN:#}"))
            .map_err(|e| GitError::CommandFailed(e.to_string()))?;

        // Get primary worktree path before finishing (while we can still run git commands)
        let primary_worktree_dir = repo.main_worktree_root()?;

        let result = handle_remove(None)?;

        // Display output based on mode
        handle_remove_output(&result)?;

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
        handle_merge_summary_output(&current_branch, &target_branch, squashed_count, true)?;
    } else {
        // Print comprehensive summary (worktree preserved)
        println!();
        handle_merge_summary_output(&current_branch, &target_branch, squashed_count, false)?;
    }

    Ok(())
}

/// Format the merge summary message
fn format_merge_summary(
    from_branch: &str,
    to_branch: &str,
    squashed_count: Option<usize>,
    cleaned_up: bool,
) -> String {
    let bold = AnstyleStyle::new().bold();
    let dim = AnstyleStyle::new().dimmed();

    let mut output = "Merge complete\n\n".to_string();

    // Show what was merged
    output.push_str(&format!(
        "  {dim}Merged: {bold}{from_branch}{bold:#} → {bold}{to_branch}{bold:#}{dim:#}\n"
    ));

    // Show squash info if applicable
    if let Some(count) = squashed_count {
        output.push_str(&format!("  {dim}Squashed: {count} commits into 1{dim:#}\n"));
    }

    // Show worktree status
    if cleaned_up {
        output.push_str(&format!("  {dim}Worktree: Removed{dim:#}"));
    } else {
        output.push_str(&format!(
            "  {dim}Worktree: Kept (use 'wt remove' to clean up){dim:#}"
        ));
    }

    output
}

/// Handle output for merge summary using global output context
fn handle_merge_summary_output(
    from_branch: &str,
    to_branch: &str,
    squashed_count: Option<usize>,
    cleaned_up: bool,
) -> Result<(), GitError> {
    let message = format_merge_summary(from_branch, to_branch, squashed_count, cleaned_up);

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

    // Stage all tracked changes (excludes untracked files)
    repo.run_command(&["add", "-u"])
        .map_err(|e| GitError::CommandFailed(format!("Failed to stage changes: {}", e)))?;

    // Check if there are staged changes after staging
    if !repo.has_staged_changes()? {
        // No staged changes means only untracked files exist
        eprintln!("{ERROR_EMOJI} {ERROR}Working tree has untracked files{ERROR:#}");
        eprintln!();
        eprintln!("{HINT_EMOJI} {HINT}Add them with 'git add' and try again{HINT:#}");
        return Err(GitError::CommandFailed(String::new()));
    }

    // Generate commit message
    crate::output::progress(format!("🔄 {CYAN}Generating commit message...{CYAN:#}"))
        .map_err(|e| GitError::CommandFailed(e.to_string()))?;

    let commit_message =
        crate::llm::generate_commit_message(custom_instruction, commit_generation_config)?;

    // Display the generated commit message
    let formatted_message = format_commit_message_for_display(&commit_message);
    print!("{}", format_with_gutter(&formatted_message, ""));

    // Commit
    repo.run_command(&["commit", "-m", &commit_message])
        .map_err(|e| GitError::CommandFailed(format!("Failed to commit: {}", e)))?;

    println!("{SUCCESS_EMOJI} {GREEN}Committed changes{GREEN:#}");

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
        eprintln!("{ERROR_EMOJI} {ERROR}Staged changes without commits{ERROR:#}");
        eprintln!();
        eprintln!("{HINT_EMOJI} {HINT}Please commit them first{HINT:#}");
        return Err(GitError::CommandFailed(String::new()));
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
    print!("{}", format_with_gutter(&formatted_message, ""));

    // Reset to merge base (soft reset stages all changes)
    repo.run_command(&["reset", "--soft", &merge_base])
        .map_err(|e| GitError::CommandFailed(format!("Failed to reset to merge base: {}", e)))?;

    // Commit with the generated message
    repo.run_command(&["commit", "-m", &commit_message])
        .map_err(|e| GitError::CommandFailed(format!("Failed to create squash commit: {}", e)))?;

    crate::output::progress(format!(
        "{SUCCESS_EMOJI} {GREEN}Squashed {commit_count} commits into one{GREEN:#}"
    ))
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
        eprintln!(
            "🔄 {CYAN}Running pre-merge check '{name}':{CYAN:#}",
            name = prepared.name
        );
        eprint!("{}", format_with_gutter(&prepared.expanded, "")); // Gutter at column 0
        let _ = std::io::stderr().flush();

        if let Err(e) = execute_command_in_worktree(worktree_path, &prepared.expanded) {
            eprintln!();
            let error_bold = ERROR.bold();
            eprintln!(
                "{ERROR_EMOJI} {ERROR}Pre-merge check failed: {error_bold}{name}{error_bold:#}{ERROR:#}",
                name = prepared.name,
            );
            eprintln!();
            eprintln!("  {e}");
            eprintln!();
            eprintln!("{HINT_EMOJI} {HINT}Use --no-verify to skip pre-merge checks{HINT:#}");
            return Err(GitError::CommandFailed(String::new()));
        }
    }

    Ok(())
}
