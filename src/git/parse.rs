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
