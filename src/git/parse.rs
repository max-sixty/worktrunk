//! Git output parsing functions

use std::path::PathBuf;

use super::{GitError, Worktree, finalize_worktree};

impl Worktree {
    pub(crate) fn parse_porcelain_list(output: &str) -> anyhow::Result<Vec<Self>> {
        let mut worktrees = Vec::new();
        let mut current: Option<Worktree> = None;

        for line in output.lines() {
            if line.is_empty() {
                if let Some(wt) = current.take() {
                    worktrees.push(finalize_worktree(wt));
                }
                continue;
            }

            let (key, value) = match line.split_once(' ') {
                Some((k, v)) => (k, Some(v)),
                None => (line, None),
            };

            match key {
                "worktree" => {
                    let Some(path) = value else {
                        return Err(GitError::ParseError {
                            message: "worktree line missing path".into(),
                        }
                        .into());
                    };
                    current = Some(Worktree {
                        path: PathBuf::from(path),
                        head: String::new(),
                        branch: None,
                        bare: false,
                        detached: false,
                        locked: None,
                        prunable: None,
                    });
                }
                key => match (key, current.as_mut()) {
                    ("HEAD", Some(wt)) => {
                        let Some(sha) = value else {
                            return Err(GitError::ParseError {
                                message: "HEAD line missing SHA".into(),
                            }
                            .into());
                        };
                        wt.head = sha.to_string();
                    }
                    ("branch", Some(wt)) => {
                        // Strip refs/heads/ prefix if present
                        let Some(branch_ref) = value else {
                            return Err(GitError::ParseError {
                                message: "branch line missing ref".into(),
                            }
                            .into());
                        };
                        let branch = branch_ref
                            .strip_prefix("refs/heads/")
                            .unwrap_or(branch_ref)
                            .to_string();
                        wt.branch = Some(branch);
                    }
                    ("bare", Some(wt)) => {
                        wt.bare = true;
                    }
                    ("detached", Some(wt)) => {
                        wt.detached = true;
                    }
                    ("locked", Some(wt)) => {
                        wt.locked = Some(value.unwrap_or_default().to_string());
                    }
                    ("prunable", Some(wt)) => {
                        wt.prunable = Some(value.unwrap_or_default().to_string());
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
    // Worktree::parse_porcelain_list Tests
    // ============================================================================

    #[test]
    fn test_parse_porcelain_list_single_worktree() {
        let output = "worktree /path/to/repo\nHEAD abc123\nbranch refs/heads/main\n\n";
        let result = Worktree::parse_porcelain_list(output);
        assert!(result.is_ok());
        let worktrees = result.unwrap();
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].path.to_str().unwrap(), "/path/to/repo");
        assert_eq!(worktrees[0].head, "abc123");
        assert_eq!(worktrees[0].branch, Some("main".to_string()));
    }

    #[test]
    fn test_parse_porcelain_list_multiple_worktrees() {
        let output = "worktree /path/main\nHEAD aaa\nbranch refs/heads/main\n\nworktree /path/feature\nHEAD bbb\nbranch refs/heads/feature\n\n";
        let result = Worktree::parse_porcelain_list(output);
        assert!(result.is_ok());
        let worktrees = result.unwrap();
        assert_eq!(worktrees.len(), 2);
        assert_eq!(worktrees[0].branch, Some("main".to_string()));
        assert_eq!(worktrees[1].branch, Some("feature".to_string()));
    }

    #[test]
    fn test_parse_porcelain_list_bare_repo() {
        let output = "worktree /path/to/repo.git\nHEAD abc123\nbare\n\n";
        let result = Worktree::parse_porcelain_list(output);
        assert!(result.is_ok());
        let worktrees = result.unwrap();
        assert_eq!(worktrees.len(), 1);
        assert!(worktrees[0].bare);
    }

    #[test]
    fn test_parse_porcelain_list_detached() {
        let output = "worktree /path/to/repo\nHEAD abc123\ndetached\n\n";
        let result = Worktree::parse_porcelain_list(output);
        assert!(result.is_ok());
        let worktrees = result.unwrap();
        assert_eq!(worktrees.len(), 1);
        assert!(worktrees[0].detached);
        assert!(worktrees[0].branch.is_none());
    }

    #[test]
    fn test_parse_porcelain_list_locked() {
        let output = "worktree /path/to/repo\nHEAD abc123\nbranch refs/heads/main\nlocked reason for lock\n\n";
        let result = Worktree::parse_porcelain_list(output);
        assert!(result.is_ok());
        let worktrees = result.unwrap();
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].locked, Some("reason for lock".to_string()));
    }

    #[test]
    fn test_parse_porcelain_list_prunable() {
        let output = "worktree /path/to/repo\nHEAD abc123\nbranch refs/heads/main\nprunable gitdir file missing\n\n";
        let result = Worktree::parse_porcelain_list(output);
        assert!(result.is_ok());
        let worktrees = result.unwrap();
        assert_eq!(worktrees.len(), 1);
        assert_eq!(
            worktrees[0].prunable,
            Some("gitdir file missing".to_string())
        );
    }

    #[test]
    fn test_parse_porcelain_list_empty() {
        let result = Worktree::parse_porcelain_list("");
        assert!(result.is_ok());
        let worktrees = result.unwrap();
        assert!(worktrees.is_empty());
    }

    #[test]
    fn test_parse_porcelain_list_no_trailing_blank() {
        // Git output may not always end with a blank line
        let output = "worktree /path/to/repo\nHEAD abc123\nbranch refs/heads/main";
        let result = Worktree::parse_porcelain_list(output);
        assert!(result.is_ok());
        let worktrees = result.unwrap();
        assert_eq!(worktrees.len(), 1);
    }

    #[test]
    fn test_parse_porcelain_list_missing_worktree_path() {
        let output = "worktree\nHEAD abc123\n\n";
        let result = Worktree::parse_porcelain_list(output);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_porcelain_list_missing_head_sha() {
        let output = "worktree /path\nHEAD\n\n";
        let result = Worktree::parse_porcelain_list(output);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_porcelain_list_branch_without_refs_prefix() {
        // This can happen in some edge cases
        let output = "worktree /path/to/repo\nHEAD abc123\nbranch main\n\n";
        let result = Worktree::parse_porcelain_list(output);
        assert!(result.is_ok());
        let worktrees = result.unwrap();
        // Should use the branch name as-is when no refs/heads/ prefix
        assert_eq!(worktrees[0].branch, Some("main".to_string()));
    }
}
