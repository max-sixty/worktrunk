//! Step commands for the merge workflow.
//!
//! This module contains the individual steps that make up `wt merge`:
//! - `step_commit` - Commit working tree changes
//! - `handle_squash` - Squash commits into one
//! - `step_show_squash_prompt` - Show squash prompt without executing
//! - `handle_rebase` - Rebase onto target branch
//! - `step_copy_ignored` - Copy gitignored files matching .worktreeinclude

use std::path::Path;

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

    // Get and validate target ref (any commit-ish for merge-base calculation)
    let target_branch = repo.require_target_ref(target)?;

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
        crate::output::print(info_message("Skipping pre-commit hooks (--no-verify)"))?;
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

    // Get merge base with target branch (required for squash)
    let merge_base = repo
        .merge_base("HEAD", &target_branch)?
        .context("Cannot squash: no common ancestor with target branch")?;

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
    let repo_root = repo.current_worktree().root()?;
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
    let repo = Repository::current()?;

    // Get and validate target ref (any commit-ish for merge-base calculation)
    let target_branch = repo.require_target_ref(target)?;

    // Get current branch
    let current_branch = repo
        .current_worktree()
        .branch()?
        .unwrap_or_else(|| "HEAD".to_string());

    // Get merge base with target branch (required for generating squash message)
    let merge_base = repo
        .merge_base("HEAD", &target_branch)?
        .context("Cannot generate squash message: no common ancestor with target branch")?;

    // Get commit subjects for the squash message
    let range = format!("{}..HEAD", merge_base);
    let subjects = repo.commit_subjects(&range)?;

    // Get repo name from directory
    let repo_root = repo.current_worktree().root()?;
    let repo_name = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo");

    let prompt = crate::llm::build_squash_prompt(
        &target_branch,
        &merge_base,
        &subjects,
        &current_branch,
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

    let repo = Repository::current()?;

    // Get and validate target ref (any commit-ish for rebase)
    let target_branch = repo.require_target_ref(target)?;

    // Check if already up-to-date (linear extension of target, no merge commits)
    if repo.is_rebased_onto(&target_branch)? {
        return Ok(RebaseResult::UpToDate(target_branch));
    }

    // Check if this is a fast-forward or true rebase
    let merge_base = repo
        .merge_base("HEAD", &target_branch)?
        .context("Cannot rebase: no common ancestor with target branch")?;
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
            message: cformat!("Failed to rebase onto <bold>{}</>: {}", target_branch, e),
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

/// Handle `wt step copy-ignored` command
///
/// Copies gitignored files from a source worktree to a destination worktree.
/// If a `.worktreeinclude` file exists, only files matching both `.worktreeinclude`
/// and gitignore patterns are copied. Without `.worktreeinclude`, all gitignored
/// files are copied. Uses COW (reflink) when available for efficient copying of
/// large directories like `target/`.
pub fn step_copy_ignored(
    from: Option<&str>,
    to: Option<&str>,
    dry_run: bool,
) -> anyhow::Result<()> {
    use ignore::gitignore::GitignoreBuilder;
    use std::fs;

    let repo = Repository::current()?;

    // Resolve source and destination worktree paths
    let (source_path, source_context) = match from {
        Some(branch) => {
            let path = repo.worktree_for_branch(branch)?.ok_or_else(|| {
                worktrunk::git::GitError::WorktreeNotFound {
                    branch: branch.to_string(),
                }
            })?;
            (path, branch.to_string())
        }
        None => {
            // Default source is the primary worktree (main worktree for normal repos,
            // default branch worktree for bare repos).
            let path = repo.primary_worktree()?.ok_or_else(|| {
                anyhow::anyhow!(
                    "No primary worktree found (bare repo with no default branch worktree)"
                )
            })?;
            let context = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            (path, context)
        }
    };

    let dest_path = match to {
        Some(branch) => repo.worktree_for_branch(branch)?.ok_or_else(|| {
            worktrunk::git::GitError::WorktreeNotFound {
                branch: branch.to_string(),
            }
        })?,
        None => repo.current_worktree().root()?.to_path_buf(),
    };

    if source_path == dest_path {
        crate::output::print(info_message("Source and destination are the same worktree"))?;
        return Ok(());
    }

    // Get ignored entries from git
    // --directory stops at directory boundaries (avoids listing thousands of files in target/)
    let ignored_entries = list_ignored_entries(&source_path, &source_context)?;

    // Filter to entries that match .worktreeinclude (or all if no file exists)
    let include_path = source_path.join(".worktreeinclude");
    let entries_to_copy: Vec<_> = if include_path.exists() {
        // Build include matcher from .worktreeinclude
        let include_matcher = {
            let mut builder = GitignoreBuilder::new(&source_path);
            if let Some(err) = builder.add(&include_path) {
                return Err(worktrunk::git::GitError::WorktreeIncludeParseError {
                    error: err.to_string(),
                }
                .into());
            }
            builder.build().context("Failed to build include matcher")?
        };
        ignored_entries
            .into_iter()
            .filter(|(path, is_dir)| include_matcher.matched(path, *is_dir).is_ignore())
            .collect()
    } else {
        // No .worktreeinclude file — default to copying all ignored entries
        ignored_entries
    };

    if entries_to_copy.is_empty() {
        crate::output::print(info_message("No matching files to copy"))?;
        return Ok(());
    }

    let mut copied_count = 0;

    // Handle dry-run: show what would be copied in a gutter list
    if dry_run {
        let items: Vec<String> = entries_to_copy
            .iter()
            .map(|(src_entry, is_dir)| {
                let relative = src_entry
                    .strip_prefix(&source_path)
                    .unwrap_or(src_entry.as_path());
                let entry_type = if *is_dir { "dir" } else { "file" };
                format!("{} ({})", relative.display(), entry_type)
            })
            .collect();
        let entry_word = if items.len() == 1 { "entry" } else { "entries" };
        crate::output::print(info_message(format!(
            "Would copy {} {}:\n{}",
            items.len(),
            entry_word,
            format_with_gutter(&items.join("\n"), None)
        )))?;
        return Ok(());
    }

    // Copy entries
    for (src_entry, is_dir) in &entries_to_copy {
        // Paths from git ls-files are always under source_path
        let relative = src_entry
            .strip_prefix(&source_path)
            .expect("git ls-files path under worktree");
        let dest_entry = dest_path.join(relative);

        if *is_dir {
            copy_dir_recursive(src_entry, &dest_entry)?;
            copied_count += 1;
        } else {
            if let Some(parent) = dest_entry.parent() {
                fs::create_dir_all(parent)?;
            }
            // Skip existing files for idempotent hook usage
            match reflink_copy::reflink_or_copy(src_entry, &dest_entry) {
                Ok(_) => copied_count += 1,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.into()),
            }
        }
    }

    // Show summary
    let entry_word = if copied_count == 1 {
        "entry"
    } else {
        "entries"
    };
    crate::output::print(success_message(format!(
        "Copied {copied_count} {entry_word}"
    )))?;

    Ok(())
}

/// List ignored entries using git ls-files
///
/// Uses `git ls-files --ignored --exclude-standard -o --directory` which:
/// - Handles all gitignore sources (global, .gitignore, .git/info/exclude, nested)
/// - Stops at directory boundaries (--directory) to avoid listing thousands of files
fn list_ignored_entries(
    worktree_path: &Path,
    context: &str,
) -> anyhow::Result<Vec<(std::path::PathBuf, bool)>> {
    use worktrunk::shell_exec::Cmd;

    let output = Cmd::new("git")
        .args([
            "ls-files",
            "--ignored",
            "--exclude-standard",
            "-o",
            "--directory",
        ])
        .current_dir(worktree_path)
        .context(context)
        .run()
        .context("Failed to run git ls-files")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git ls-files failed: {}", stderr.trim());
    }

    // Parse output: directories end with /
    let entries = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| {
            let is_dir = line.ends_with('/');
            let path = worktree_path.join(line.trim_end_matches('/'));
            (path, is_dir)
        })
        .collect();

    Ok(entries)
}

/// Copy a directory recursively using reflink (COW).
///
/// On macOS/APFS, attempts atomic directory clone first via `clonefile()` syscall,
/// which is O(1) regardless of file count. Falls back to file-by-file copying on
/// other platforms or when atomic clone fails.
fn copy_dir_recursive(src: &Path, dest: &Path) -> anyhow::Result<()> {
    // On macOS, try atomic directory clone first (single syscall for entire tree)
    // This is dramatically faster for large directories like target/ with 100k+ files
    #[cfg(target_os = "macos")]
    {
        use std::io::ErrorKind;

        // reflink::reflink() on macOS uses clonefile() which supports directories
        match reflink_copy::reflink(src, dest) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                // Destination exists - fall through to file-by-file for idempotent merging
            }
            Err(e) => {
                // clonefile failed (cross-device, not APFS, etc.) - fall through
                log::debug!("Atomic directory clone failed, using file-by-file fallback: {e}");
            }
        }
    }

    // Fallback: file-by-file copying (works on all platforms)
    copy_dir_recursive_fallback(src, dest)
}

/// File-by-file recursive copy with reflink per file.
///
/// Used as fallback when atomic directory clone isn't available or fails.
fn copy_dir_recursive_fallback(src: &Path, dest: &Path) -> anyhow::Result<()> {
    use std::fs;
    use std::io::ErrorKind;

    fs::create_dir_all(dest)?;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;

        // Skip symlinks
        if file_type.is_symlink() {
            continue;
        }

        let src_path = entry.path();
        let dest_path = dest.join(entry.file_name());

        if file_type.is_dir() {
            copy_dir_recursive_fallback(&src_path, &dest_path)?;
        } else {
            // Skip existing files for idempotent hook usage
            match reflink_copy::reflink_or_copy(&src_path, &dest_path) {
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.into()),
            }
        }
    }

    Ok(())
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
