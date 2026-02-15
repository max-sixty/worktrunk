//! VCS detection by filesystem markers.
//!
//! Walks ancestor directories looking for `.jj/` or `.git/` to determine
//! which VCS manages the repository. Co-located repos (both markers present)
//! prefer jj.

use std::path::Path;

use super::VcsKind;

/// Detect which VCS manages the repository containing `path`.
///
/// Walks ancestors looking for `.jj/` and `.git/` markers. At each level:
/// - `.jj/` present → jj (even if `.git/` also exists, since co-located repos
///   have both and jj is the primary VCS)
/// - `.git/` present (file or directory) → git
///
/// Returns `None` if no VCS markers are found.
pub fn detect_vcs(path: &Path) -> Option<VcsKind> {
    let mut current = Some(path);
    while let Some(dir) = current {
        if dir.join(".jj").is_dir() {
            return Some(VcsKind::Jj);
        }
        // .git can be a directory (normal repo) or file (worktree link)
        if dir.join(".git").exists() {
            return Some(VcsKind::Git);
        }
        current = dir.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn test_detect_git_repo() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();

        assert_eq!(detect_vcs(dir.path()), Some(VcsKind::Git));
    }

    #[test]
    fn test_detect_jj_repo() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(".jj")).unwrap();

        assert_eq!(detect_vcs(dir.path()), Some(VcsKind::Jj));
    }

    #[test]
    fn test_detect_colocated_prefers_jj() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(".jj")).unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();

        assert_eq!(detect_vcs(dir.path()), Some(VcsKind::Jj));
    }

    #[test]
    fn test_detect_no_vcs() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(detect_vcs(dir.path()), None);
    }

    #[test]
    fn test_detect_in_subdirectory() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        let sub = dir.path().join("src").join("lib");
        fs::create_dir_all(&sub).unwrap();

        assert_eq!(detect_vcs(&sub), Some(VcsKind::Git));
    }

    #[test]
    fn test_detect_git_worktree_file() {
        let dir = tempfile::tempdir().unwrap();
        // Git worktrees use a .git file (not directory) pointing to the main repo
        fs::write(dir.path().join(".git"), "gitdir: /some/path").unwrap();

        assert_eq!(detect_vcs(dir.path()), Some(VcsKind::Git));
    }
}
