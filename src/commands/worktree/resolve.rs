//! Worktree resolution and path computation.
//!
//! Functions for resolving worktree arguments and computing expected paths.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::Context;
use color_print::cformat;
use normalize_path::NormalizePath;
use worktrunk::config::UserConfig;
use worktrunk::git::{Repository, ResolvedWorktree};
use worktrunk::path::{format_path_for_display, paths_match};
use worktrunk::styling::{
    eprintln, format_bash_with_gutter, format_toml, hint_message, info_message, success_message,
    warning_message,
};

use crate::output::prompt::{PromptResponse, prompt_yes_no_preview};

/// Resolve a worktree argument using branch-first lookup.
///
/// Resolution order:
/// 1. Special symbols ("@", "-", "^") are handled specially
/// 2. Resolve argument as branch name
/// 3. If branch has a worktree, return it
/// 4. Fall back to path-based lookup (supports detached worktrees)
/// 5. Otherwise, return branch-only (no worktree)
///
/// If branch lookup fails to find a worktree, the argument is tried as a
/// filesystem path (absolute or relative to CWD). This supports removing
/// detached HEAD worktrees which have no branch name.
pub fn resolve_worktree_arg(repo: &Repository, name: &str) -> anyhow::Result<ResolvedWorktree> {
    // Special symbols - delegate to Repository for consistent error handling
    match name {
        "@" | "-" | "^" => {
            return repo.resolve_worktree(name);
        }
        _ => {}
    }

    // Resolve as branch name
    let branch = repo.resolve_worktree_name(name)?;

    // Branch-first: check if branch has worktree anywhere
    if let Some(path) = repo.worktree_for_branch(&branch)? {
        return Ok(ResolvedWorktree::Worktree {
            path,
            branch: Some(branch),
        });
    }

    // No worktree for branch - fall back to path-based lookup (supports detached worktrees)
    let candidate = Path::new(name);
    // Try as absolute path, or resolve relative to CWD
    let abs_path = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        std::env::current_dir()
            .context("Failed to determine current directory")?
            .join(candidate)
    };
    if let Some((path, wt_branch)) = repo.worktree_at_path(&abs_path)? {
        return Ok(ResolvedWorktree::Worktree {
            path,
            branch: wt_branch,
        });
    }

    // No worktree for branch and no worktree at the path
    Ok(ResolvedWorktree::BranchOnly { branch })
}

/// Compute the expected worktree path for a branch name.
///
/// For the default branch, returns the repo root (main worktree location).
/// For other branches, applies the `worktree-path` template from config.
///
/// Uses cached values from Repository for `default_branch` and `is_bare`.
pub fn compute_worktree_path(
    repo: &Repository,
    branch: &str,
    config: &UserConfig,
) -> anyhow::Result<PathBuf> {
    let repo_root = repo.repo_path()?;
    let default_branch = repo.default_branch().unwrap_or_default();
    let is_bare = repo.is_bare()?;

    // Default branch lives at repo root (main worktree), not a templated path.
    // Exception: bare repos have no main worktree, so all branches use templated paths.
    if !is_bare && branch == default_branch {
        return Ok(repo_root.to_path_buf());
    }

    let repo_name = repo_root
        .file_name()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Repository path has no filename: {}",
                format_path_for_display(repo_root)
            )
        })?
        .to_str()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Repository path contains invalid UTF-8: {}",
                format_path_for_display(repo_root)
            )
        })?;

    let project = repo.project_identifier().ok();
    let expanded_path = config.format_path(repo_name, branch, repo, project.as_deref())?;

    Ok(repo_root.join(expanded_path).normalize())
}

/// Check if a worktree is at its expected path based on config template.
///
/// Returns true if the worktree's actual path matches what `compute_worktree_path`
/// would generate for its branch. Detached HEAD always returns false (no expected path).
///
/// Uses canonicalization to handle symlinks and relative paths correctly.
/// Uses cached values from Repository for `default_branch` and `is_bare`.
pub fn is_worktree_at_expected_path(
    wt: &worktrunk::git::WorktreeInfo,
    repo: &Repository,
    config: &UserConfig,
) -> bool {
    match &wt.branch {
        Some(branch) => compute_worktree_path(repo, branch, config)
            .map(|expected| paths_match(&wt.path, &expected))
            .unwrap_or(false),
        None => false,
    }
}

/// Returns the expected path if `actual_path` differs from the template-computed path.
///
/// Returns `Some(expected_path)` when there's a mismatch, `None` when paths match.
/// Used to show path mismatch warnings in switch, picker, remove, and merge.
pub fn path_mismatch(
    repo: &Repository,
    branch: &str,
    actual_path: &std::path::Path,
    config: &UserConfig,
) -> Option<PathBuf> {
    compute_worktree_path(repo, branch, config)
        .ok()
        .filter(|expected| !paths_match(actual_path, expected))
}

/// Compute a user-facing display name for a worktree.
///
/// Returns styled content with branch names bolded:
/// - If branch is consistent with worktree location: just the branch name (bolded)
/// - If branch differs from expected location: `dir_name (on **branch**)` (both bolded)
/// - If detached HEAD: `dir_name (detached)` (dir_name bolded)
///
/// "Consistent" means the worktree path matches `compute_worktree_path(branch)`,
/// which returns repo root for default branch and templated path for others.
pub fn worktree_display_name(
    wt: &worktrunk::git::WorktreeInfo,
    repo: &Repository,
    config: &UserConfig,
) -> String {
    let dir_name = wt.dir_name();

    match &wt.branch {
        Some(branch) => {
            if is_worktree_at_expected_path(wt, repo, config) {
                cformat!("<bold>{branch}</>")
            } else {
                cformat!("<bold>{dir_name}</> (on <bold>{branch}</>)")
            }
        }
        None => cformat!("<bold>{dir_name}</> (detached)"),
    }
}

/// Suggested worktree-path template for bare repos with hidden directory names.
///
/// Places worktrees as siblings of the bare repo directory inside the parent,
/// e.g., `myproject/.git` + branch `feature` → `myproject/feature`.
/// Uses an absolute path (`repo_path`/../) to avoid ambiguity with relative resolution.
const BARE_REPO_WORKTREE_PATH: &str = "{{ repo_path }}/../{{ branch | sanitize }}";

/// Check whether a template string references `{{ repo }}` or `{{ main_worktree }}`.
fn template_references_repo_name(template: &str) -> bool {
    worktrunk::config::template_references_var(template, "repo")
        || worktrunk::config::template_references_var(template, "main_worktree")
}

/// Offer to set a project-level `worktree-path` for bare repos with hidden directory names.
///
/// When a bare repo lives at a hidden path like `.git` or `.bare`, the `{{ repo }}`
/// template variable resolves to that directory name, producing broken worktree paths.
/// This function detects the situation and offers to set a project-level override.
///
/// Returns `true` if config was modified (caller should use the updated config).
pub fn offer_bare_repo_worktree_path_fix(
    repo: &Repository,
    config: &UserConfig,
    branch: &str,
) -> anyhow::Result<bool> {
    if !repo.is_bare()? {
        return Ok(false);
    }

    // Symbolic identifiers (-, @, ^, pr:N, mr:N) haven't been resolved to a real
    // branch name yet, so the example paths would be misleading. Skip the prompt;
    // it will surface on the next switch with a concrete branch name.
    if branch == "-" || branch == "@" || branch == "^" || branch.contains(':') {
        return Ok(false);
    }

    let repo_path = repo.repo_path()?;
    let repo_name = repo_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if !repo_name.starts_with('.') {
        return Ok(false);
    }

    if repo
        .config_value("worktrunk.skip-bare-repo-prompt")
        .unwrap_or(None)
        .is_some()
    {
        return Ok(false);
    }

    // Skip if git config already has an explicit per-clone override.
    if repo
        .config_value("worktrunk.worktree-path")
        .unwrap_or(None)
        .is_some()
    {
        return Ok(false);
    }

    let project_id = repo.project_identifier()?;
    let template = config.worktree_path_for_project(&project_id);
    if !template_references_repo_name(&template) {
        return Ok(false);
    }

    // Display names for messages
    let display_path = repo_path
        .parent()
        .map(|p| format_path_for_display(p).to_string())
        .unwrap_or_else(|| format_path_for_display(repo_path).to_string());
    let parent_name = repo_path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("project");

    // Use the actual branch being switched to in example paths. Calling
    // `sanitize_branch_name` keeps the displayed example aligned with what
    // the `sanitize` Jinja filter applies inside `BARE_REPO_WORKTREE_PATH`.
    let sanitized = worktrunk::config::sanitize_branch_name(branch);
    let example_bad = format!("{parent_name}/{repo_name}.{sanitized}");
    let example_good = format!("{parent_name}/{sanitized}");

    // Non-interactive: warn and show the git config command to run.
    if !std::io::stdin().is_terminal() {
        eprintln!(
            "{}",
            warning_message(cformat!(
                "Bare repo at <bold>{parent_name}/{repo_name}</> — worktrees will be at <bold>{example_bad}</>"
            ))
        );
        eprintln!(
            "{}",
            hint_message(cformat!(
                "To place worktrees at <underline>{example_good}</>, run:"
            ))
        );
        eprintln!(
            "{}",
            format_bash_with_gutter(&format!(
                "git config worktrunk.worktree-path \"{BARE_REPO_WORKTREE_PATH}\""
            ))
        );
        return Ok(false);
    }

    // Interactive: show diagnosis, then prompt
    eprintln!(
        "{}",
        warning_message(cformat!(
            "Bare repo at <bold>{parent_name}/{repo_name}</> — worktrees will be at <bold>{example_bad}</>"
        ))
    );

    match prompt_yes_no_preview(
        &cformat!("Configure worktree-path to place worktrees at <bold>{example_good}</>?"),
        move || {
            eprintln!(
                "{}",
                info_message("Would set in .git/config (this clone only):")
            );
            eprintln!(
                "{}",
                format_toml(&format!(
                    "worktrunk.worktree-path = \"{BARE_REPO_WORKTREE_PATH}\""
                ))
            );
            eprintln!();
        },
    )? {
        PromptResponse::Accepted => {
            repo.set_config("worktrunk.worktree-path", BARE_REPO_WORKTREE_PATH)?;
            print_accepted_message(&display_path);
            Ok(true)
        }
        PromptResponse::Declined => {
            if let Err(e) = repo.set_config("worktrunk.skip-bare-repo-prompt", "true") {
                log::warn!("Failed to save skip-bare-repo-prompt to git config: {e}");
            }
            Ok(false)
        }
    }
}

fn print_accepted_message(display_path: &str) {
    eprintln!(
        "{}",
        success_message(cformat!(
            "Set <bold>worktrunk.worktree-path</> for <bold>{display_path}</> (this clone only):"
        ))
    );
    eprintln!(
        "{}",
        format_toml(&format!(
            "worktrunk.worktree-path = \"{BARE_REPO_WORKTREE_PATH}\""
        ))
    );

    // Blank line separates this setup phase from the main operation that follows
    eprintln!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_template_references_repo_name_default() {
        // Default template uses {{ repo }}
        assert!(template_references_repo_name(
            "{{ repo_path }}/../{{ repo }}.{{ branch | sanitize }}"
        ));
    }

    #[test]
    fn test_template_references_repo_name_with_filter() {
        assert!(template_references_repo_name("{{ repo | sanitize }}"));
    }

    #[test]
    fn test_template_references_repo_name_deprecated_alias() {
        assert!(template_references_repo_name(
            "{{ main_worktree }}.{{ branch }}"
        ));
    }

    #[test]
    fn test_template_references_repo_name_not_repo_path() {
        // {{ repo_path }} should NOT match
        assert!(!template_references_repo_name(
            "{{ repo_path }}/../{{ branch | sanitize }}"
        ));
    }

    #[test]
    fn test_template_references_repo_name_no_repo() {
        assert!(!template_references_repo_name("../{{ branch | sanitize }}"));
    }

    #[test]
    fn test_template_references_repo_name_no_spaces() {
        assert!(template_references_repo_name("{{repo}}.{{branch}}"));
    }

    #[test]
    fn test_template_references_repo_name_no_braces() {
        // "repo" outside template expressions should not match
        assert!(!template_references_repo_name("my-repo-path/{{ branch }}"));
    }

    #[test]
    fn test_template_references_repo_name_substring_prefix() {
        // "myrepo" should NOT match — "repo" is a suffix of a longer identifier
        assert!(!template_references_repo_name("{{ myrepo }}"));
        assert!(!template_references_repo_name("{{ norepo }}"));
    }
}
