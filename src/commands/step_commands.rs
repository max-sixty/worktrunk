//! Step commands for the merge workflow.
//!
//! This module contains the individual steps that make up `wt merge`:
//! - `step_commit` - Commit working tree changes
//! - `handle_squash` - Squash commits into one
//! - `step_show_squash_prompt` - Show squash prompt without executing
//! - `handle_rebase` - Rebase onto target branch
//! - `step_copy_ignored` - Copy gitignored files matching .worktreeinclude

use std::path::{Path, PathBuf};

use anyhow::Context;
use color_print::cformat;
use worktrunk::HookType;
use worktrunk::config::UserConfig;
use worktrunk::git::Repository;
use worktrunk::styling::{
    format_with_gutter, hint_message, info_message, progress_message, success_message,
    warning_message,
};

use super::commit::{CommitGenerator, CommitOptions};
use super::context::CommandEnv;
use super::hooks::{HookFailureStrategy, run_hook_with_filter};
use super::repository_ext::RepositoryCliExt;

/// Handle `wt step commit` command
///
/// `stage` is the CLI-provided stage mode. If None, uses the effective config default.
pub fn step_commit(
    yes: bool,
    no_verify: bool,
    stage: Option<super::commit::StageMode>,
    show_prompt: bool,
) -> anyhow::Result<()> {
    use super::command_approval::approve_hooks;

    // Handle --show-prompt early: just build and output the prompt
    if show_prompt {
        let repo = worktrunk::git::Repository::current()?;
        let config = UserConfig::load().context("Failed to load config")?;
        let project_id = repo.project_identifier().ok();
        let commit_config = config.commit_generation(project_id.as_deref());
        let prompt = crate::llm::build_commit_prompt(&commit_config)?;
        crate::output::stdout(prompt)?;
        return Ok(());
    }

    let env = CommandEnv::for_action("commit")?;
    let ctx = env.context(yes);

    // Determine effective stage mode: CLI > project config > global config > default
    let stage_mode = stage
        .or_else(|| env.commit().and_then(|c| c.stage))
        .unwrap_or_default();

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
/// * `stage` - CLI-provided stage mode. If None, uses the effective config default.
pub fn handle_squash(
    target: Option<&str>,
    yes: bool,
    skip_pre_commit: bool,
    stage: Option<super::commit::StageMode>,
) -> anyhow::Result<SquashResult> {
    use super::commit::StageMode;

    let env = CommandEnv::for_action("squash")?;
    let repo = &env.repo;
    // Squash requires being on a branch (can't squash in detached HEAD)
    let current_branch = env.require_branch("squash")?.to_string();
    let ctx = env.context(yes);
    let effective_config = env.commit_generation();
    let generator = CommitGenerator::new(&effective_config);

    // Determine effective stage mode: CLI > project config > global config > default
    let stage_mode = stage
        .or_else(|| env.commit().and_then(|c| c.stage))
        .unwrap_or_default();

    // Get and validate target ref (any commit-ish for merge-base calculation)
    let integration_target = repo.require_target_ref(target)?;

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
        let extra_vars = [("target", integration_target.as_str())];
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
        .merge_base("HEAD", &integration_target)?
        .context("Cannot squash: no common ancestor with target branch")?;

    // Count commits since merge base
    let commit_count = repo.count_commits(&merge_base, "HEAD")?;

    // Check if there are staged changes in addition to commits
    let wt = repo.current_worktree();
    let has_staged = wt.has_staged_changes()?;

    // Handle different scenarios
    if commit_count == 0 && !has_staged {
        // No commits and no staged changes - nothing to squash
        return Ok(SquashResult::NoCommitsAhead(integration_target));
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
        let backup_message = format!("{} → {} (squash)", current_branch, integration_target);
        let sha = wt.create_safety_backup(&backup_message)?;
        crate::output::print(hint_message(format!("Backup created @ {sha}")))?;
    }

    // Get commit subjects for the squash message
    let subjects = repo.commit_subjects(&range)?;

    // Generate squash commit message
    crate::output::print(progress_message("Generating squash commit message..."))?;

    generator.emit_hint_if_needed()?;

    // Get current branch and repo name for template variables
    let repo_root = wt.root()?;
    let repo_name = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo");

    let commit_message = crate::llm::generate_squash_message(
        &integration_target,
        &merge_base,
        &subjects,
        &current_branch,
        repo_name,
        &effective_config,
    )?;

    // Display the generated commit message
    let formatted_message = generator.format_message_for_display(&commit_message);
    crate::output::print(format_with_gutter(&formatted_message, None))?;

    // Reset to merge base (soft reset stages all changes, including any already-staged uncommitted changes)
    repo.run_command(&["reset", "--soft", &merge_base])
        .context("Failed to reset to merge base")?;

    // Check if there are actually any changes to commit
    if !wt.has_staged_changes()? {
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
pub fn step_show_squash_prompt(target: Option<&str>) -> anyhow::Result<()> {
    let repo = Repository::current()?;
    let config = UserConfig::load().context("Failed to load config")?;
    let project_id = repo.project_identifier().ok();
    let effective_config = config.commit_generation(project_id.as_deref());

    // Get and validate target ref (any commit-ish for merge-base calculation)
    let integration_target = repo.require_target_ref(target)?;

    // Get current branch
    let wt = repo.current_worktree();
    let current_branch = wt.branch()?.unwrap_or_else(|| "HEAD".to_string());

    // Get merge base with target branch (required for generating squash message)
    let merge_base = repo
        .merge_base("HEAD", &integration_target)?
        .context("Cannot generate squash message: no common ancestor with target branch")?;

    // Get commit subjects for the squash message
    let range = format!("{}..HEAD", merge_base);
    let subjects = repo.commit_subjects(&range)?;

    // Get repo name from directory
    let repo_root = wt.root()?;
    let repo_name = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo");

    let prompt = crate::llm::build_squash_prompt(
        &integration_target,
        &merge_base,
        &subjects,
        &current_branch,
        repo_name,
        &effective_config,
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
    let repo = Repository::current()?;

    // Get and validate target ref (any commit-ish for rebase)
    let integration_target = repo.require_target_ref(target)?;

    // Check if already up-to-date (linear extension of target, no merge commits)
    if repo.is_rebased_onto(&integration_target)? {
        return Ok(RebaseResult::UpToDate(integration_target));
    }

    // Check if this is a fast-forward or true rebase
    let merge_base = repo
        .merge_base("HEAD", &integration_target)?
        .context("Cannot rebase: no common ancestor with target branch")?;
    let head_sha = repo.run_command(&["rev-parse", "HEAD"])?.trim().to_string();
    let is_fast_forward = merge_base == head_sha;

    // Only show progress for true rebases (fast-forwards are instant)
    if !is_fast_forward {
        crate::output::print(progress_message(cformat!(
            "Rebasing onto <bold>{integration_target}</>..."
        )))?;
    }

    let rebase_result = repo.run_command(&["rebase", &integration_target]);

    // If rebase failed, check if it's due to conflicts
    if let Err(e) = rebase_result {
        if let Some(state) = repo.worktree_state()?
            && state.starts_with("REBASING")
        {
            // Extract git's stderr output from the error
            let git_output = e.to_string();
            return Err(worktrunk::git::GitError::RebaseConflict {
                target_branch: integration_target.clone(),
                git_output,
            }
            .into());
        }
        // Not a rebase conflict, return original error
        return Err(worktrunk::git::GitError::Other {
            message: cformat!(
                "Failed to rebase onto <bold>{}</>: {}",
                integration_target,
                e
            ),
        }
        .into());
    }

    // Verify rebase completed successfully (safety check for edge cases)
    if let Some(state) = repo.worktree_state()? {
        let _ = state; // used for diagnostics
        return Err(worktrunk::git::GitError::RebaseConflict {
            target_branch: integration_target.clone(),
            git_output: String::new(),
        }
        .into());
    }

    // Success
    if is_fast_forward {
        crate::output::print(success_message(cformat!(
            "Fast-forwarded to <bold>{integration_target}</>"
        )))?;
    } else {
        crate::output::print(success_message(cformat!(
            "Rebased onto <bold>{integration_target}</>"
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

    // Filter out entries that contain other worktrees (prevents recursive copying when
    // worktrees are nested inside the source, e.g., worktree-path = ".worktrees/...")
    let worktree_paths: Vec<PathBuf> = repo
        .list_worktrees()?
        .into_iter()
        .map(|wt| wt.path)
        .collect();
    let entries_to_copy: Vec<_> = entries_to_copy
        .into_iter()
        .filter(|(entry_path, _)| {
            // Exclude if any worktree (other than source) is inside or equal to this entry
            !worktree_paths
                .iter()
                .any(|wt_path| wt_path != &source_path && wt_path.starts_with(entry_path))
        })
        .collect();

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
/// Uses file-by-file copying with per-file reflink on all platforms. This spreads
/// I/O operations over time rather than issuing them in a single burst.
///
/// ## Why not use atomic directory cloning on macOS?
///
/// macOS/APFS supports `clonefile()` on directories, which clones an entire tree
/// atomically. However, Apple explicitly discourages this in the man page:
///
/// > "Cloning directories with these functions is strongly discouraged.
/// > Use copyfile(3) to clone directories instead."
/// > — clonefile(2) man page
///
/// In practice, atomic `clonefile()` on a Rust `target/` directory (~236K files)
/// saturates disk I/O at ~45K ops/sec, blocking interactive processes like shell
/// startup for several seconds. The per-file approach spreads operations over
/// time, keeping the system responsive even though total copy time is longer.
///
/// Apple recommends `copyfile()` with `COPYFILE_CLONE` for directories, which
/// internally walks the tree and clones per-file — equivalent to what we do here.
fn copy_dir_recursive(src: &Path, dest: &Path) -> anyhow::Result<()> {
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
        let src_path = entry.path();
        let dest_path = dest.join(entry.file_name());

        if file_type.is_symlink() {
            // Copy symlink (preserves the link, doesn't follow it)
            if !dest_path.exists() {
                let target = fs::read_link(&src_path)?;
                #[cfg(unix)]
                std::os::unix::fs::symlink(&target, &dest_path)?;
                #[cfg(windows)]
                {
                    // Check source to determine symlink type (target may be relative/broken)
                    let is_dir = src_path.metadata().map(|m| m.is_dir()).unwrap_or(false);
                    if is_dir {
                        std::os::windows::fs::symlink_dir(&target, &dest_path)?;
                    } else {
                        std::os::windows::fs::symlink_file(&target, &dest_path)?;
                    }
                }
            }
        } else if file_type.is_dir() {
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

/// Move worktrees to their expected paths based on the `worktree-path` template.
///
/// # Invariant
///
/// **`--commit --clobber` should never fail** (except for truly unrecoverable errors
/// like disk full or permissions denied).
///
/// # Flags
///
/// | Flag | Purpose |
/// |------|---------|
/// | `--dry-run` | Show what would be moved without moving |
/// | `--commit` | Auto-commit dirty worktrees before relocating |
/// | `--clobber` | Move non-worktree paths out of the way (`<path>.bak-<timestamp>`) |
///
/// # Failure Cases
///
/// The command should **only skip** when:
///
/// | Condition | Without Flag | With Flag |
/// |-----------|--------------|-----------|
/// | Dirty worktree | Skip | `--commit`: auto-commit, then move |
/// | Locked worktree | Skip | Must `git worktree unlock` manually |
/// | Non-worktree at target | Skip | `--clobber`: backup and move |
///
/// Main worktree with non-default branch: creates new worktree at expected path, switches
/// main back to default branch (can't use `git worktree move` on main worktree).
///
/// When worktrees occupy each other's target paths, the algorithm uses a temp location
/// to break the dependency.
///
/// # Target Classification
///
/// For each mismatched worktree, classify its target path:
///
/// | Classification | Description | Action |
/// |----------------|-------------|--------|
/// | `Empty` | Target doesn't exist | Move directly |
/// | `Worktree` | Target is another worktree we're relocating | Coordinate via dependency graph |
/// | `Blocked` | Non-worktree exists at target | Skip or clobber |
///
/// # Algorithm
///
/// ```text
/// 1. Gather candidates: worktrees where current_path != expected_path
///
/// 2. Pre-check each candidate:
///    - Locked: skip (user must unlock manually)
///    - Dirty without --commit: skip
///    - Dirty with --commit: auto-commit
///
/// 3. Classify targets and handle blockers:
///    - Empty: ready to move
///    - Another worktree we're moving: add to dependency graph
///    - Non-worktree without --clobber: skip
///    - Non-worktree with --clobber: backup to <path>.bak-<timestamp>
///
/// 4. Build dependency graph:
///    - Edge A→B means "A's target is currently occupied by worktree B"
///
/// 5. Process in topological order:
///    - Find worktrees whose target is empty (no incoming edges)
///    - Move them, remove from graph
///    - Repeat until only cycles remain
///
/// 6. Resolve cycles with temp locations:
///    - Pick one worktree from cycle, move to .git/wt-relocate-tmp/
///    - Continue processing (cycle is now broken)
///    - After all others done, move from temp to final location
/// ```
///
/// # Example: Cycle Resolution
///
/// ```text
/// Before:
///   alpha @ repo.beta    (wants repo.alpha)
///   beta  @ repo.alpha   (wants repo.beta)
///
/// Processing:
///   1. Neither target is empty, so move alpha → .git/wt-relocate-tmp/alpha
///   2. Move beta → repo.beta (target now empty)
///   3. Move alpha from temp → repo.alpha
/// ```
///
/// # Temp Location
///
/// Uses `.git/wt-relocate-tmp/` inside the main worktree's git directory:
/// - Same filesystem (atomic moves)
/// - Inside .git (invisible to user)
/// - Cleaned up after successful relocation
pub fn step_relocate(
    branches: Vec<String>,
    dry_run: bool,
    commit: bool,
    clobber: bool,
) -> anyhow::Result<()> {
    use super::worktree::{get_path_mismatch, paths_match};
    use worktrunk::path::format_path_for_display;
    use worktrunk::shell_exec::Cmd;

    let repo = Repository::current()?;
    let config = UserConfig::load()?;
    let default_branch = repo.default_branch().unwrap_or_default();
    let repo_path = repo.repo_path().to_path_buf();

    // Get all worktrees, excluding prunable ones
    let worktrees: Vec<_> = repo
        .list_worktrees()?
        .into_iter()
        .filter(|wt| wt.prunable.is_none())
        .collect();

    // Filter to requested branches if any were specified
    let candidates: Vec<_> = if branches.is_empty() {
        worktrees
    } else {
        worktrees
            .into_iter()
            .filter(|wt| {
                wt.branch
                    .as_ref()
                    .is_some_and(|b| branches.iter().any(|arg| arg == b))
            })
            .collect()
    };

    // Find mismatched worktrees (worktrees not at their expected paths)
    let mismatched: Vec<_> = candidates
        .into_iter()
        .filter_map(|wt| {
            let branch = wt.branch.as_deref()?;
            get_path_mismatch(&repo, branch, &wt.path, &config).map(|expected| (wt, expected))
        })
        .collect();

    if mismatched.is_empty() {
        crate::output::print(info_message("All worktrees are at expected paths"))?;
        return Ok(());
    }

    // Dry run: show preview
    if dry_run {
        crate::output::print(info_message(format!(
            "{} worktree{} would be relocated:",
            mismatched.len(),
            if mismatched.len() == 1 { "" } else { "s" }
        )))?;
        crate::output::blank()?;

        for (wt, expected_path) in &mismatched {
            let branch = wt.branch.as_deref().unwrap();
            let src_display = format_path_for_display(&wt.path);
            let dest_display = format_path_for_display(expected_path);

            crate::output::print(cformat!(
                "  <bold>{branch}</>: {src_display} → {dest_display}"
            ))?;
        }
        return Ok(());
    }

    // Phase 1: Pre-check locked/dirty and filter to processable worktrees
    let cwd = std::env::current_dir().ok();
    let mut skipped = 0;
    let mut pending: Vec<(worktrunk::git::WorktreeInfo, PathBuf)> = Vec::new();

    for (wt, expected_path) in mismatched {
        let branch = wt.branch.as_deref().unwrap();

        // Check locked - always skip (user must unlock manually)
        if let Some(reason) = &wt.locked {
            let reason_text = if reason.is_empty() {
                String::new()
            } else {
                format!(": {reason}")
            };
            crate::output::print(warning_message(cformat!(
                "Skipping <bold>{branch}</> (locked{reason_text})"
            )))?;
            skipped += 1;
            continue;
        }

        // Check dirty
        let worktree = repo.worktree_at(&wt.path);
        if worktree.is_dirty()? {
            if commit {
                crate::output::print(progress_message(cformat!(
                    "Committing changes in <bold>{branch}</>..."
                )))?;
                commit_worktree_changes(&repo, &wt.path, &config)?;
            } else {
                crate::output::print(warning_message(cformat!(
                    "Skipping <bold>{branch}</> (uncommitted changes)"
                )))?;
                crate::output::print(hint_message(
                    "Use --commit to auto-commit changes before relocating",
                ))?;
                skipped += 1;
                continue;
            }
        }

        pending.push((wt, expected_path));
    }

    if pending.is_empty() {
        if skipped > 0 {
            crate::output::blank()?;
            crate::output::print(info_message(format!(
                "Skipped {skipped} worktree{}",
                if skipped == 1 { "" } else { "s" }
            )))?;
        }
        return Ok(());
    }

    // Phase 2: Build map of current locations (to detect swaps/cycles)
    // Maps: canonical target path → index in pending (if target is a worktree we're moving)
    let mut current_locations: std::collections::HashMap<PathBuf, usize> =
        std::collections::HashMap::new();
    for (i, (wt, _)) in pending.iter().enumerate() {
        // Canonicalize to handle symlinks
        let canonical = wt.path.canonicalize().unwrap_or_else(|_| wt.path.clone());
        current_locations.insert(canonical, i);
    }

    // Phase 3: Classify targets and handle blockers
    // Track which pending items are blocked by non-worktrees
    let mut blocked_by_non_worktree: std::collections::HashSet<usize> =
        std::collections::HashSet::new();

    for (i, (_, expected_path)) in pending.iter().enumerate() {
        if !expected_path.exists() {
            continue; // Target is empty, no blocker
        }

        let canonical_target = expected_path
            .canonicalize()
            .unwrap_or_else(|_| expected_path.clone());

        if current_locations.contains_key(&canonical_target) {
            // Target is another worktree we're moving - will handle via dependency graph
            continue;
        }

        // Target exists but is NOT a worktree we're moving - it's a blocker
        let branch = pending[i].0.branch.as_deref().unwrap();
        if clobber {
            // Backup the blocker (use get_now() for deterministic timestamps in tests)
            let timestamp_secs = worktrunk::utils::get_now() as i64;
            let datetime = chrono::DateTime::from_timestamp(timestamp_secs, 0)
                .unwrap_or_else(chrono::Utc::now);
            let suffix = datetime.format("%Y%m%d-%H%M%S");
            let backup_path = expected_path.with_file_name(format!(
                "{}.bak-{suffix}",
                expected_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
            ));
            crate::output::print(progress_message(cformat!(
                "Backing up {} → {}",
                format_path_for_display(expected_path),
                format_path_for_display(&backup_path)
            )))?;
            std::fs::rename(expected_path, &backup_path)
                .with_context(|| format!("Failed to backup {}", expected_path.display()))?;
        } else {
            crate::output::print(warning_message(cformat!(
                "Skipping <bold>{branch}</> (target blocked: {})",
                format_path_for_display(expected_path)
            )))?;
            crate::output::print(hint_message("Use --clobber to backup blocking paths"))?;
            blocked_by_non_worktree.insert(i);
            skipped += 1;
        }
    }

    // Phase 4: Build dependency graph and process
    // We process worktrees whose target is empty first, then handle cycles
    let temp_dir = repo.git_common_dir().join("wt-relocate-tmp");
    let mut relocated = 0;
    let mut moved_indices: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut temp_relocated: Vec<(usize, PathBuf)> = Vec::new(); // (index, temp_path)

    // Helper to check if a target is currently empty (not occupied by a pending worktree)
    let is_target_empty = |idx: usize,
                           pending: &[(worktrunk::git::WorktreeInfo, PathBuf)],
                           moved: &std::collections::HashSet<usize>,
                           current_locs: &std::collections::HashMap<PathBuf, usize>|
     -> bool {
        let expected = &pending[idx].1;
        if !expected.exists() {
            return true;
        }
        let canonical = expected.canonicalize().unwrap_or_else(|_| expected.clone());
        match current_locs.get(&canonical) {
            Some(&occupant_idx) => moved.contains(&occupant_idx), // Occupant already moved
            None => false, // Non-worktree blocker (should have been handled)
        }
    };

    // Process until all pending are moved or in temp
    loop {
        let mut made_progress = false;

        // Find worktrees whose target is now empty
        for i in 0..pending.len() {
            if moved_indices.contains(&i) || blocked_by_non_worktree.contains(&i) {
                continue;
            }

            if is_target_empty(i, &pending, &moved_indices, &current_locations) {
                let (wt, expected_path) = &pending[i];
                let branch = wt.branch.as_deref().unwrap();
                let is_main = paths_match(&wt.path, &repo_path);

                let src_display = format_path_for_display(&wt.path);
                let dest_display = format_path_for_display(expected_path);

                if is_main {
                    // Main worktree: switch to default first, then create new wt
                    crate::output::print(progress_message(cformat!(
                        "Switching main worktree to <bold>{}</>...",
                        default_branch
                    )))?;

                    Cmd::new("git")
                        .args(["checkout", &default_branch])
                        .current_dir(&repo_path)
                        .context("main")
                        .run()
                        .context("Failed to checkout default branch")?;

                    Cmd::new("git")
                        .args(["worktree", "add"])
                        .arg(expected_path.to_string_lossy())
                        .arg(branch)
                        .context(branch)
                        .run()
                        .context("Failed to create worktree")?;
                } else {
                    Cmd::new("git")
                        .args(["worktree", "move"])
                        .arg(wt.path.to_string_lossy())
                        .arg(expected_path.to_string_lossy())
                        .context(branch)
                        .run()
                        .context("Failed to move worktree")?;
                }

                crate::output::print(success_message(cformat!(
                    "Relocated <bold>{branch}</>: {src_display} → {dest_display}"
                )))?;

                // Update shell if user is inside
                if let Some(ref cwd_path) = cwd
                    && cwd_path.starts_with(&wt.path)
                {
                    let relative = cwd_path.strip_prefix(&wt.path).unwrap_or(Path::new(""));
                    crate::output::change_directory(expected_path.join(relative))?;
                }

                moved_indices.insert(i);
                relocated += 1;
                made_progress = true;
            }
        }

        if made_progress {
            continue;
        }

        // No progress - we have a cycle. Break it by moving one to temp.
        let cycle_idx = (0..pending.len())
            .find(|&i| !moved_indices.contains(&i) && !blocked_by_non_worktree.contains(&i));

        match cycle_idx {
            Some(i) => {
                let (wt, _) = &pending[i];
                let branch = wt.branch.as_deref().unwrap();

                // Create temp directory if needed
                if !temp_dir.exists() {
                    std::fs::create_dir_all(&temp_dir)?;
                }

                let temp_path = temp_dir.join(branch);
                crate::output::print(progress_message(cformat!(
                    "Moving <bold>{branch}</> to temporary location..."
                )))?;

                Cmd::new("git")
                    .args(["worktree", "move"])
                    .arg(wt.path.to_string_lossy())
                    .arg(temp_path.to_string_lossy())
                    .context(branch)
                    .run()
                    .context("Failed to move worktree to temp")?;

                // Update current_locations to reflect the move
                let old_canonical = wt.path.canonicalize().unwrap_or_else(|_| wt.path.clone());
                current_locations.remove(&old_canonical);

                temp_relocated.push((i, temp_path.clone()));
                moved_indices.insert(i);
            }
            None => break, // All done
        }
    }

    // Phase 5: Move temp-relocated worktrees to final destinations
    for (i, temp_path) in temp_relocated {
        let (_, expected_path) = &pending[i];
        let branch = pending[i].0.branch.as_deref().unwrap();

        let dest_display = format_path_for_display(expected_path);

        Cmd::new("git")
            .args(["worktree", "move"])
            .arg(temp_path.to_string_lossy())
            .arg(expected_path.to_string_lossy())
            .context(branch)
            .run()
            .context("Failed to move worktree from temp to final location")?;

        crate::output::print(success_message(cformat!(
            "Relocated <bold>{branch}</> → {dest_display}"
        )))?;

        // Update shell if user was inside
        if let Some(ref cwd_path) = cwd
            && cwd_path.starts_with(&temp_path)
        {
            let relative = cwd_path.strip_prefix(&temp_path).unwrap_or(Path::new(""));
            crate::output::change_directory(expected_path.join(relative))?;
        }

        relocated += 1;
    }

    // Clean up temp directory if empty
    if temp_dir.exists() {
        let _ = std::fs::remove_dir(&temp_dir); // Ignore error if not empty
    }

    // Summary
    if relocated > 0 || skipped > 0 {
        crate::output::blank()?;
        let relocated_word = if relocated == 1 {
            "worktree"
        } else {
            "worktrees"
        };
        if skipped == 0 {
            crate::output::print(success_message(format!(
                "Relocated {relocated} {relocated_word}"
            )))?;
        } else {
            let skipped_word = if skipped == 1 {
                "worktree"
            } else {
                "worktrees"
            };
            crate::output::print(info_message(format!(
                "Relocated {relocated} {relocated_word}, skipped {skipped} {skipped_word}"
            )))?;
        }
    }

    Ok(())
}
/// Commit changes in a specific worktree using LLM-generated message.
fn commit_worktree_changes(
    repo: &Repository,
    worktree_path: &Path,
    config: &UserConfig,
) -> anyhow::Result<()> {
    use worktrunk::shell_exec::Cmd;

    let project_id = repo.project_identifier().ok();
    let commit_config = config.commit_generation(project_id.as_deref());
    let generator = super::commit::CommitGenerator::new(&commit_config);

    // Stage all changes
    Cmd::new("git")
        .args(["add", "-A"])
        .current_dir(worktree_path)
        .run()
        .context("Failed to stage changes")?;

    // Check if there are staged changes
    let worktree = repo.worktree_at(worktree_path);
    if !worktree.has_staged_changes()? {
        return Ok(()); // Nothing to commit
    }

    generator.emit_hint_if_needed()?;
    let commit_message = crate::llm::generate_commit_message(&commit_config)?;

    let formatted_message = generator.format_message_for_display(&commit_message);
    crate::output::print(format_with_gutter(&formatted_message, None))?;

    Cmd::new("git")
        .args(["commit", "-m", &commit_message])
        .current_dir(worktree_path)
        .run()
        .context("Failed to commit")?;

    let commit_hash = Cmd::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(worktree_path)
        .run()?;
    let commit_hash = String::from_utf8_lossy(&commit_hash.stdout)
        .trim()
        .to_string();

    crate::output::print(success_message(cformat!(
        "Committed @ <dim>{commit_hash}</>"
    )))?;

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
