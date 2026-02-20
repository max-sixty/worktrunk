//! Step commands for the merge workflow and standalone worktree utilities.
//!
//! Merge steps:
//! - `step_commit` - Commit working tree changes
//! - `handle_squash` - Squash commits into one
//! - `step_show_squash_prompt` - Show squash prompt without executing
//! - `handle_rebase` - Rebase onto target branch
//! - `step_diff` - Show all changes since branching
//!
//! Standalone:
//! - `step_copy_ignored` - Copy gitignored files matching .worktreeinclude
//! - `handle_promote` - Put a branch into the main worktree

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::Context;
use color_print::cformat;
use ignore::gitignore::GitignoreBuilder;
use worktrunk::HookType;
use worktrunk::config::UserConfig;
use worktrunk::git::Repository;
use worktrunk::styling::{
    eprintln, format_with_gutter, hint_message, info_message, progress_message, success_message,
    verbosity, warning_message,
};

use super::command_approval::approve_hooks;
use super::commit::{CommitGenerator, CommitOptions, StageMode};
use super::context::CommandEnv;
use super::hooks::{HookFailureStrategy, run_hook_with_filter};
use super::repository_ext::RepositoryCliExt;
use worktrunk::shell_exec::Cmd;

/// Handle `wt step commit` command
///
/// `stage` is the CLI-provided stage mode. If None, uses the effective config default.
pub fn step_commit(
    yes: bool,
    no_verify: bool,
    stage: Option<StageMode>,
    show_prompt: bool,
) -> anyhow::Result<()> {
    // Handle --show-prompt early: just build and output the prompt
    if show_prompt {
        let repo = worktrunk::git::Repository::current()?;
        let config = UserConfig::load().context("Failed to load config")?;
        let project_id = repo.project_identifier().ok();
        let commit_config = config.commit_generation(project_id.as_deref());
        let prompt = crate::llm::build_commit_prompt(&commit_config)?;
        println!("{}", prompt);
        return Ok(());
    }

    // Load config once, run LLM setup prompt, then reuse config
    let mut config = UserConfig::load().context("Failed to load config")?;
    // One-time LLM setup prompt (errors logged internally; don't block commit)
    let _ = crate::output::prompt_commit_generation(&mut config);

    let env = CommandEnv::for_action("commit", config)?;
    let ctx = env.context(yes);

    // CLI flag overrides config value
    let stage_mode = stage.unwrap_or(env.resolved().commit.stage());

    // "Approve at the Gate": approve pre-commit hooks upfront (unless --no-verify)
    // Shadow no_verify: if user declines approval, skip hooks but continue commit
    let no_verify = if !no_verify {
        let approved = approve_hooks(&ctx, &[HookType::PreCommit])?;
        if !approved {
            eprintln!(
                "{}",
                info_message("Commands declined, committing without hooks",)
            );
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
    options.warn_about_untracked = stage_mode == StageMode::All;

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
/// * `no_verify` - If true, skip all pre-commit hooks (from --no-verify flag)
/// * `stage` - CLI-provided stage mode. If None, uses the effective config default.
pub fn handle_squash(
    target: Option<&str>,
    yes: bool,
    no_verify: bool,
    stage: Option<StageMode>,
) -> anyhow::Result<SquashResult> {
    // Load config once, run LLM setup prompt, then reuse config
    let mut config = UserConfig::load().context("Failed to load config")?;
    // One-time LLM setup prompt (errors logged internally; don't block commit)
    let _ = crate::output::prompt_commit_generation(&mut config);

    let env = CommandEnv::for_action("squash", config)?;
    let repo = &env.repo;
    // Squash requires being on a branch (can't squash in detached HEAD)
    let current_branch = env.require_branch("squash")?.to_string();
    let ctx = env.context(yes);
    let resolved = env.resolved();
    let generator = CommitGenerator::new(&resolved.commit_generation);

    // CLI flag overrides config value
    let stage_mode = stage.unwrap_or(resolved.commit.stage());

    // Check if any pre-commit hooks exist (needed for skip message and approval)
    let project_config = repo.load_project_config()?;
    let user_hooks = ctx.config.hooks(ctx.project_id().as_deref());
    let any_hooks_exist = user_hooks.pre_commit.is_some()
        || project_config
            .as_ref()
            .is_some_and(|c| c.hooks.pre_commit.is_some());

    // "Approve at the Gate": approve pre-commit hooks upfront (unless --no-verify)
    // Shadow no_verify: if user declines approval, skip hooks but continue squash
    let no_verify = if !no_verify {
        let approved = approve_hooks(&ctx, &[HookType::PreCommit])?;
        if !approved {
            eprintln!(
                "{}",
                info_message("Commands declined, squashing without hooks")
            );
            true // Skip hooks
        } else {
            false // Run hooks
        }
    } else {
        // Show skip message when --no-verify was passed and hooks exist
        if any_hooks_exist {
            eprintln!(
                "{}",
                info_message("Skipping pre-commit hooks (--no-verify)")
            );
        }
        true // --no-verify was passed
    };

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

    // Run pre-commit hooks (user first, then project)
    if !no_verify {
        let extra_vars = [("target", integration_target.as_str())];
        run_hook_with_filter(
            &ctx,
            user_hooks.pre_commit.as_ref(),
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
        generator.commit_staged_changes(&wt, true, true, stage_mode)?;
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
            StageMode::Tracked => " & tracked changes",
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
    eprintln!("{}", progress_message(squash_progress));

    // Create safety backup before potentially destructive reset if there are working tree changes
    if has_staged {
        let backup_message = format!("{} → {} (squash)", current_branch, integration_target);
        let sha = wt.create_safety_backup(&backup_message)?;
        eprintln!("{}", hint_message(format!("Backup created @ {sha}")));
    }

    // Get commit subjects for the squash message
    let subjects = repo.commit_subjects(&range)?;

    // Generate squash commit message
    eprintln!(
        "{}",
        progress_message("Generating squash commit message...")
    );

    generator.emit_hint_if_needed();

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
        &resolved.commit_generation,
    )?;

    // Display the generated commit message
    let formatted_message = generator.format_message_for_display(&commit_message);
    eprintln!("{}", format_with_gutter(&formatted_message, None));

    // Reset to merge base (soft reset stages all changes, including any already-staged uncommitted changes)
    //
    // TOCTOU note: Between this reset and the commit below, an external process could
    // modify the staging area. This is extremely unlikely (requires precise timing) and
    // the consequence is minor (unexpected content in squash commit). The commit message
    // generated above accurately reflects the original commits being squashed, so any
    // discrepancy would be visible in the diff. Considered acceptable risk.
    repo.run_command(&["reset", "--soft", &merge_base])
        .context("Failed to reset to merge base")?;

    // Check if there are actually any changes to commit
    if !wt.has_staged_changes()? {
        eprintln!(
            "{}",
            info_message(format!(
                "No changes after squashing {commit_count} {commit_text}"
            ))
        );
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
    eprintln!(
        "{}",
        success_message(cformat!("Squashed @ <dim>{commit_hash}</>"))
    );

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
    println!("{}", prompt);
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
        eprintln!(
            "{}",
            progress_message(cformat!("Rebasing onto <bold>{integration_target}</>..."))
        );
    }

    let rebase_result = repo.run_command(&["rebase", &integration_target]);

    // If rebase failed, check if it's due to conflicts
    if let Err(e) = rebase_result {
        // Check if it's a rebase conflict
        let is_rebasing = repo
            .worktree_state()?
            .is_some_and(|s| s.starts_with("REBASING"));
        if is_rebasing {
            // Extract git's stderr output from the error
            let git_output = e.to_string();
            return Err(worktrunk::git::GitError::RebaseConflict {
                target_branch: integration_target,
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
    if repo.worktree_state()?.is_some() {
        return Err(worktrunk::git::GitError::RebaseConflict {
            target_branch: integration_target,
            git_output: String::new(),
        }
        .into());
    }

    // Success
    let msg = if is_fast_forward {
        cformat!("Fast-forwarded to <bold>{integration_target}</>")
    } else {
        cformat!("Rebased onto <bold>{integration_target}</>")
    };
    eprintln!("{}", success_message(msg));

    Ok(RebaseResult::Rebased)
}

/// Handle `wt step diff` command
///
/// Shows all changes since branching from the target: committed, staged, unstaged,
/// and untracked files in a single diff. Uses a temporary index to include untracked
/// files without modifying the real git index.
///
/// TODO: consider adding `--stage` flag (all/tracked/none) like `step commit` to
/// control which change types are included. `tracked` would skip the temp index,
/// `none` would diff only committed changes.
pub fn step_diff(target: Option<&str>, extra_args: &[String]) -> anyhow::Result<()> {
    let repo = Repository::current()?;
    let wt = repo.current_worktree();

    // Get and validate target ref
    let integration_target = repo.require_target_ref(target)?;

    // Get merge base
    let merge_base = repo
        .merge_base("HEAD", &integration_target)?
        .context("No common ancestor with target branch")?;

    let current_branch = wt.branch()?.unwrap_or_else(|| "HEAD".to_string());

    // Create an empty temporary index and register all working tree files with
    // `git add -N .` so untracked files become visible to `git diff`.
    let worktree_root = wt.root()?;

    let temp_index = tempfile::NamedTempFile::new().context("Failed to create temporary index")?;
    let temp_index_path = temp_index
        .path()
        .to_str()
        .context("Temporary index path is not valid UTF-8")?;

    // Initialize a valid empty index
    Cmd::new("git")
        .args(["read-tree", "--empty"])
        .current_dir(&worktree_root)
        .context(&current_branch)
        .env("GIT_INDEX_FILE", temp_index_path)
        .run()
        .context("Failed to initialize temporary index")?;

    // Register all working tree files as intent-to-add
    Cmd::new("git")
        .args(["add", "--intent-to-add", "."])
        .current_dir(&worktree_root)
        .context(&current_branch)
        .env("GIT_INDEX_FILE", temp_index_path)
        .run()
        .context("Failed to register untracked files")?;

    // Stream diff to stdout — git handles pager and coloring
    let mut diff_args = vec!["diff".to_string(), merge_base];
    diff_args.extend_from_slice(extra_args);
    Cmd::new("git")
        .args(&diff_args)
        .current_dir(&worktree_root)
        .context(&current_branch)
        .env("GIT_INDEX_FILE", temp_index_path)
        .stream()?;

    Ok(())
}

/// List gitignored entries in a worktree, filtered by `.worktreeinclude` and excluding
/// entries that contain nested worktrees.
///
/// Combines three steps:
/// 1. `list_ignored_entries()` — git ls-files for ignored entries
/// 2. `.worktreeinclude` filtering — only matching entries if the file exists
/// 3. Nested worktree filtering — exclude entries containing other worktrees
fn list_and_filter_ignored_entries(
    worktree_path: &Path,
    context: &str,
    worktree_paths: &[PathBuf],
) -> anyhow::Result<Vec<(PathBuf, bool)>> {
    let ignored_entries = list_ignored_entries(worktree_path, context)?;

    // Filter to entries that match .worktreeinclude (or all if no file exists)
    let include_path = worktree_path.join(".worktreeinclude");
    let filtered: Vec<_> = if include_path.exists() {
        let include_matcher = {
            let mut builder = GitignoreBuilder::new(worktree_path);
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
        ignored_entries
    };

    // Filter out entries that contain other worktrees
    Ok(filtered
        .into_iter()
        .filter(|(entry_path, _)| {
            !worktree_paths
                .iter()
                .any(|wt_path| wt_path != worktree_path && wt_path.starts_with(entry_path))
        })
        .collect())
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
    force: bool,
) -> anyhow::Result<()> {
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
        None => repo.current_worktree().root()?,
    };

    if source_path == dest_path {
        eprintln!(
            "{}",
            info_message("Source and destination are the same worktree")
        );
        return Ok(());
    }

    let worktree_paths: Vec<PathBuf> = repo
        .list_worktrees()?
        .into_iter()
        .map(|wt| wt.path)
        .collect();
    let entries_to_copy =
        list_and_filter_ignored_entries(&source_path, &source_context, &worktree_paths)?;

    if entries_to_copy.is_empty() {
        eprintln!("{}", info_message("No matching files to copy"));
        return Ok(());
    }

    let verbose = verbosity();
    let mut copied_count = 0;

    // Show entries in verbose or dry-run mode
    if verbose >= 1 || dry_run {
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
        let verb = if dry_run { "Would copy" } else { "Copying" };
        eprintln!(
            "{}",
            info_message(format!(
                "{verb} {} {}:\n{}",
                items.len(),
                entry_word,
                format_with_gutter(&items.join("\n"), None)
            ))
        );
        if dry_run {
            return Ok(());
        }
    }

    // Copy entries
    for (src_entry, is_dir) in &entries_to_copy {
        // Paths from git ls-files are always under source_path
        let relative = src_entry
            .strip_prefix(&source_path)
            .expect("git ls-files path under worktree");
        let dest_entry = dest_path.join(relative);

        if *is_dir {
            copy_dir_recursive(src_entry, &dest_entry, force)
                .with_context(|| format!("copying directory {}", relative.display()))?;
            copied_count += 1;
        } else {
            if let Some(parent) = dest_entry.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating directory for {}", relative.display()))?;
            }
            if force {
                remove_if_exists(&dest_entry)?;
            }
            // Skip existing files for idempotent hook usage
            match reflink_copy::reflink_or_copy(src_entry, &dest_entry) {
                Ok(_) => copied_count += 1,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => {
                    return Err(
                        anyhow::Error::from(e).context(format!("copying {}", relative.display()))
                    );
                }
            }
        }
    }

    // Show summary
    let entry_word = if copied_count == 1 {
        "entry"
    } else {
        "entries"
    };
    eprintln!(
        "{}",
        success_message(format!("Copied {copied_count} {entry_word}"))
    );

    Ok(())
}

/// Remove a file, ignoring "not found" errors.
fn remove_if_exists(path: &Path) -> anyhow::Result<()> {
    if let Err(e) = fs::remove_file(path) {
        anyhow::ensure!(e.kind() == ErrorKind::NotFound, e);
    }
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
fn copy_dir_recursive(src: &Path, dest: &Path, force: bool) -> anyhow::Result<()> {
    copy_dir_recursive_fallback(src, dest, force)
}

/// File-by-file recursive copy with reflink per file.
///
/// Used as fallback when atomic directory clone isn't available or fails.
fn copy_dir_recursive_fallback(src: &Path, dest: &Path, force: bool) -> anyhow::Result<()> {
    fs::create_dir_all(dest).with_context(|| format!("creating directory {}", dest.display()))?;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dest_path = dest.join(entry.file_name());

        if file_type.is_symlink() {
            // Copy symlink (preserves the link, doesn't follow it)
            if force {
                remove_if_exists(&dest_path)?;
            }
            // Use symlink_metadata to detect broken symlinks (exists() follows symlinks
            // and returns false for broken ones, causing EEXIST on the next symlink call)
            if dest_path.symlink_metadata().is_err() {
                let target = fs::read_link(&src_path)
                    .with_context(|| format!("reading symlink {}", src_path.display()))?;
                create_symlink(&target, &src_path, &dest_path)?;
            }
        } else if file_type.is_dir() {
            copy_dir_recursive_fallback(&src_path, &dest_path, force)?;
        } else if !file_type.is_file() {
            // Skip non-regular files (sockets, FIFOs, etc.)
            log::debug!("skipping non-regular file: {}", src_path.display());
        } else {
            if force {
                remove_if_exists(&dest_path)?;
            }
            // Skip existing files for idempotent hook usage
            match reflink_copy::reflink_or_copy(&src_path, &dest_path) {
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
                Err(e) => {
                    return Err(
                        anyhow::Error::from(e).context(format!("copying {}", src_path.display()))
                    );
                }
            }
        }
    }

    Ok(())
}

/// Move a file or directory, falling back to copy+delete on cross-device errors.
fn move_entry(src: &Path, dest: &Path, is_dir: bool) -> anyhow::Result<()> {
    // Ensure parent directory exists
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating parent directory for {}", dest.display()))?;
    }

    match fs::rename(src, dest) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::CrossesDevices => copy_and_remove(src, dest, is_dir),
        Err(e) => Err(anyhow::Error::from(e).context(format!(
            "moving {} to {}",
            src.display(),
            dest.display()
        ))),
    }
}

/// Copy then delete — fallback when `rename` fails with EXDEV (cross-device).
fn copy_and_remove(src: &Path, dest: &Path, is_dir: bool) -> anyhow::Result<()> {
    if is_dir {
        copy_dir_recursive(src, dest, true)?;
        fs::remove_dir_all(src)
            .with_context(|| format!("removing source directory {}", src.display()))?;
    } else {
        reflink_copy::reflink_or_copy(src, dest)
            .with_context(|| format!("copying {} to {}", src.display(), dest.display()))?;
        fs::remove_file(src).with_context(|| format!("removing source file {}", src.display()))?;
    }
    Ok(())
}

const PROMOTE_STAGING_DIR: &str = "wt-promote-staging";

/// Move gitignored files from both worktrees into a staging directory.
///
/// Called BEFORE the branch exchange because `git switch` silently overwrites
/// ignored files that collide with tracked files on the target branch.
///
/// Returns the staging directory path and the count of entries staged.
fn stage_ignored(
    repo: &Repository,
    path_a: &Path,
    entries_a: &[(PathBuf, bool)],
    path_b: &Path,
    entries_b: &[(PathBuf, bool)],
) -> anyhow::Result<(PathBuf, usize)> {
    let staging_dir = repo.git_common_dir().join(PROMOTE_STAGING_DIR);
    fs::create_dir_all(&staging_dir).context("creating promote staging directory")?;

    let staging_a = staging_dir.join("a");
    let staging_b = staging_dir.join("b");
    let mut count = 0;

    // Move A's entries → staging/a
    for (src_entry, is_dir) in entries_a {
        let relative = src_entry
            .strip_prefix(path_a)
            .context("entry not under worktree A")?;
        let staging_entry = staging_a.join(relative);
        if fs::symlink_metadata(src_entry).is_ok() {
            move_entry(src_entry, &staging_entry, *is_dir)
                .with_context(|| format!("staging {}", relative.display()))?;
            count += 1;
        }
    }

    // Move B's entries → staging/b
    for (src_entry, is_dir) in entries_b {
        let relative = src_entry
            .strip_prefix(path_b)
            .context("entry not under worktree B")?;
        let staging_entry = staging_b.join(relative);
        if fs::symlink_metadata(src_entry).is_ok() {
            move_entry(src_entry, &staging_entry, *is_dir)
                .with_context(|| format!("staging {}", relative.display()))?;
            count += 1;
        }
    }

    // Clean up empty staging directory (can happen if all entries vanished
    // between listing and staging due to TOCTOU)
    if count == 0 && staging_dir.exists() {
        let _ = fs::remove_dir_all(&staging_dir);
    }

    Ok((staging_dir, count))
}

/// Distribute staged files to their new worktrees after a branch exchange.
///
/// B's original files (in staging/b) go to worktree A (which now has B's branch).
/// A's original files (in staging/a) go to worktree B (which now has A's branch).
fn distribute_staged(
    staging_dir: &Path,
    path_a: &Path,
    entries_a: &[(PathBuf, bool)],
    path_b: &Path,
    entries_b: &[(PathBuf, bool)],
) -> anyhow::Result<usize> {
    let staging_a = staging_dir.join("a");
    let staging_b = staging_dir.join("b");
    let mut count = 0;

    // Move B's staged entries → A (A now has B's branch)
    for (src_entry, is_dir) in entries_b {
        let relative = src_entry
            .strip_prefix(path_b)
            .context("entry not under worktree B")?;
        let staging_entry = staging_b.join(relative);
        let dest_entry = path_a.join(relative);
        if fs::symlink_metadata(&staging_entry).is_ok() {
            move_entry(&staging_entry, &dest_entry, *is_dir)
                .context(format!("distributing {}", relative.display()))?;
            count += 1;
        }
    }

    // Move A's staged entries → B (B now has A's branch)
    for (src_entry, is_dir) in entries_a {
        let relative = src_entry
            .strip_prefix(path_a)
            .context("entry not under worktree A")?;
        let staging_entry = staging_a.join(relative);
        let dest_entry = path_b.join(relative);
        if fs::symlink_metadata(&staging_entry).is_ok() {
            move_entry(&staging_entry, &dest_entry, *is_dir)
                .context(format!("distributing {}", relative.display()))?;
            count += 1;
        }
    }

    // Clean up staging directory (best-effort — files are already distributed)
    let _ = fs::remove_dir_all(staging_dir);

    Ok(count)
}

/// Restore staged files to their original worktrees (undo staging on failure).
///
/// Unlike `distribute_staged` which crosses entries (A→B, B→A), this puts each
/// side's entries back where they came from (staging-a→A, staging-b→B).
fn restore_staged(
    staging_dir: &Path,
    path_a: &Path,
    entries_a: &[(PathBuf, bool)],
    path_b: &Path,
    entries_b: &[(PathBuf, bool)],
) -> anyhow::Result<()> {
    let staging_a = staging_dir.join("a");
    let staging_b = staging_dir.join("b");

    // Move staging-a entries back to A
    for (src_entry, is_dir) in entries_a {
        let relative = src_entry
            .strip_prefix(path_a)
            .context("entry not under worktree A")?;
        let staging_entry = staging_a.join(relative);
        if fs::symlink_metadata(&staging_entry).is_ok() {
            move_entry(&staging_entry, src_entry, *is_dir)
                .context(format!("restoring {}", relative.display()))?;
        }
    }

    // Move staging-b entries back to B
    for (src_entry, is_dir) in entries_b {
        let relative = src_entry
            .strip_prefix(path_b)
            .context("entry not under worktree B")?;
        let staging_entry = staging_b.join(relative);
        if fs::symlink_metadata(&staging_entry).is_ok() {
            move_entry(&staging_entry, src_entry, *is_dir)
                .context(format!("restoring {}", relative.display()))?;
        }
    }

    // Clean up staging directory
    if staging_dir.exists() {
        fs::remove_dir_all(staging_dir).context("cleaning up promote staging directory")?;
    }

    Ok(())
}

/// Best-effort restore with user-visible warning on failure.
fn restore_staged_or_warn(
    staging_dir: &Path,
    path_a: &Path,
    entries_a: &[(PathBuf, bool)],
    path_b: &Path,
    entries_b: &[(PathBuf, bool)],
) {
    if let Err(restore_err) = restore_staged(staging_dir, path_a, entries_a, path_b, entries_b) {
        eprintln!(
            "{}",
            warning_message(format!(
                "Failed to restore staged files: {restore_err:#}. \
                 Files may need manual recovery from: {}",
                staging_dir.display()
            ))
        );
    }
}

/// Result of a promote operation
pub enum PromoteResult {
    /// Branch was promoted successfully
    Promoted,
    /// Already in canonical state (requested branch is already in main)
    AlreadyInMain(String),
}

/// Exchange branches between two worktrees with rollback on failure.
///
/// Steps: detach target → detach main → switch main → switch target.
/// On failure at any step, attempts to restore both worktrees to their original branches.
fn exchange_branches(
    main_wt: &worktrunk::git::WorkingTree<'_>,
    main_branch: &str,
    target_wt: &worktrunk::git::WorkingTree<'_>,
    target_branch: &str,
) -> anyhow::Result<()> {
    // Step 1: Detach target (if this fails, nothing has changed)
    target_wt
        .run_command(&["checkout", "--detach"])
        .context("Failed to detach HEAD in target worktree")?;

    // Step 2: Detach main (if this fails, re-attach target)
    if let Err(e) = main_wt
        .run_command(&["checkout", "--detach"])
        .context("Failed to detach HEAD in main worktree")
    {
        if let Err(rb) = target_wt.run_command(&["switch", target_branch]) {
            log::warn!("Rollback failed re-attaching target: {rb}");
        }
        return Err(e);
    }

    // Step 3: Switch main to target_branch (if this fails, re-attach both)
    if let Err(e) = main_wt
        .run_command(&["switch", target_branch])
        .context("Failed to switch to branch in main worktree")
    {
        if let Err(rb) = main_wt.run_command(&["switch", main_branch]) {
            log::warn!("Rollback failed re-attaching main: {rb}");
        }
        if let Err(rb) = target_wt.run_command(&["switch", target_branch]) {
            log::warn!("Rollback failed re-attaching target: {rb}");
        }
        return Err(e);
    }

    // Step 4: Switch target to main_branch (if this fails, reverse step 3)
    if let Err(e) = target_wt
        .run_command(&["switch", main_branch])
        .context("Failed to switch to branch in target worktree")
    {
        if let Err(rb) = main_wt.run_command(&["switch", main_branch]) {
            log::warn!("Rollback failed re-attaching main: {rb}");
        }
        if let Err(rb) = target_wt.run_command(&["switch", target_branch]) {
            log::warn!("Rollback failed re-attaching target: {rb}");
        }
        return Err(e);
    }

    Ok(())
}

/// Handle `wt step promote` command
///
/// Promotes a branch to the main worktree, exchanging it with whatever branch is currently there.
///
/// ## Interruption recovery
///
/// The swap uses a staging directory at `.git/wt-promote-staging/` and proceeds
/// in three phases:
///
/// 1. **Stage**: move ignored files from both worktrees into staging (`a/`, `b/`)
/// 2. **Exchange**: detach + `git switch` to swap branches
/// 3. **Distribute**: move staged files to their new worktrees, then delete staging
///
/// A hard kill at any phase leaves files in staging, never deleted. The next run
/// detects the leftover directory and bails with a recovery path. Branch exchange
/// has per-step rollback for git errors; a kill during `git switch` may leave a
/// worktree detached (fix: `git switch <branch>`).
pub fn handle_promote(branch: Option<&str>) -> anyhow::Result<PromoteResult> {
    use worktrunk::git::GitError;

    let repo = Repository::current()?;
    let worktrees = repo.list_worktrees()?;

    if worktrees.is_empty() {
        anyhow::bail!("No worktrees found");
    }

    // For normal repos, worktrees[0] is the main worktree
    // For bare repos, there's no main worktree - we don't support promote there
    if repo.is_bare() {
        anyhow::bail!("wt step promote is not supported in bare repositories");
    }

    let main_wt = &worktrees[0];
    let main_path = &main_wt.path;
    let main_branch = main_wt
        .branch
        .clone()
        .ok_or_else(|| GitError::DetachedHead {
            action: Some("promote".into()),
        })?;

    // Resolve the branch to promote (default_branch computed lazily, only when needed)
    let target_branch = match branch {
        Some(b) => b.to_string(),
        None => {
            let current_wt = repo.current_worktree();
            if !current_wt.is_linked()? {
                // From main worktree with no args: restore default branch
                repo.default_branch()
                    .ok_or_else(|| anyhow::anyhow!("Could not determine default branch"))?
            } else {
                // From other worktree with no args: promote current branch
                current_wt.branch()?.ok_or_else(|| GitError::DetachedHead {
                    action: Some("promote".into()),
                })?
            }
        }
    };

    // Check if target is already in main worktree
    if target_branch == main_branch {
        return Ok(PromoteResult::AlreadyInMain(target_branch));
    }

    // Find the worktree with the target branch
    let target_wt = worktrees
        .iter()
        .skip(1) // Skip main worktree
        .find(|wt| wt.branch.as_deref() == Some(&target_branch))
        .ok_or_else(|| GitError::WorktreeNotFound {
            branch: target_branch.clone(),
        })?;

    let target_path = &target_wt.path;

    // Ensure both worktrees are clean
    let main_working_tree = repo.worktree_at(main_path);
    let target_working_tree = repo.worktree_at(target_path);

    main_working_tree.ensure_clean("promote", Some(&main_branch), false)?;
    target_working_tree.ensure_clean("promote", Some(&target_branch), false)?;

    // Bail early if a leftover staging dir exists from a previous interrupted promote —
    // it may contain the user's only copy of files from the failed swap.
    let staging_path = repo.git_common_dir().join(PROMOTE_STAGING_DIR);
    if staging_path.exists() {
        return Err(anyhow::anyhow!(
            "Files may need manual recovery from: {}\n\
             Remove it to retry: rm -rf \"{}\"",
            staging_path.display(),
            staging_path.display()
        )
        .context("Found leftover staging directory from an interrupted promote"));
    }

    // Check if we're restoring canonical state (promoting default branch back to main worktree)
    // Only lookup default_branch if needed for messaging (already resolved if no-arg from main)
    let default_branch = repo.default_branch();
    let is_restoring = default_branch.as_ref() == Some(&target_branch);

    if is_restoring {
        // Restoring default branch to main worktree - no warning needed
        eprintln!("{}", info_message("Restoring main worktree"));
    } else {
        // Creating mismatch - show warning and how to restore
        eprintln!(
            "{}",
            warning_message("Promoting creates mismatched worktree state (shown as ⚑ in wt list)",)
        );
        // Only show restore hint if we know the default branch
        if let Some(default) = &default_branch {
            eprintln!(
                "{}",
                hint_message(cformat!(
                    "Run <bright-black>wt step promote {default}</> to restore canonical locations"
                ))
            );
        }
    }

    // Discover gitignored entries BEFORE branch exchange — .gitignore rules belong
    // to the current branch and will change after `git switch`.
    let worktree_paths: Vec<PathBuf> = worktrees.iter().map(|wt| wt.path.clone()).collect();
    let main_entries = list_and_filter_ignored_entries(main_path, &main_branch, &worktree_paths)?;
    let target_entries =
        list_and_filter_ignored_entries(target_path, &target_branch, &worktree_paths)?;

    // Move gitignored files to staging BEFORE branch exchange.
    // `git switch` silently overwrites ignored files that collide with tracked
    // files on the target branch — staging them first prevents data loss.
    let staged = if !main_entries.is_empty() || !target_entries.is_empty() {
        let (dir, count) = stage_ignored(
            &repo,
            main_path,
            &main_entries,
            target_path,
            &target_entries,
        )
        .context(format!(
            "Failed to stage ignored files. Already-staged files may be recoverable from: {}",
            staging_path.display()
        ))?;
        if count > 0 { Some((dir, count)) } else { None }
    } else {
        None
    };

    // Perform the branch exchange with rollback on failure:
    // 1. Detach both worktrees (releases branch locks)
    // 2. Switch to exchanged branches
    //
    // Detach target first so if it fails, main worktree is unchanged.
    if let Err(e) = exchange_branches(
        &main_working_tree,
        &main_branch,
        &target_working_tree,
        &target_branch,
    ) {
        // Branch exchange failed — restore staged files to their original worktrees.
        if let Some((ref staging_dir, _)) = staged {
            restore_staged_or_warn(
                staging_dir,
                main_path,
                &main_entries,
                target_path,
                &target_entries,
            );
        }
        return Err(e);
    }

    // Distribute staged files to their new worktrees (after branch exchange)
    let swapped = if let Some((ref staging_dir, _)) = staged {
        distribute_staged(
            staging_dir,
            main_path,
            &main_entries,
            target_path,
            &target_entries,
        )
        .context(format!(
            "Failed to distribute staged files. Staged files may be recoverable from: {}",
            staging_dir.display()
        ))?
    } else {
        0
    };

    // Print success messages only after everything succeeded
    eprintln!(
        "{}",
        success_message(cformat!(
            "Promoted: main worktree now has <bold>{target_branch}</>; {} now has <bold>{main_branch}</>",
            worktrunk::path::format_path_for_display(target_path)
        ))
    );
    if swapped > 0 {
        let path_word = if swapped == 1 { "path" } else { "paths" };
        eprintln!(
            "{}",
            success_message(format!("Swapped {swapped} gitignored {path_word}"))
        );
    }

    Ok(PromoteResult::Promoted)
}

/// Create a symlink, handling platform differences.
///
/// On Windows, distinguishes between file and directory symlinks by checking the
/// source path's metadata (the target may be relative or broken, so we use the
/// source to determine the type).
fn create_symlink(target: &Path, src_path: &Path, dest_path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        let _ = src_path; // Used on Windows to determine symlink type
        std::os::unix::fs::symlink(target, dest_path)
            .with_context(|| format!("creating symlink {}", dest_path.display()))?;
    }
    #[cfg(windows)]
    {
        let is_dir = src_path.metadata().map(|m| m.is_dir()).unwrap_or(false);
        if is_dir {
            std::os::windows::fs::symlink_dir(target, dest_path)
                .with_context(|| format!("creating symlink {}", dest_path.display()))?;
        } else {
            std::os::windows::fs::symlink_file(target, dest_path)
                .with_context(|| format!("creating symlink {}", dest_path.display()))?;
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, src_path, dest_path);
        anyhow::bail!("symlink creation not supported on this platform");
    }
    Ok(())
}

/// Move worktrees to their expected paths based on the `worktree-path` template.
///
/// See `src/commands/relocate.rs` for the implementation details and algorithm.
///
/// # Flags
///
/// | Flag | Purpose |
/// |------|---------|
/// | `--dry-run` | Show what would be moved without moving |
/// | `--commit` | Auto-commit dirty worktrees with LLM-generated messages before relocating |
/// | `--clobber` | Move non-worktree paths out of the way (`<path>.bak-<timestamp>`) |
/// | `[branches...]` | Specific branches to relocate (default: all mismatched) |
pub fn step_relocate(
    branches: Vec<String>,
    dry_run: bool,
    commit: bool,
    clobber: bool,
) -> anyhow::Result<()> {
    use super::relocate::{
        GatherResult, RelocationExecutor, ValidationResult, gather_candidates, show_all_skipped,
        show_dry_run_preview, show_no_relocations_needed, show_summary, validate_candidates,
    };

    let repo = Repository::current()?;
    let config = UserConfig::load()?;
    let default_branch = repo.default_branch().unwrap_or_default();

    // Validate default branch early - needed for main worktree relocation
    if default_branch.is_empty() {
        anyhow::bail!(
            "Cannot determine default branch; set with: wt config state default-branch set main"
        );
    }
    let repo_path = repo.repo_path().to_path_buf();

    // Phase 1: Gather candidates (worktrees not at expected paths)
    let GatherResult {
        candidates,
        template_errors,
    } = gather_candidates(&repo, &config, &branches)?;

    if candidates.is_empty() {
        show_no_relocations_needed(template_errors);
        return Ok(());
    }

    // Dry run: show preview and exit
    if dry_run {
        show_dry_run_preview(&candidates);
        return Ok(());
    }

    // Phase 2: Validate candidates (check locked/dirty, optionally auto-commit)
    let ValidationResult { validated, skipped } =
        validate_candidates(&repo, &config, candidates, commit, &repo_path)?;

    if validated.is_empty() {
        show_all_skipped(skipped);
        return Ok(());
    }

    // Phase 3 & 4: Create executor (classifies targets) and execute relocations
    let mut executor = RelocationExecutor::new(&repo, validated, clobber)?;
    let cwd = std::env::current_dir().ok();
    executor.execute(&repo_path, &default_branch, cwd.as_deref())?;

    // Show summary
    let total_skipped = skipped + executor.skipped;
    show_summary(executor.relocated, total_skipped);

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

    #[test]
    fn test_remove_if_exists_nonexistent() {
        // NotFound is silently ignored
        assert!(remove_if_exists(Path::new("/nonexistent/file")).is_ok());
    }

    #[test]
    fn test_remove_if_exists_not_a_file() {
        // Trying to remove a directory with remove_file produces a non-NotFound error
        let dir = std::env::temp_dir();
        assert!(remove_if_exists(&dir).is_err());
    }

    #[test]
    fn test_restore_staged_moves_files_back() {
        let tmp = tempfile::tempdir().unwrap();
        let staging = tmp.path().join("staging");
        let path_a = tmp.path().join("worktree_a");
        let path_b = tmp.path().join("worktree_b");

        // Create staging directory with files from both worktrees
        fs::create_dir_all(staging.join("a/build")).unwrap();
        fs::write(staging.join("a/build/artifact"), "from A").unwrap();
        fs::write(staging.join("a/app.log"), "A log").unwrap();
        fs::create_dir_all(staging.join("b")).unwrap();
        fs::write(staging.join("b/debug.log"), "B log").unwrap();

        // Create destination directories
        fs::create_dir_all(&path_a).unwrap();
        fs::create_dir_all(&path_b).unwrap();

        let entries_a = vec![
            (path_a.join("build"), true),
            (path_a.join("app.log"), false),
        ];
        let entries_b = vec![(path_b.join("debug.log"), false)];

        restore_staged(&staging, &path_a, &entries_a, &path_b, &entries_b).unwrap();

        // A's files restored to A
        assert_eq!(
            fs::read_to_string(path_a.join("build/artifact")).unwrap(),
            "from A"
        );
        assert_eq!(fs::read_to_string(path_a.join("app.log")).unwrap(), "A log");

        // B's files restored to B
        assert_eq!(
            fs::read_to_string(path_b.join("debug.log")).unwrap(),
            "B log"
        );

        // Staging directory cleaned up
        assert!(!staging.exists());
    }

    #[test]
    fn test_restore_staged_skips_missing_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let staging = tmp.path().join("staging");
        let path_a = tmp.path().join("worktree_a");
        let path_b = tmp.path().join("worktree_b");

        // Create staging with only some entries
        fs::create_dir_all(staging.join("a")).unwrap();
        fs::write(staging.join("a/exists.log"), "data").unwrap();
        fs::create_dir_all(staging.join("b")).unwrap();

        fs::create_dir_all(&path_a).unwrap();
        fs::create_dir_all(&path_b).unwrap();

        let entries_a = vec![
            (path_a.join("exists.log"), false),
            (path_a.join("missing.log"), false), // not in staging
        ];
        let entries_b: Vec<(PathBuf, bool)> = vec![];

        // Should succeed, skipping missing entries
        restore_staged(&staging, &path_a, &entries_a, &path_b, &entries_b).unwrap();

        assert_eq!(
            fs::read_to_string(path_a.join("exists.log")).unwrap(),
            "data"
        );
        assert!(!path_a.join("missing.log").exists());
    }

    #[test]
    fn test_restore_staged_or_warn_succeeds_silently() {
        let tmp = tempfile::tempdir().unwrap();
        let staging = tmp.path().join("staging");
        let path_a = tmp.path().join("a");
        let path_b = tmp.path().join("b");

        fs::create_dir_all(staging.join("a")).unwrap();
        fs::write(staging.join("a/file.txt"), "data").unwrap();
        fs::create_dir_all(staging.join("b")).unwrap();
        fs::create_dir_all(&path_a).unwrap();
        fs::create_dir_all(&path_b).unwrap();

        let entries_a = vec![(path_a.join("file.txt"), false)];
        let entries_b: Vec<(PathBuf, bool)> = vec![];

        // Should not panic
        restore_staged_or_warn(&staging, &path_a, &entries_a, &path_b, &entries_b);

        assert_eq!(fs::read_to_string(path_a.join("file.txt")).unwrap(), "data");
    }

    #[test]
    fn test_move_entry_file() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("source.txt");
        let dest = tmp.path().join("subdir/dest.txt");

        fs::write(&src, "content").unwrap();
        move_entry(&src, &dest, false).unwrap();

        assert!(!src.exists());
        assert_eq!(fs::read_to_string(&dest).unwrap(), "content");
    }

    #[test]
    fn test_move_entry_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("srcdir");
        let dest = tmp.path().join("nested/destdir");

        fs::create_dir_all(src.join("inner")).unwrap();
        fs::write(src.join("inner/file.txt"), "nested").unwrap();
        fs::write(src.join("root.txt"), "root").unwrap();

        move_entry(&src, &dest, true).unwrap();

        assert!(!src.exists());
        assert_eq!(
            fs::read_to_string(dest.join("inner/file.txt")).unwrap(),
            "nested"
        );
        assert_eq!(fs::read_to_string(dest.join("root.txt")).unwrap(), "root");
    }

    #[test]
    fn test_copy_and_remove_file() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("source.txt");
        let dest = tmp.path().join("dest.txt");

        fs::write(&src, "content").unwrap();
        copy_and_remove(&src, &dest, false).unwrap();

        assert!(!src.exists());
        assert_eq!(fs::read_to_string(&dest).unwrap(), "content");
    }

    #[test]
    fn test_copy_and_remove_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("srcdir");
        let dest = tmp.path().join("destdir");

        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("sub/file.txt"), "nested").unwrap();
        fs::write(src.join("root.txt"), "root").unwrap();

        copy_and_remove(&src, &dest, true).unwrap();

        assert!(!src.exists());
        assert_eq!(
            fs::read_to_string(dest.join("sub/file.txt")).unwrap(),
            "nested"
        );
        assert_eq!(fs::read_to_string(dest.join("root.txt")).unwrap(), "root");
    }

    #[test]
    fn test_restore_staged_or_warn_handles_bad_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let staging = tmp.path().join("staging");
        // Entries reference paths outside the "worktree" prefix, causing strip_prefix to fail
        let path_a = tmp.path().join("a");
        let path_b = tmp.path().join("b");
        fs::create_dir_all(staging.join("a")).unwrap();
        fs::create_dir_all(staging.join("b")).unwrap();
        fs::create_dir_all(&path_a).unwrap();
        fs::create_dir_all(&path_b).unwrap();

        // Entries with wrong prefix — strip_prefix will fail, causing restore_staged to error
        let entries_a = vec![(PathBuf::from("/wrong/prefix/file.txt"), false)];
        let entries_b: Vec<(PathBuf, bool)> = vec![];

        // Should not panic, just warn
        restore_staged_or_warn(&staging, &path_a, &entries_a, &path_b, &entries_b);

        // Staging directory may still exist since restore failed
    }
}
