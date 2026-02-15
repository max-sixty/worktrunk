//! Step commands for the merge workflow.
//!
//! This module contains the individual steps that make up `wt merge`:
//! - `step_commit` - Commit working tree changes
//! - `handle_squash` - Squash commits into one
//! - `step_show_squash_prompt` - Show squash prompt without executing
//! - `handle_rebase` - Rebase onto target branch
//! - `step_copy_ignored` - Copy gitignored files matching .worktreeinclude

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::Context;
use color_print::cformat;
use ignore::gitignore::GitignoreBuilder;
use worktrunk::HookType;
use worktrunk::config::{CommitGenerationConfig, UserConfig};
use worktrunk::git::Repository;
use worktrunk::styling::{
    eprintln, format_with_gutter, hint_message, info_message, progress_message, success_message,
};
use worktrunk::workspace::{SquashOutcome, Workspace};

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
    // --show-prompt is VCS-agnostic: build prompt from trait method and print
    if show_prompt {
        let ws = worktrunk::workspace::open_workspace()?;
        let cwd = std::env::current_dir()?;
        let config = UserConfig::load().context("Failed to load config")?;
        let project_id = ws.project_identifier().ok();
        let commit_config = config.commit_generation(project_id.as_deref());

        let (diff, diff_stat) = ws.committable_diff_for_prompt(&cwd)?;
        let branch = ws.current_name(&cwd)?.unwrap_or_else(|| "HEAD".to_string());
        let repo_name = ws
            .root_path()?
            .file_name()
            .and_then(|n| n.to_str().map(String::from))
            .unwrap_or_else(|| "repo".to_string());
        let recent_commits = ws.recent_subjects(None, 5);

        let input = crate::llm::CommitInput {
            diff: &diff,
            diff_stat: &diff_stat,
            branch: &branch,
            repo_name: &repo_name,
            recent_commits: recent_commits.as_ref(),
        };
        let prompt = crate::llm::build_commit_prompt(&input, &commit_config)?;
        println!("{}", prompt);
        return Ok(());
    }

    // Open workspace once, route by VCS type via downcast
    let workspace = worktrunk::workspace::open_workspace()?;
    if workspace.as_any().downcast_ref::<Repository>().is_none() {
        return super::handle_step_jj::step_commit_jj();
    }

    // Load config once, run LLM setup prompt, then reuse config
    let mut config = UserConfig::load().context("Failed to load config")?;
    // One-time LLM setup prompt (errors logged internally; don't block commit)
    let _ = crate::output::prompt_commit_generation(&mut config);

    let env = CommandEnv::with_workspace(workspace, "commit", config)?;
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

/// VCS-agnostic squash core: count, generate message, execute squash.
///
/// Used by both git and jj paths. Does NOT handle staging, hooks, safety backups,
/// or progress display — those are git-specific concerns in `handle_squash`.
pub(crate) fn do_squash(
    workspace: &dyn Workspace,
    target: &str,
    path: &Path,
    commit_gen_config: &CommitGenerationConfig,
    branch_name: &str,
    repo_name: &str,
) -> anyhow::Result<SquashResult> {
    let feature_head = workspace.feature_head(path)?;

    // Check if already integrated
    if workspace.is_integrated(&feature_head, target)?.is_some() {
        return Ok(SquashResult::NoCommitsAhead(target.to_string()));
    }

    let (ahead, _) = workspace.ahead_behind(target, &feature_head)?;

    if ahead == 0 {
        return Ok(SquashResult::NoCommitsAhead(target.to_string()));
    }

    if ahead == 1 && !workspace.is_dirty(path)? {
        return Ok(SquashResult::AlreadySingleCommit);
    }

    // Gather data for message generation
    let subjects = workspace.commit_subjects(target, &feature_head)?;
    let (diff, diff_stat) = workspace.diff_for_prompt(target, &feature_head, path)?;
    let recent_commits = workspace.recent_subjects(Some(target), 5);

    // Generate squash commit message
    eprintln!(
        "{}",
        progress_message("Generating squash commit message...")
    );

    let generator = CommitGenerator::new(commit_gen_config);
    generator.emit_hint_if_needed();

    let input = crate::llm::SquashInput {
        target_branch: target,
        diff: &diff,
        diff_stat: &diff_stat,
        subjects: &subjects,
        current_branch: branch_name,
        repo_name,
        recent_commits: recent_commits.as_ref(),
    };
    let commit_message = crate::llm::generate_squash_message(&input, commit_gen_config)?;

    // Display the generated commit message
    let formatted_message = generator.format_message_for_display(&commit_message);
    eprintln!("{}", format_with_gutter(&formatted_message, None));

    // Execute the squash
    match workspace.squash_commits(target, &commit_message, path)? {
        SquashOutcome::Squashed(id) => {
            eprintln!("{}", success_message(cformat!("Squashed @ <dim>{id}</>")));
            Ok(SquashResult::Squashed)
        }
        SquashOutcome::NoNetChanges => {
            let commit_text = if ahead == 1 { "commit" } else { "commits" };
            eprintln!(
                "{}",
                info_message(format!("No changes after squashing {ahead} {commit_text}"))
            );
            Ok(SquashResult::NoNetChanges)
        }
    }
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
    // Open workspace once, route by VCS type via downcast
    let workspace = worktrunk::workspace::open_workspace()?;
    if workspace.as_any().downcast_ref::<Repository>().is_none() {
        // jj path: use do_squash() directly (no staging/hooks)
        let cwd = std::env::current_dir()?;
        let target = workspace.resolve_integration_target(target)?;

        let config = UserConfig::load().context("Failed to load config")?;
        let project_id = workspace.project_identifier().ok();
        let resolved = config.resolved(project_id.as_deref());

        let ws_name = workspace
            .current_name(&cwd)?
            .unwrap_or_else(|| "default".to_string());
        let repo_name = project_id.as_deref().unwrap_or("repo");

        return do_squash(
            &*workspace,
            &target,
            &cwd,
            &resolved.commit_generation,
            &ws_name,
            repo_name,
        );
    }
    // Workspace is git — proceed with CommandEnv which takes ownership
    let cwd = std::env::current_dir()?;
    let mut config = UserConfig::load().context("Failed to load config")?;
    // One-time LLM setup prompt (errors logged internally; don't block commit)
    let _ = crate::output::prompt_commit_generation(&mut config);

    let env = CommandEnv::with_workspace(workspace, "squash", config)?;
    let repo = env.require_repo()?;
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

    // Get diff data for LLM prompt
    let (diff, diff_stat) = repo.diff_for_prompt(&merge_base, "HEAD", &cwd)?;
    let recent_commits = repo.recent_subjects(Some(&merge_base), 5);

    let input = crate::llm::SquashInput {
        target_branch: &integration_target,
        diff: &diff,
        diff_stat: &diff_stat,
        subjects: &subjects,
        current_branch: &current_branch,
        repo_name,
        recent_commits: recent_commits.as_ref(),
    };
    let commit_message = crate::llm::generate_squash_message(&input, &resolved.commit_generation)?;

    // Display the generated commit message
    let formatted_message = generator.format_message_for_display(&commit_message);
    eprintln!("{}", format_with_gutter(&formatted_message, None));

    // Execute squash via trait (reset --soft, check staged, commit)
    //
    // TOCTOU note: Between the reset and commit inside squash_commits, an external
    // process could modify the staging area. This is extremely unlikely and the
    // consequence is minor (unexpected content in squash commit). Considered acceptable.
    match repo.squash_commits(&integration_target, &commit_message, &cwd)? {
        SquashOutcome::Squashed(commit_hash) => {
            eprintln!(
                "{}",
                success_message(cformat!("Squashed @ <dim>{commit_hash}</>"))
            );
            Ok(SquashResult::Squashed)
        }
        SquashOutcome::NoNetChanges => {
            eprintln!(
                "{}",
                info_message(format!(
                    "No changes after squashing {commit_count} {commit_text}"
                ))
            );
            Ok(SquashResult::NoNetChanges)
        }
    }
}

/// Handle `wt step push` command.
///
/// Fully trait-based: opens the workspace and uses `advance_and_push`
/// for both git and jj, with zero VcsKind branching.
///
/// Each VCS implementation validates push safety internally:
/// - Git checks fast-forward (is_ancestor) — allows merge commits
/// - Jj checks that target is ancestor of feature tip (is_rebased_onto)
pub fn step_push(target: Option<&str>) -> anyhow::Result<()> {
    let ws = worktrunk::workspace::open_workspace()?;
    let cwd = std::env::current_dir()?;

    let target = ws.resolve_integration_target(target)?;

    let result = ws
        .advance_and_push(&target, &cwd)
        .context("Failed to push")?;

    if result.commit_count > 0 {
        let mut summary_parts = vec![format!(
            "{} commit{}",
            result.commit_count,
            if result.commit_count == 1 { "" } else { "s" }
        )];
        summary_parts.extend(result.stats_summary);

        let stats_str = summary_parts.join(", ");
        let paren_close = cformat!("<bright-black>)</>");
        eprintln!(
            "{}",
            success_message(cformat!(
                "Pushed to <bold>{target}</> <bright-black>({stats_str}</>{}",
                paren_close
            ))
        );
    } else {
        eprintln!(
            "{}",
            info_message(cformat!("Already up to date with <bold>{target}</>"))
        );
    }

    Ok(())
}

/// Handle `wt step squash --show-prompt`
///
/// Builds and outputs the squash prompt without running the LLM or squashing.
/// Works for both git and jj repositories.
pub fn step_show_squash_prompt(target: Option<&str>) -> anyhow::Result<()> {
    let ws = worktrunk::workspace::open_workspace()?;
    let cwd = std::env::current_dir()?;

    let config = UserConfig::load().context("Failed to load config")?;
    let project_id = ws.project_identifier().ok();
    let effective_config = config.commit_generation(project_id.as_deref());

    let target = ws.resolve_integration_target(target)?;
    let feature_head = ws.feature_head(&cwd)?;

    let current_branch = ws.current_name(&cwd)?.unwrap_or_else(|| "HEAD".to_string());

    let subjects = ws.commit_subjects(&target, &feature_head)?;
    let (diff, diff_stat) = ws.diff_for_prompt(&target, &feature_head, &cwd)?;
    let recent_commits = ws.recent_subjects(Some(&target), 5);

    let repo_name = ws
        .root_path()?
        .file_name()
        .and_then(|n| n.to_str().map(String::from))
        .unwrap_or_else(|| "repo".to_string());

    let input = crate::llm::SquashInput {
        target_branch: &target,
        diff: &diff,
        diff_stat: &diff_stat,
        subjects: &subjects,
        current_branch: &current_branch,
        repo_name: &repo_name,
        recent_commits: recent_commits.as_ref(),
    };
    let prompt = crate::llm::build_squash_prompt(&input, &effective_config)?;
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
    let ws = worktrunk::workspace::open_workspace()?;
    let cwd = std::env::current_dir()?;

    let target = ws.resolve_integration_target(target)?;

    if ws.is_rebased_onto(&target, &cwd)? {
        return Ok(RebaseResult::UpToDate(target));
    }

    let outcome = ws.rebase_onto(&target, &cwd)?;

    let msg = match outcome {
        worktrunk::workspace::RebaseOutcome::FastForward => {
            cformat!("Fast-forwarded to <bold>{target}</>")
        }
        worktrunk::workspace::RebaseOutcome::Rebased => {
            cformat!("Rebased onto <bold>{target}</>")
        }
    };
    eprintln!("{}", success_message(msg));

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
    force: bool,
) -> anyhow::Result<()> {
    let workspace = worktrunk::workspace::open_workspace()?;
    let repo = super::require_git_workspace(&*workspace, "step copy-ignored")?;

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
        eprintln!("{}", info_message("No matching files to copy"));
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
        eprintln!(
            "{}",
            info_message(format!(
                "Would copy {} {}:\n{}",
                items.len(),
                entry_word,
                format_with_gutter(&items.join("\n"), None)
            ))
        );
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
            copy_dir_recursive(src_entry, &dest_entry, force)?;
            copied_count += 1;
        } else {
            if let Some(parent) = dest_entry.parent() {
                fs::create_dir_all(parent)?;
            }
            if force {
                remove_if_exists(&dest_entry)?;
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
    fs::create_dir_all(dest)?;

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
            copy_dir_recursive_fallback(&src_path, &dest_path, force)?;
        } else {
            if force {
                remove_if_exists(&dest_path)?;
            }
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

    let workspace = worktrunk::workspace::open_workspace()?;
    let repo = super::require_git_workspace(&*workspace, "step relocate")?;
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
    } = gather_candidates(repo, &config, &branches)?;

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
        validate_candidates(repo, &config, candidates, commit, &repo_path)?;

    if validated.is_empty() {
        show_all_skipped(skipped);
        return Ok(());
    }

    // Phase 3 & 4: Create executor (classifies targets) and execute relocations
    let mut executor = RelocationExecutor::new(repo, validated, clobber)?;
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
}
