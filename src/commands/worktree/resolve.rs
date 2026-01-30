//! Worktree resolution and path computation.
//!
//! Functions for resolving worktree arguments and computing expected paths.

use std::path::{Path, PathBuf};

use color_print::cformat;
use dunce::canonicalize;
use normalize_path::NormalizePath;
use worktrunk::config::UserConfig;
use worktrunk::git::{GitError, Repository, ResolvedWorktree};
use worktrunk::path::format_path_for_display;

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
    let repo_root = repo.repo_path();
    let default_branch = repo.default_branch().unwrap_or_default();
    let is_bare = repo.is_bare();

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
    let expanded_path = config
        .format_path(repo_name, branch, repo, project.as_deref())
        .map_err(|e| anyhow::anyhow!("Failed to format worktree path: {e}"))?;

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
/// Used to show path mismatch warnings in `wt remove` and `wt merge`.
pub fn get_path_mismatch(
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
        let timestamp = worktrunk::utils::get_now() as i64;
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
}
