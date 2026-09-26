//! Git output parsing functions

use std::path::PathBuf;

#[cfg(unix)]
use std::{ffi::OsString, os::unix::ffi::OsStringExt};

use super::{GitError, WorktreeInfo, finalize_worktree};

#[cfg(unix)]
pub(crate) fn path_from_git_bytes(path: &[u8]) -> PathBuf {
    PathBuf::from(OsString::from_vec(path.to_vec()))
}

#[cfg(not(unix))]
pub(crate) fn path_from_git_bytes(path: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(path).into_owned())
}

pub(crate) fn path_from_git_stdout(stdout: &[u8]) -> PathBuf {
    let path = stdout.strip_suffix(b"\n").unwrap_or(stdout);
    path_from_git_bytes(path)
}

impl WorktreeInfo {
    pub(crate) fn parse_porcelain_list_z(output: &[u8]) -> anyhow::Result<Vec<Self>> {
        Self::parse_porcelain_fields(output.split(|byte| *byte == b'\0'))
    }

    fn parse_porcelain_fields<'a>(
        fields: impl IntoIterator<Item = &'a [u8]>,
    ) -> anyhow::Result<Vec<Self>> {
        let mut worktrees = Vec::new();
        let mut current: Option<WorktreeInfo> = None;

        for line in fields {
            if line.is_empty() {
                if let Some(wt) = current.take() {
                    worktrees.push(finalize_worktree(wt));
                }
                continue;
            }

            let (key, value) = match line.iter().position(|byte| *byte == b' ') {
                Some(index) => (&line[..index], Some(&line[index + 1..])),
                None => (line, None),
            };

            match key {
                b"worktree" => {
                    let Some(path) = value else {
                        return Err(GitError::ParseError {
                            message: "worktree line missing path".into(),
                        }
                        .into());
                    };
                    current = Some(WorktreeInfo {
                        path: path_from_git_bytes(path),
                        head: String::new(),
                        branch: None,
                        bare: false,
                        detached: false,
                        locked: None,
                        prunable: None,
                    });
                }
                key => match (key, current.as_mut()) {
                    (b"HEAD", Some(wt)) => {
                        let Some(sha) = value else {
                            return Err(GitError::ParseError {
                                message: "HEAD line missing SHA".into(),
                            }
                            .into());
                        };
                        wt.head = String::from_utf8_lossy(sha).into_owned();
                    }
                    (b"branch", Some(wt)) => {
                        // Strip refs/heads/ prefix if present
                        let Some(branch_ref) = value else {
                            return Err(GitError::ParseError {
                                message: "branch line missing ref".into(),
                            }
                            .into());
                        };
                        let branch_ref = String::from_utf8_lossy(branch_ref);
                        let branch = branch_ref
                            .strip_prefix("refs/heads/")
                            .unwrap_or(&branch_ref)
                            .to_string();
                        wt.branch = Some(branch);
                    }
                    (b"bare", Some(wt)) => {
                        wt.bare = true;
                    }
                    (b"detached", Some(wt)) => {
                        wt.detached = true;
                    }
                    (b"locked", Some(wt)) => {
                        wt.locked =
                            Some(String::from_utf8_lossy(value.unwrap_or_default()).into_owned());
                    }
                    (b"prunable", Some(wt)) => {
                        wt.prunable =
                            Some(String::from_utf8_lossy(value.unwrap_or_default()).into_owned());
                    }
                    _ => {
                        // Ignore unknown attributes or attributes before first worktree
                    }
                },
            }
        }

        // Push the last worktree if the output doesn't end with a blank line
        if let Some(wt) = current {
            worktrees.push(finalize_worktree(wt));
        }

        Ok(worktrees)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DefaultBranchName(String);

impl DefaultBranchName {
    pub(crate) fn from_local(remote: &str, output: &str) -> anyhow::Result<Self> {
        let trimmed = output.trim();

        // Strip "remote/" prefix if present
        let prefix = format!("{}/", remote);
        let branch = trimmed.strip_prefix(&prefix).unwrap_or(trimmed);

        if branch.is_empty() {
            return Err(GitError::ParseError {
                message: format!("Empty branch name from {}/HEAD", remote),
            }
            .into());
        }

        Ok(Self(branch.to_string()))
    }

    pub(crate) fn from_remote(output: &str) -> anyhow::Result<Self> {
        output
            .lines()
            .find_map(|line| {
                line.strip_prefix("ref: ")
                    .and_then(|symref| symref.split_once('\t'))
                    .map(|(ref_path, _)| ref_path)
                    .and_then(|ref_path| ref_path.strip_prefix("refs/heads/"))
                    .map(|branch| branch.to_string())
            })
            .map(Self)
            .ok_or_else(|| {
                GitError::ParseError {
                    message: "Could not find symbolic ref in ls-remote output".into(),
                }
                .into()
            })
    }

    pub(crate) fn into_string(self) -> String {
        self.0
    }
}

/// Parse `git status --porcelain -z` output into a list of affected filenames.
///
/// The -z format uses NUL separators and handles renames specially:
/// - Normal entries: `XY path\0`
/// - Renames/copies: `XY new_path\0old_path\0`
///
/// This correctly handles filenames with spaces and ensures both old and new
/// paths are included for renames/copies (important for overlap detection).
/// Raw bytes keep distinct non-UTF-8 paths distinct during comparison.
pub fn parse_porcelain_z(output: &[u8]) -> Vec<Vec<u8>> {
    let mut files = Vec::new();
    let mut entries = output
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty());

    while let Some(entry) = entries.next() {
        // Each entry is "XY path" where XY is exactly 2 status chars
        if entry.len() < 3 {
            continue;
        }

        let status = &entry[0..2];
        let path = &entry[3..];
        files.push(path.to_vec());

        // For renames (R) and copies (C), the next NUL-separated field is the old path
        let has_source_path = status.contains(&b'R') || status.contains(&b'C');
        if has_source_path && let Some(old_path) = entries.next() {
            files.push(old_path.to_vec());
        }
    }

    files
}

/// Parse untracked files from `git status --porcelain -z` output.
///
/// Format: "XY path\0" where XY is the status code and path follows a space.
/// Untracked files have status "??".
pub fn parse_untracked_files(status_output: &str) -> Vec<String> {
    let mut files = Vec::new();
    let mut entries = status_output.split('\0').filter(|s| !s.is_empty());

    while let Some(entry) = entries.next() {
        // Format: "XY PATH" where XY is 2 status chars, space, then path
        if entry.len() < 3 {
            continue;
        }

        let status = &entry[0..2];
        let path = &entry[3..];

        // Only collect untracked files
        if status == "??" {
            files.push(path.to_string());
        }

        // Skip old path for renames/copies (we don't care about them here)
        if status.contains(['R', 'C']) {
            entries.next();
        }
    }

    files
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================================
    // DefaultBranchName::from_local Tests
    // ============================================================================

    #[test]
    fn test_from_local_simple() {
        let result = DefaultBranchName::from_local("origin", "main");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().into_string(), "main");
    }

    #[test]
    fn test_from_local_with_remote_prefix() {
        let result = DefaultBranchName::from_local("origin", "origin/main");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().into_string(), "main");
    }

    #[test]
    fn test_from_local_with_whitespace() {
        let result = DefaultBranchName::from_local("origin", "  main  \n");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().into_string(), "main");
    }

    #[test]
    fn test_from_local_empty() {
        let result = DefaultBranchName::from_local("origin", "");
        assert!(result.is_err());
    }

    #[test]
    fn test_from_local_only_whitespace() {
        let result = DefaultBranchName::from_local("origin", "   \n  ");
        assert!(result.is_err());
    }

    #[test]
    fn test_from_local_different_remote() {
        let result = DefaultBranchName::from_local("upstream", "upstream/develop");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().into_string(), "develop");
    }

    // ============================================================================
    // DefaultBranchName::from_remote Tests
    // ============================================================================

    #[test]
    fn test_from_remote_standard() {
        let output = "ref: refs/heads/main\tHEAD\n";
        let result = DefaultBranchName::from_remote(output);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().into_string(), "main");
    }

    #[test]
    fn test_from_remote_master() {
        let output = "ref: refs/heads/master\tHEAD\n";
        let result = DefaultBranchName::from_remote(output);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().into_string(), "master");
    }

    #[test]
    fn test_from_remote_with_other_lines() {
        let output = "abc123\tHEAD\nref: refs/heads/develop\tHEAD\ndef456\trefs/heads/main\n";
        let result = DefaultBranchName::from_remote(output);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().into_string(), "develop");
    }

    #[test]
    fn test_from_remote_no_ref() {
        let output = "abc123\tHEAD\n";
        let result = DefaultBranchName::from_remote(output);
        assert!(result.is_err());
    }

    #[test]
    fn test_from_remote_empty() {
        let result = DefaultBranchName::from_remote("");
        assert!(result.is_err());
    }

    // ============================================================================
    // WorktreeInfo::parse_porcelain_list_z Tests
    // ============================================================================

    #[test]
    fn test_parse_porcelain_list_single_worktree() {
        let output = b"worktree /path/to/repo\0HEAD abc123\0branch refs/heads/main\0\0";
        let worktrees = WorktreeInfo::parse_porcelain_list_z(output).unwrap();
        let [wt]: [WorktreeInfo; 1] = worktrees.try_into().unwrap();
        assert_eq!(wt.path.to_str().unwrap(), "/path/to/repo");
        assert_eq!(wt.head, "abc123");
        assert_eq!(wt.branch, Some("main".to_string()));
    }

    #[test]
    fn test_parse_porcelain_list_multiple_worktrees() {
        let output = b"worktree /path/main\0HEAD aaa\0branch refs/heads/main\0\0worktree /path/feature\0HEAD bbb\0branch refs/heads/feature\0\0";
        let worktrees = WorktreeInfo::parse_porcelain_list_z(output).unwrap();
        let [main_wt, feature_wt]: [WorktreeInfo; 2] = worktrees.try_into().unwrap();
        assert_eq!(main_wt.branch, Some("main".to_string()));
        assert_eq!(feature_wt.branch, Some("feature".to_string()));
    }

    #[test]
    fn test_parse_porcelain_list_bare_repo() {
        let output = b"worktree /path/to/repo.git\0HEAD abc123\0bare\0\0";
        let worktrees = WorktreeInfo::parse_porcelain_list_z(output).unwrap();
        let [wt]: [WorktreeInfo; 1] = worktrees.try_into().unwrap();
        assert!(wt.bare);
    }

    #[test]
    fn test_parse_porcelain_list_detached() {
        let output = b"worktree /path/to/repo\0HEAD abc123\0detached\0\0";
        let worktrees = WorktreeInfo::parse_porcelain_list_z(output).unwrap();
        let [wt]: [WorktreeInfo; 1] = worktrees.try_into().unwrap();
        assert!(wt.detached);
        assert!(wt.branch.is_none());
    }

    #[test]
    fn test_parse_porcelain_list_locked() {
        let output = b"worktree /path/to/repo\0HEAD abc123\0branch refs/heads/main\0locked reason for lock\0\0";
        let worktrees = WorktreeInfo::parse_porcelain_list_z(output).unwrap();
        let [wt]: [WorktreeInfo; 1] = worktrees.try_into().unwrap();
        assert_eq!(wt.locked, Some("reason for lock".to_string()));
    }

    #[test]
    fn test_parse_porcelain_list_prunable() {
        let output = b"worktree /path/to/repo\0HEAD abc123\0branch refs/heads/main\0prunable gitdir file missing\0\0";
        let worktrees = WorktreeInfo::parse_porcelain_list_z(output).unwrap();
        let [wt]: [WorktreeInfo; 1] = worktrees.try_into().unwrap();
        assert_eq!(wt.prunable, Some("gitdir file missing".to_string()));
    }

    #[test]
    fn test_parse_porcelain_list_empty() {
        let result = WorktreeInfo::parse_porcelain_list_z(b"");
        assert!(result.is_ok());
        let worktrees = result.unwrap();
        assert!(worktrees.is_empty());
    }

    #[test]
    fn test_parse_porcelain_list_no_trailing_blank() {
        // Git output may not always end with a blank line
        let output = b"worktree /path/to/repo\0HEAD abc123\0branch refs/heads/main";
        let result = WorktreeInfo::parse_porcelain_list_z(output);
        assert!(result.is_ok());
        let worktrees = result.unwrap();
        assert_eq!(worktrees.len(), 1);
    }

    #[test]
    fn test_parse_porcelain_list_missing_worktree_path() {
        let output = b"worktree\0HEAD abc123\0\0";
        let result = WorktreeInfo::parse_porcelain_list_z(output);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_porcelain_list_missing_head_sha() {
        let output = b"worktree /path\0HEAD\0\0";
        let result = WorktreeInfo::parse_porcelain_list_z(output);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_porcelain_list_branch_without_refs_prefix() {
        // This can happen in some edge cases
        let output = b"worktree /path/to/repo\0HEAD abc123\0branch main\0\0";
        let worktrees = WorktreeInfo::parse_porcelain_list_z(output).unwrap();
        let [wt]: [WorktreeInfo; 1] = worktrees.try_into().unwrap();
        // Should use the branch name as-is when no refs/heads/ prefix
        assert_eq!(wt.branch, Some("main".to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn test_parse_porcelain_list_z_preserves_non_utf8_path() {
        use std::os::unix::ffi::OsStrExt;

        let output = b"worktree /path/to/linked-\xff\0HEAD abc123\0branch refs/heads/feature\0\0";
        let worktrees = WorktreeInfo::parse_porcelain_list_z(output).unwrap();
        let [wt]: [WorktreeInfo; 1] = worktrees.try_into().unwrap();

        assert_eq!(wt.path.as_os_str().as_bytes(), b"/path/to/linked-\xff");
        assert_eq!(wt.branch.as_deref(), Some("feature"));
    }

    // ============================================================================
    // parse_porcelain_z Tests
    // ============================================================================

    #[test]
    fn test_parse_porcelain_z_empty() {
        assert!(parse_porcelain_z(b"").is_empty());
    }

    #[test]
    fn test_parse_porcelain_z_modified_file() {
        // "M  src/main.rs\0"
        let output = " M src/main.rs\0";
        let files = parse_porcelain_z(output.as_bytes());
        assert_eq!(files, vec![b"src/main.rs".to_vec()]);
    }

    #[test]
    fn test_parse_porcelain_z_multiple_files() {
        let output = " M src/main.rs\0?? new_file.txt\0";
        let files = parse_porcelain_z(output.as_bytes());
        assert_eq!(
            files,
            vec![b"src/main.rs".to_vec(), b"new_file.txt".to_vec()]
        );
    }

    #[test]
    fn test_parse_porcelain_z_rename() {
        // Renames: "R  new_name\0old_name\0"
        let output = "R  new_name.rs\0old_name.rs\0";
        let files = parse_porcelain_z(output.as_bytes());
        assert_eq!(
            files,
            vec![b"new_name.rs".to_vec(), b"old_name.rs".to_vec()]
        );
    }

    #[test]
    fn test_parse_porcelain_z_copy() {
        let output = "C  copy.rs\0original.rs\0";
        let files = parse_porcelain_z(output.as_bytes());
        assert_eq!(files, vec![b"copy.rs".to_vec(), b"original.rs".to_vec()]);
    }

    #[test]
    fn test_parse_porcelain_z_rename_among_others() {
        let output = " M keep.rs\0R  new.rs\0old.rs\0?? untracked.txt\0";
        let files = parse_porcelain_z(output.as_bytes());
        assert_eq!(
            files,
            vec![
                b"keep.rs".to_vec(),
                b"new.rs".to_vec(),
                b"old.rs".to_vec(),
                b"untracked.txt".to_vec()
            ]
        );
    }

    #[test]
    fn test_parse_porcelain_z_worktree_rename() {
        let output = " R new.rs\0old.rs\0?? untracked.txt\0";
        let files = parse_porcelain_z(output.as_bytes());
        assert_eq!(
            files,
            vec![
                b"new.rs".to_vec(),
                b"old.rs".to_vec(),
                b"untracked.txt".to_vec()
            ]
        );
    }

    #[test]
    fn test_parse_porcelain_z_spaces_in_path() {
        let output = " M path with spaces/file name.rs\0";
        let files = parse_porcelain_z(output.as_bytes());
        assert_eq!(files, vec![b"path with spaces/file name.rs".to_vec()]);
    }

    #[test]
    fn test_parse_porcelain_z_skips_short_entries() {
        // Entries shorter than 3 chars (status + space + path) are skipped
        let output = " M valid.rs\0ab\0";
        let files = parse_porcelain_z(output.as_bytes());
        assert_eq!(files, vec![b"valid.rs".to_vec()]);
    }

    #[test]
    fn test_parse_porcelain_z_preserves_distinct_non_utf8_paths() {
        let output = b" D collision-\xfe\0 D collision-\xff\0";
        let files = parse_porcelain_z(output);
        assert_eq!(
            files,
            vec![b"collision-\xfe".to_vec(), b"collision-\xff".to_vec()]
        );
    }

    // ============================================================================
    // parse_untracked_files Tests
    // ============================================================================

    #[test]
    fn test_parse_untracked_files_empty() {
        assert!(parse_untracked_files("").is_empty());
    }

    #[test]
    fn test_parse_untracked_files_only_untracked() {
        let output = "?? new_file.txt\0?? another.rs\0";
        let files = parse_untracked_files(output);
        assert_eq!(files, vec!["new_file.txt", "another.rs"]);
    }

    #[test]
    fn test_parse_untracked_files_filters_tracked() {
        let output = " M modified.rs\0?? untracked.txt\0A  added.rs\0";
        let files = parse_untracked_files(output);
        assert_eq!(files, vec!["untracked.txt"]);
    }

    #[test]
    fn test_parse_untracked_files_skips_rename_old_path() {
        // Rename entry has an extra NUL-separated old path that must be skipped
        let output = "R  new.rs\0old.rs\0?? untracked.txt\0";
        let files = parse_untracked_files(output);
        assert_eq!(files, vec!["untracked.txt"]);
    }

    #[test]
    fn test_parse_untracked_files_skips_worktree_rename_old_path() {
        // A rename/copy in either status column has an extra old-path field.
        // Make that old path look like another rename so a parser that treats
        // it as a status entry will consume the real untracked record.
        let output = " R new.rs\0R  old.rs\0?? untracked.txt\0";
        let files = parse_untracked_files(output);
        assert_eq!(files, vec!["untracked.txt"]);
    }

    #[test]
    fn test_parse_untracked_files_no_untracked() {
        let output = " M modified.rs\0A  added.rs\0";
        let files = parse_untracked_files(output);
        assert!(files.is_empty());
    }

    #[test]
    fn test_parse_untracked_files_spaces_in_path() {
        let output = "?? path with spaces/new file.txt\0";
        let files = parse_untracked_files(output);
        assert_eq!(files, vec!["path with spaces/new file.txt"]);
    }
}
