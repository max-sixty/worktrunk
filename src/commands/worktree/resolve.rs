//! Worktree resolution and path computation.
//!
//! Functions for resolving worktree arguments and computing expected paths.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::Context;
use color_print::cformat;
use normalize_path::NormalizePath;
use worktrunk::config::UserConfig;
use worktrunk::git::{GitError, Repository, ResolvedWorktree};
use worktrunk::path::{format_path_for_display, paths_match};
use worktrunk::styling::{
    eprintln, format_toml, hint_message, info_message, success_message, warning_message,
};

use crate::output::prompt::{PromptResponse, prompt_yes_no_preview};

use super::types::OperationMode;

/// Resolve a worktree argument using branch-first lookup.
///
/// Resolution order:
/// 1. Special symbols ("@", "-", "^") are handled specially
/// 2. Resolve argument as branch name
/// 3. If branch has a worktree, return it
/// 4. For `Remove`: fall back to path-based lookup (supports detached worktrees)
/// 5. Otherwise, return branch-only (no worktree)
///
/// For `CreateOrSwitch` context: If the branch has no worktree but expected
/// path is occupied by another branch's worktree, an error is raised.
///
/// For `Remove` context: If branch lookup fails to find a worktree, the
/// argument is tried as a filesystem path (absolute or relative to CWD).
/// This supports removing detached HEAD worktrees which have no branch name.
pub fn resolve_worktree_arg(
    repo: &Repository,
    name: &str,
    config: &UserConfig,
    context: OperationMode,
) -> anyhow::Result<ResolvedWorktree> {
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

    // No worktree for branch - check if expected path is occupied (only for create/switch)
    if context == OperationMode::CreateOrSwitch {
        let expected_path = compute_worktree_path(repo, name, config)?;
        if let Some((_, occupant_branch)) = repo.worktree_at_path(&expected_path)? {
            // Path is occupied by a different branch's worktree
            return Err(GitError::WorktreePathOccupied {
                branch,
                path: expected_path,
                occupant: occupant_branch,
            }
            .into());
        }
    }

    // For Remove: fall back to path-based lookup (supports detached worktrees)
    if context == OperationMode::Remove {
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
    }

    // No worktree for branch (and path not occupied, or we don't care about path)
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

/// Generate a backup path for the given path with a timestamp suffix.
///
/// For paths with extensions: `file.txt` → `file.txt.bak.TIMESTAMP`
/// For paths without extensions: `foo` → `foo.bak.TIMESTAMP`
///
/// Returns an error for unusual paths without a file name (e.g., `/` or `..`).
pub(super) fn generate_backup_path(
    path: &std::path::Path,
    suffix: &str,
) -> anyhow::Result<PathBuf> {
    let file_name = path.file_name().ok_or_else(|| {
        anyhow::anyhow!(
            "Cannot generate backup path for {}",
            format_path_for_display(path)
        )
    })?;

    if path.extension().is_none() {
        // Path has no extension (e.g., /repo/feature)
        Ok(path.with_file_name(format!("{}.bak.{suffix}", file_name.to_string_lossy())))
    } else {
        // Path has an extension (e.g., /repo.feature or /file.txt)
        Ok(path.with_extension(format!(
            "{}.bak.{suffix}",
            path.extension()
                .map(|e| e.to_string_lossy().to_string())
                .unwrap_or_default()
        )))
    }
}

/// Move a stale path aside so `wt switch --clobber` can take its place.
///
/// Renames `worktree_path` to a `.bak.<base_suffix>` sibling. If that name is
/// already taken — a same-second clobber, or a path that raced in after
/// planning — it counts up (`…-2`, `…-3`, …) until it finds a free name.
/// Every attempt is an atomic no-overwrite rename
/// ([`worktrunk::fs::rename_no_replace`]), so an existing backup is never
/// overwritten; the move just lands on the next free name. Returns the path
/// the stale directory was moved to.
pub(super) fn back_up_clobbered_path(
    worktree_path: &Path,
    base_suffix: &str,
) -> anyhow::Result<PathBuf> {
    // Count up until a free name is found. This cannot spin forever: a
    // directory holds finitely many entries, so some `-N` is always unused.
    let mut n: u64 = 1;
    loop {
        // First attempt uses the bare suffix; later ones disambiguate with -N.
        let suffix = if n == 1 {
            base_suffix.to_string()
        } else {
            format!("{base_suffix}-{n}")
        };
        let candidate = generate_backup_path(worktree_path, &suffix)?;
        match worktrunk::fs::rename_no_replace(worktree_path, &candidate) {
            Ok(()) => return Ok(candidate),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
            Err(err) => {
                return Err(anyhow::Error::new(err).context(format!(
                    "Failed to move {} to {}",
                    format_path_for_display(worktree_path),
                    format_path_for_display(&candidate),
                )));
            }
        }
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
) -> anyhow::Result<bool> {
    if !repo.is_bare()? {
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

    // Example paths to show the user what changes
    let example_bad = format!("{parent_name}/{repo_name}.feature-auth");
    let example_good = format!("{parent_name}/feature-auth");

    let config_path_display = worktrunk::config::config_path()
        .map(|p| format_path_for_display(&p).to_string())
        .unwrap_or_else(|| "~/.config/worktrunk/config.toml".to_string());

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
                None,
            )?;
            print_accepted_message(&display_path, &config_path_display);
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
    fn test_generate_backup_path_with_extension() {
        // Paths with extensions: file.txt -> file.txt.bak.TIMESTAMP
        let path = PathBuf::from("/tmp/repo.feature");
        let backup = generate_backup_path(&path, "20250101-000000").unwrap();
        assert_eq!(
            backup,
            PathBuf::from("/tmp/repo.feature.bak.20250101-000000")
        );

        let path = PathBuf::from("/tmp/file.txt");
        let backup = generate_backup_path(&path, "20250101-000000").unwrap();
        assert_eq!(backup, PathBuf::from("/tmp/file.txt.bak.20250101-000000"));
    }

    #[test]
    fn test_generate_backup_path_without_extension() {
        // Paths without extensions: foo -> foo.bak.TIMESTAMP
        let path = PathBuf::from("/tmp/repo/feature");
        let backup = generate_backup_path(&path, "20250101-000000").unwrap();
        assert_eq!(
            backup,
            PathBuf::from("/tmp/repo/feature.bak.20250101-000000")
        );

        let path = PathBuf::from("/tmp/mydir");
        let backup = generate_backup_path(&path, "20250101-000000").unwrap();
        assert_eq!(backup, PathBuf::from("/tmp/mydir.bak.20250101-000000"));
    }

    #[test]
    fn test_generate_backup_path_unusual_paths() {
        // Root path has no file name
        let path = PathBuf::from("/");
        assert!(generate_backup_path(&path, "20250101-000000").is_err());

        // Parent reference has no file name
        let path = PathBuf::from("..");
        assert!(generate_backup_path(&path, "20250101-000000").is_err());
    }

    #[test]
    fn test_back_up_clobbered_path_moves_to_fresh_suffix() {
        let temp = tempfile::tempdir().unwrap();
        let stale = temp.path().join("feature");
        std::fs::create_dir(&stale).unwrap();
        std::fs::write(stale.join("file"), "content").unwrap();

        let used = back_up_clobbered_path(&stale, "20250101-000000").unwrap();

        assert_eq!(used, temp.path().join("feature.bak.20250101-000000"));
        assert!(!stale.exists(), "stale path should be moved away");
        assert_eq!(
            std::fs::read_to_string(used.join("file")).unwrap(),
            "content"
        );
    }

    #[test]
    fn test_back_up_clobbered_path_falls_back_when_suffix_taken() {
        let temp = tempfile::tempdir().unwrap();
        let stale = temp.path().join("feature");
        std::fs::create_dir(&stale).unwrap();

        // The preferred backup name and its first -N variant are both taken.
        let taken = temp.path().join("feature.bak.20250101-000000");
        std::fs::create_dir(&taken).unwrap();
        std::fs::write(taken.join("keep"), "pre-existing").unwrap();
        std::fs::create_dir(temp.path().join("feature.bak.20250101-000000-2")).unwrap();

        let used = back_up_clobbered_path(&stale, "20250101-000000").unwrap();

        // Lands on -3; neither pre-existing backup is overwritten.
        assert_eq!(used, temp.path().join("feature.bak.20250101-000000-3"));
        assert!(!stale.exists());
        assert_eq!(
            std::fs::read_to_string(taken.join("keep")).unwrap(),
            "pre-existing"
        );
    }

    #[test]
    fn test_back_up_clobbered_path_errors_when_source_missing() {
        // A missing source fails the rename with a non-AlreadyExists error,
        // which surfaces rather than being retried.
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("does-not-exist");
        assert!(back_up_clobbered_path(&missing, "20250101-000000").is_err());
    }

    #[test]
    fn test_back_up_clobbered_path_keeps_incrementing_past_many_collisions() {
        // There is no attempt cap: the move keeps counting up until a free
        // name is found, however many backups already exist.
        let temp = tempfile::tempdir().unwrap();
        let stale = temp.path().join("feature");
        std::fs::create_dir(&stale).unwrap();

        // Occupy the preferred name and the first 49 -N fallbacks (suffix "S").
        std::fs::create_dir(temp.path().join("feature.bak.S")).unwrap();
        for n in 2..=50 {
            std::fs::create_dir(temp.path().join(format!("feature.bak.S-{n}"))).unwrap();
        }

        let used = back_up_clobbered_path(&stale, "S").unwrap();

        assert_eq!(used, temp.path().join("feature.bak.S-51"));
        assert!(!stale.exists(), "stale path should be moved away");
    }

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
