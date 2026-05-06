//! Helpers shared between multiple step subcommands.
//!
//! - `print_dry_run` — used by `commit` and `squash` for `--dry-run` output.
//! - Copy-ignored discovery (`list_and_filter_ignored_entries` and friends) —
//!   used by `copy_ignored` and `promote`.

use std::path::{Path, PathBuf};

use anyhow::Context;
use ignore::gitignore::GitignoreBuilder;
use worktrunk::config::CopyIgnoredConfig;
use worktrunk::git::Repository;
use worktrunk::shell_exec::Cmd;
use worktrunk::styling::{format_bash_with_gutter, format_heading, format_with_gutter};

use super::super::commit::CommitGenerator;

/// Print the three dry-run sections: rendered prompt, LLM command, generated message.
///
/// The COMMAND and MESSAGE sections use the same gutter treatment as the regular commit
/// flow — `format_bash_with_gutter` for the shell invocation, and the bold-first-line
/// commit message format wrapped in `format_with_gutter`. The PROMPT is left ungutter'd
/// to keep `--dry-run`'s output visually aligned with `--show-prompt`.
///
/// Output is routed through the pager when stdout is a TTY (see writing-user-outputs →
/// "When to page output"). The helper falls back to direct stdout when piped.
pub(super) fn print_dry_run(
    prompt: &str,
    commit_config: &worktrunk::config::CommitGenerationConfig,
    message: &str,
) -> anyhow::Result<()> {
    let command_block = match commit_config
        .command
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        Some(cmd) => format_bash_with_gutter(&crate::llm::render_llm_invocation(cmd)?),
        None => format_with_gutter("(LLM not configured — using built-in fallback)", None),
    };
    let formatted = CommitGenerator::new(commit_config).format_message_for_display(message);
    let out = format!(
        "{prompt_heading}\n{prompt}\n\n{command_heading}\n{command_block}\n\n{message_heading}\n{message_block}\n",
        prompt_heading = format_heading("PROMPT", None),
        command_heading = format_heading("COMMAND", None),
        message_heading = format_heading("MESSAGE", None),
        message_block = format_with_gutter(&formatted, None),
    );

    crate::help_pager::show_help_in_pager(&out, true);
    Ok(())
}

/// Built-in excludes for `wt step copy-ignored`: VCS metadata + tool-state directories.
///
/// VCS directories contain internal state tied to a specific working directory.
/// Git's own `.git` is implicitly excluded (git ls-files never reports it), but
/// other VCS tools colocated with git need explicit exclusion. Tool-state
/// directories (`.conductor/`, `.worktrees/`, etc.) are project-local state that
/// shouldn't be shared between worktrees.
pub(super) const BUILTIN_COPY_IGNORED_EXCLUDES: &[&str] = &[
    ".bzr/",
    ".conductor/",
    ".entire/",
    ".hg/",
    ".jj/",
    ".pijul/",
    ".sl/",
    ".svn/",
    ".worktrees/",
];

fn default_copy_ignored_excludes() -> Vec<String> {
    BUILTIN_COPY_IGNORED_EXCLUDES
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

/// Resolve the full copy-ignored config by merging built-in defaults, project
/// config (`.config/wt.toml`), and user config (global + per-project overrides).
pub(super) fn resolve_copy_ignored_config(repo: &Repository) -> anyhow::Result<CopyIgnoredConfig> {
    let mut config = CopyIgnoredConfig {
        exclude: default_copy_ignored_excludes(),
    };
    let user_config = repo.user_config();
    let project_config = repo
        .project_config()
        .context("Failed to load project config")?;
    if let Some(project_config) = project_config
        && let Some(project_copy_ignored) = project_config.copy_ignored()
    {
        config = config.merged_with(project_copy_ignored);
    }
    let project_id = repo.project_identifier().ok();
    config = config.merged_with(&user_config.copy_ignored(project_id.as_deref()));
    Ok(config)
}

/// List gitignored entries in a worktree, filtered by `.worktreeinclude` and excluding
/// configured patterns, VCS metadata directories, and entries that contain nested worktrees.
///
/// Combines five steps:
/// 1. `list_ignored_entries()` — git ls-files for ignored entries
/// 2. `.worktreeinclude` filtering — only matching entries if the file exists
/// 3. `[step.copy-ignored].exclude` filtering — skip entries matching configured patterns
/// 4. Built-in exclude filtering — always skip VCS metadata and tool-state directories
/// 5. Nested worktree filtering — exclude entries containing other worktrees
pub(super) fn list_and_filter_ignored_entries(
    worktree_path: &Path,
    context: &str,
    worktree_paths: &[PathBuf],
    exclude_patterns: &[String],
) -> anyhow::Result<Vec<(PathBuf, bool)>> {
    let ignored_entries = list_ignored_entries(worktree_path, context)?;

    // Filter to entries that match .worktreeinclude (or all if no file exists)
    let include_path = worktree_path.join(".worktreeinclude");
    let filtered: Vec<_> = if include_path.exists() {
        let include_matcher = {
            let mut builder = GitignoreBuilder::new(worktree_path);
            if let Some(err) = builder.add(&include_path) {
                // The `ignore` crate formats the path with OS-native separators;
                // normalize to forward slashes for consistent display.
                return Err(worktrunk::git::GitError::WorktreeIncludeParseError {
                    error: err.to_string().replace('\\', "/"),
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

    // Build exclude matcher for configured patterns (if any)
    let exclude_matcher = if exclude_patterns.is_empty() {
        None
    } else {
        let mut builder = GitignoreBuilder::new(worktree_path);
        for pattern in exclude_patterns {
            builder.add_line(None, pattern).map_err(|error| {
                anyhow::anyhow!(
                    "Invalid [step.copy-ignored].exclude pattern {:?}: {}",
                    pattern,
                    error
                )
            })?;
        }
        Some(
            builder
                .build()
                .context("Failed to build copy-ignored exclude matcher")?,
        )
    };

    // Filter out excluded patterns, VCS metadata directories, and nested worktrees
    Ok(filtered
        .into_iter()
        .filter(|(path, is_dir)| {
            // Skip entries matching configured exclude patterns
            if let Some(ref matcher) = exclude_matcher {
                let relative = path.strip_prefix(worktree_path).unwrap_or(path.as_path());
                if matcher.matched(relative, *is_dir).is_ignore() {
                    return false;
                }
            }
            // Skip built-in excluded directories (.jj, .hg, .worktrees, etc.)
            if *is_dir
                && path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|name| {
                        BUILTIN_COPY_IGNORED_EXCLUDES
                            .iter()
                            .any(|pat| pat.trim_end_matches('/') == name)
                    })
            {
                return false;
            }
            // Skip entries that contain other worktrees
            !worktree_paths
                .iter()
                .any(|wt_path| wt_path != worktree_path && wt_path.starts_with(path))
        })
        .collect())
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
