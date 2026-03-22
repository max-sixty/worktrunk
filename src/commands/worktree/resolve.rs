//! Worktree resolution and path computation.
//!
//! Functions for resolving worktree arguments and computing expected paths.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use color_print::cformat;
use dunce::canonicalize;
use normalize_path::NormalizePath;
use worktrunk::config::UserConfig;
use worktrunk::git::{GitError, Repository, ResolvedWorktree};
use worktrunk::path::format_path_for_display;
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
/// 4. Otherwise, return branch-only (no worktree)
///
/// For `CreateOrSwitch` context: If the branch has no worktree but expected
/// path is occupied by another branch's worktree, an error is raised.
///
/// For `Remove` context: Path occupation is ignored since we're not creating
/// a worktree - we just return `BranchOnly` if no worktree exists.
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

/// Canonicalize a path, resolving parent symlinks even if the path doesn't exist.
///
/// For existing paths, uses standard canonicalization.
/// For non-existent paths, canonicalizes the longest existing prefix and appends
/// the remaining components. This handles macOS `/var` -> `/private/var` symlinks
/// correctly for computed worktree paths that don't exist yet.
fn canonicalize_with_parents(path: &Path) -> PathBuf {
    // Try direct canonicalization first
    if let Ok(canonical) = canonicalize(path) {
        return canonical;
    }

    // Path doesn't exist - find the longest existing prefix and canonicalize that
    let mut existing_prefix = path.to_path_buf();
    let mut suffix_components = Vec::new();

    // Walk up until we find an existing path
    while !existing_prefix.exists() {
        if let Some(file_name) = existing_prefix.file_name() {
            suffix_components.push(file_name.to_os_string());
            if let Some(parent) = existing_prefix.parent() {
                existing_prefix = parent.to_path_buf();
            } else {
                // Reached filesystem root without finding existing path
                return path.to_path_buf();
            }
        } else {
            // No more components to strip
            return path.to_path_buf();
        }
    }

    // Canonicalize the existing prefix and append the non-existent components
    let canonical_prefix = canonicalize(&existing_prefix).unwrap_or(existing_prefix);
    let mut result = canonical_prefix;
    for component in suffix_components.into_iter().rev() {
        result.push(component);
    }
    result
}

/// Compare two paths for equality, canonicalizing to handle symlinks and relative paths.
///
/// Returns `true` if the paths resolve to the same location.
/// Handles the case where one path exists and the other doesn't by resolving
/// parent directory symlinks for non-existent paths.
pub(crate) fn paths_match(a: &std::path::Path, b: &std::path::Path) -> bool {
    let a_canonical = canonicalize_with_parents(a);
    let b_canonical = canonicalize_with_parents(b);
    a_canonical == b_canonical
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

/// Compute the backup path for clobber operations.
///
/// Returns `Ok(None)` if path doesn't exist.
/// Returns `Ok(Some(backup_path))` if clobber is true and path exists.
/// Returns `Err(GitError::WorktreePathExists)` if clobber is false and path exists.
pub(super) fn compute_clobber_backup(
    path: &Path,
    branch: &str,
    clobber: bool,
    create: bool,
) -> anyhow::Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }

    if clobber {
        let timestamp = worktrunk::utils::epoch_now() as i64;
        let datetime =
            chrono::DateTime::from_timestamp(timestamp, 0).unwrap_or_else(chrono::Utc::now);
        let suffix = datetime.format("%Y%m%d-%H%M%S").to_string();
        let backup_path = generate_backup_path(path, &suffix)?;

        if backup_path.exists() {
            anyhow::bail!(
                "Backup path already exists: {}",
                worktrunk::path::format_path_for_display(&backup_path)
            );
        }
        Ok(Some(backup_path))
    } else {
        Err(GitError::WorktreePathExists {
            branch: branch.to_string(),
            path: path.to_path_buf(),
            create,
        }
        .into())
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
    let env = minijinja::Environment::new();
    let Ok(tmpl) = env.template_from_str(template) else {
        return false;
    };
    let vars = tmpl.undeclared_variables(false);
    vars.contains("repo") || vars.contains("main_worktree")
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
    fn test_paths_match_identical() {
        let path = PathBuf::from("/tmp/test");
        assert!(paths_match(&path, &path));
    }

    #[test]
    fn test_paths_match_different() {
        let a = PathBuf::from("/tmp/foo");
        let b = PathBuf::from("/tmp/bar");
        assert!(!paths_match(&a, &b));
    }

    #[test]
    fn test_canonicalize_with_parents_existing_path() {
        // Existing paths should be canonicalized normally
        let tmp = std::env::temp_dir();
        let canonical = canonicalize_with_parents(&tmp);
        // Should resolve to the actual canonical path
        assert!(canonical.is_absolute());
    }

    #[test]
    fn test_canonicalize_with_parents_nonexistent() {
        // Non-existent path under existing parent should resolve parent symlinks
        let tmp = std::env::temp_dir();
        let nonexistent = tmp.join("nonexistent-test-dir-12345");
        let canonical = canonicalize_with_parents(&nonexistent);

        // The parent (/tmp or similar) should be canonicalized
        // On macOS, /tmp -> /private/tmp, so the result should contain "private" if on macOS
        assert!(canonical.is_absolute());

        // The non-existent component should still be at the end
        assert_eq!(
            canonical.file_name().unwrap().to_str().unwrap(),
            "nonexistent-test-dir-12345"
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn test_paths_match_macos_var_symlink() {
        // On macOS, /var is a symlink to /private/var
        // This test verifies the fix for symlink resolution with non-existent paths

        // Create a temp directory under /var/tmp (which is /private/var/tmp)
        let test_dir = PathBuf::from("/var/tmp/wt-test-paths-match");

        // Clean up existing test state and create fresh
        let _ = std::fs::remove_dir_all(&test_dir);
        std::fs::create_dir_all(&test_dir).expect("Failed to create test dir");

        // Test: non-existent path via /private/var vs non-existent path via /var
        let private_path = PathBuf::from("/private/var/tmp/wt-test-paths-match/subdir");
        let var_path = PathBuf::from("/var/tmp/wt-test-paths-match/subdir");

        // Neither subdir exists, but parent does
        // paths_match should return true because both resolve to same location
        assert!(
            paths_match(&private_path, &var_path),
            "Paths should match: {:?} vs {:?}",
            canonicalize_with_parents(&private_path),
            canonicalize_with_parents(&var_path)
        );

        // Clean up
        let _ = std::fs::remove_dir_all(&test_dir);
    }

    #[test]
    fn test_paths_match_existing_vs_nonexistent() {
        // When one path exists and the other doesn't, parent symlinks should be resolved
        let tmp = std::env::temp_dir();

        // Create an existing directory
        let existing = tmp.join("wt-test-existing");
        std::fs::create_dir_all(&existing).expect("Failed to create test dir");

        // Non-existent sibling
        let nonexistent = tmp.join("wt-test-nonexistent");
        let _ = std::fs::remove_dir_all(&nonexistent);

        // These should NOT match (different paths)
        assert!(!paths_match(&existing, &nonexistent));

        // But same path with different representations should match
        // (this tests that we're correctly canonicalizing even when one side doesn't exist)
        let canonical = canonicalize_with_parents(&existing);
        assert!(paths_match(&existing, &canonical));

        // Clean up
        let _ = std::fs::remove_dir_all(&existing);
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
