//! Worktree path computation and display.
//!
//! Where a worktree *belongs* — the `worktree-path` template, the expected-path
//! check that names a worktree in output, and the bare-repo template prompt.
//! Turning what the user typed into a worktree is the opposite direction and
//! lives in [`Repository::resolve_worktree`](worktrunk::git::Repository::resolve_worktree).

use std::io::IsTerminal;
use std::path::PathBuf;

use color_print::cformat;
use normalize_path::NormalizePath;
use worktrunk::config::UserConfig;
use worktrunk::git::Repository;
use worktrunk::path::{format_path_for_display, paths_match};
use worktrunk::styling::{
    eprintln, format_toml, hint_message, info_message, success_message, warning_message,
};

use crate::output::prompt::{PromptResponse, prompt_yes_no_preview};

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
    config: &mut UserConfig,
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

    if repo.has_shown_hint("skip-bare-repo-prompt") {
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

    let config_path_display = worktrunk::config::config_path_for_display();

    // Non-interactive: warn and show the config to add.
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
                "To place worktrees at <underline>{example_good}</>, add to <underline>{config_path_display}</>:"
            ))
        );
        let config_snippet =
            format!("[projects.\"{project_id}\"]\nworktree-path = \"{BARE_REPO_WORKTREE_PATH}\"");
        eprintln!("{}", format_toml(&config_snippet));
        return Ok(false);
    }

    // Interactive: show diagnosis, then prompt
    eprintln!(
        "{}",
        warning_message(cformat!(
            "Bare repo at <bold>{parent_name}/{repo_name}</> — worktrees will be at <bold>{example_bad}</>"
        ))
    );

    let config_path_for_preview = config_path_display.clone();
    let project_id_for_preview = project_id.clone();
    match prompt_yes_no_preview(
        &cformat!("Configure worktree-path to place worktrees at <bold>{example_good}</>?"),
        move || {
            eprintln!(
                "{}",
                info_message(cformat!("Would add to <bold>{config_path_for_preview}</>:"))
            );
            let preview = format!(
                "[projects.\"{project_id_for_preview}\"]\nworktree-path = \"{BARE_REPO_WORKTREE_PATH}\""
            );
            eprintln!("{}", format_toml(&preview));
            eprintln!();
        },
    )? {
        PromptResponse::Accepted => {
            config.set_project_worktree_path(
                &project_id,
                BARE_REPO_WORKTREE_PATH.to_string(),
                &worktrunk::config::require_config_path()?,
            )?;
            print_accepted_message(&display_path, &config_path_display);
            Ok(true)
        }
        PromptResponse::Declined => {
            // Best-effort, like every other hint write: a failed persist just
            // means the prompt may reappear on the next switch.
            let _ = repo.mark_hint_shown("skip-bare-repo-prompt");
            Ok(false)
        }
    }
}

fn print_accepted_message(display_path: &str, config_path: &str) {
    eprintln!(
        "{}",
        success_message(cformat!(
            "Set <bold>worktree-path</> for <bold>{display_path}</>:"
        ))
    );
    let global_config = format!("worktree-path = \"{BARE_REPO_WORKTREE_PATH}\"");
    eprintln!("{}", format_toml(&global_config));
    eprintln!(
        "{}",
        hint_message(cformat!(
            "To set globally, add to <underline>{config_path}</>"
        ))
    );

    // Blank line separates this setup phase from the main operation that follows
    eprintln!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_template_references_repo_name() {
        let cases = [
            (
                "default template",
                "{{ repo_path }}/../{{ repo }}.{{ branch | sanitize }}",
                true,
            ),
            ("repo with filter", "{{ repo | sanitize }}", true),
            (
                "deprecated main_worktree alias",
                "{{ main_worktree }}.{{ branch }}",
                true,
            ),
            (
                "repo_path is a distinct variable",
                "{{ repo_path }}/../{{ branch | sanitize }}",
                false,
            ),
            (
                "no repository variable",
                "../{{ branch | sanitize }}",
                false,
            ),
            ("expression without spaces", "{{repo}}.{{branch}}", true),
            (
                "repo outside an expression",
                "my-repo-path/{{ branch }}",
                false,
            ),
            ("repo suffix in myrepo", "{{ myrepo }}", false),
            ("repo suffix in norepo", "{{ norepo }}", false),
        ];

        for (name, template, expected) in cases {
            assert_eq!(
                template_references_repo_name(template),
                expected,
                "{name}: {template}"
            );
        }
    }
}
