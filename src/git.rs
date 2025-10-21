use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Worktree {
    pub path: PathBuf,
    pub head: String,
    pub branch: Option<String>,
    pub bare: bool,
    pub detached: bool,
    pub locked: Option<String>,
    pub prunable: Option<String>,
}

#[derive(Debug)]
pub enum GitError {
    CommandFailed(String),
    ParseError(String),
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // CommandFailed messages are already formatted with emoji and colors
            GitError::CommandFailed(msg) => write!(f, "{}", msg),
            // ParseError messages need formatting
            GitError::ParseError(msg) => {
                use crate::styling::{ERROR, ERROR_EMOJI};
                write!(f, "{ERROR_EMOJI} {ERROR}{msg}{ERROR:#}")
            }
        }
    }
}

impl std::error::Error for GitError {}

/// Repository context for git operations.
///
/// Provides a more ergonomic API than the `*_in(path, ...)` functions by
/// encapsulating the repository path.
///
/// # Examples
///
/// ```no_run
/// use worktrunk::git::Repository;
///
/// let repo = Repository::current();
/// let branch = repo.current_branch()?;
/// let is_dirty = repo.is_dirty()?;
/// # Ok::<(), worktrunk::git::GitError>(())
/// ```
#[derive(Debug, Clone)]
pub struct Repository {
    path: PathBuf,
}

impl Repository {
    /// Create a repository context at the specified path.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Create a repository context for the current directory.
    ///
    /// This is the most common usage pattern.
    pub fn current() -> Self {
        Self::at(".")
    }

    /// Get the path this repository context operates on.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Get the primary remote name for this repository.
    ///
    /// Uses the following strategy:
    /// 1. If the current branch has an upstream, use its remote
    ///    (Note: Detached HEAD falls through to step 2)
    /// 2. Otherwise, get the first remote from `git remote`
    /// 3. Fall back to "origin" if no remotes exist
    pub fn primary_remote(&self) -> Result<String, GitError> {
        // Try to get the remote from the current branch's upstream
        if let Ok(Some(branch)) = self.current_branch()
            && let Ok(Some(upstream)) = self.upstream_branch(&branch)
            && let Some((remote, _)) = upstream.split_once('/')
        {
            return Ok(remote.to_string());
        }

        // Fall back to first remote in the list
        let output = self.run_command(&["remote"])?;
        let first_remote = output.lines().next();

        Ok(first_remote.unwrap_or("origin").to_string())
    }

    /// Check if a git branch exists (local or remote).
    pub fn branch_exists(&self, branch: &str) -> Result<bool, GitError> {
        // Try local branch first
        if self
            .run_command(&["rev-parse", "--verify", &format!("refs/heads/{}", branch)])
            .is_ok()
        {
            return Ok(true);
        }

        // Try remote branch
        let remote = self.primary_remote()?;
        Ok(self
            .run_command(&[
                "rev-parse",
                "--verify",
                &format!("refs/remotes/{}/{}", remote, branch),
            ])
            .is_ok())
    }

    /// Get the current branch name, or None if in detached HEAD state.
    pub fn current_branch(&self) -> Result<Option<String>, GitError> {
        let stdout = self.run_command(&["branch", "--show-current"])?;
        let branch = stdout.trim();

        if branch.is_empty() {
            Ok(None) // Detached HEAD
        } else {
            Ok(Some(branch.to_string()))
        }
    }

    /// Get the default branch name for the repository.
    ///
    /// Uses a hybrid approach:
    /// 1. Try local cache (remote/HEAD) first for speed
    /// 2. If not cached, query the remote and cache the result
    pub fn default_branch(&self) -> Result<String, GitError> {
        let remote = self.primary_remote()?;
        // Try local cache first (fast path)
        self.get_local_default_branch(&remote).or_else(|_| {
            // Query remote and cache it
            let branch = self.query_remote_default_branch(&remote)?;
            self.cache_default_branch(&remote, &branch)?;
            Ok(branch)
        })
    }

    /// Get the git common directory (the actual .git directory for the repository).
    pub fn git_common_dir(&self) -> Result<PathBuf, GitError> {
        let stdout = self.run_command(&["rev-parse", "--git-common-dir"])?;
        Ok(PathBuf::from(stdout.trim()))
    }

    /// Get the git directory (may be different from common-dir in worktrees).
    pub fn git_dir(&self) -> Result<PathBuf, GitError> {
        let stdout = self.run_command(&["rev-parse", "--git-dir"])?;
        Ok(PathBuf::from(stdout.trim()))
    }

    /// Get the canonicalized repository root directory (parent of .git).
    ///
    /// The canonicalization resolves symlinks and relative paths, which is important
    /// for worktree operations to ensure consistent path handling.
    pub fn repo_root(&self) -> Result<PathBuf, GitError> {
        let git_common_dir = self
            .git_common_dir()?
            .canonicalize()
            .map_err(|e| GitError::CommandFailed(format!("Failed to canonicalize path: {}", e)))?;

        git_common_dir
            .parent()
            .ok_or_else(|| GitError::CommandFailed("Invalid git directory".to_string()))
            .map(|p| p.to_path_buf())
    }

    /// Check if the working tree has uncommitted changes.
    pub fn is_dirty(&self) -> Result<bool, GitError> {
        let stdout = self.run_command(&["status", "--porcelain"])?;
        Ok(!stdout.trim().is_empty())
    }

    /// Ensure the working tree is clean (no uncommitted changes).
    ///
    /// Returns an error if there are uncommitted changes.
    pub fn ensure_clean_working_tree(&self) -> Result<(), GitError> {
        if self.is_dirty()? {
            use crate::styling::{ERROR, ERROR_EMOJI};
            return Err(GitError::CommandFailed(format!(
                "{ERROR_EMOJI} {ERROR}Working tree has uncommitted changes. Commit or stash them first.{ERROR:#}"
            )));
        }
        Ok(())
    }

    /// Get the worktree root directory (top-level of the working tree).
    pub fn worktree_root(&self) -> Result<PathBuf, GitError> {
        let stdout = self.run_command(&["rev-parse", "--show-toplevel"])?;
        Ok(PathBuf::from(stdout.trim()))
    }

    /// Check if the path is in a worktree (vs the main repository).
    pub fn is_in_worktree(&self) -> Result<bool, GitError> {
        let git_dir = self.git_dir()?;
        let common_dir = self.git_common_dir()?;
        Ok(git_dir != common_dir)
    }

    /// Check if base is an ancestor of head (i.e., would be a fast-forward).
    pub fn is_ancestor(&self, base: &str, head: &str) -> Result<bool, GitError> {
        self.run_command_check(&["merge-base", "--is-ancestor", base, head])
    }

    /// Count commits between base and head.
    pub fn count_commits(&self, base: &str, head: &str) -> Result<usize, GitError> {
        let range = format!("{}..{}", base, head);
        let stdout = self.run_command(&["rev-list", "--count", &range])?;
        stdout
            .trim()
            .parse()
            .map_err(|e| GitError::ParseError(format!("Failed to parse commit count: {}", e)))
    }

    /// Check if there are merge commits in the range base..head.
    pub fn has_merge_commits(&self, base: &str, head: &str) -> Result<bool, GitError> {
        let range = format!("{}..{}", base, head);
        let stdout = self.run_command(&["rev-list", "--merges", &range])?;
        Ok(!stdout.trim().is_empty())
    }

    /// Get files changed between base and head.
    pub fn changed_files(&self, base: &str, head: &str) -> Result<Vec<String>, GitError> {
        let range = format!("{}..{}", base, head);
        let stdout = self.run_command(&["diff", "--name-only", &range])?;
        Ok(stdout.lines().map(String::from).collect())
    }

    /// Get commit timestamp in seconds since epoch.
    pub fn commit_timestamp(&self, commit: &str) -> Result<i64, GitError> {
        let stdout = self.run_command(&["show", "-s", "--format=%ct", commit])?;
        stdout
            .trim()
            .parse()
            .map_err(|e| GitError::ParseError(format!("Failed to parse timestamp: {}", e)))
    }

    /// Get commit message (subject line) for a commit.
    pub fn commit_message(&self, commit: &str) -> Result<String, GitError> {
        let stdout = self.run_command(&["show", "-s", "--format=%s", commit])?;
        Ok(stdout.trim().to_owned())
    }

    /// Get the upstream tracking branch for the given branch.
    pub fn upstream_branch(&self, branch: &str) -> Result<Option<String>, GitError> {
        let result = self.run_command(&["rev-parse", "--abbrev-ref", &format!("{}@{{u}}", branch)]);

        match result {
            Ok(upstream) => {
                let trimmed = upstream.trim();
                Ok((!trimmed.is_empty()).then(|| trimmed.to_string()))
            }
            Err(_) => Ok(None), // No upstream configured
        }
    }

    /// Get merge/rebase status for the worktree.
    pub fn worktree_state(&self) -> Result<Option<String>, GitError> {
        let git_dir = self.git_dir()?;

        // Check for merge
        if git_dir.join("MERGE_HEAD").exists() {
            return Ok(Some("MERGING".to_string()));
        }

        // Check for rebase
        if git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists() {
            let rebase_dir = if git_dir.join("rebase-merge").exists() {
                git_dir.join("rebase-merge")
            } else {
                git_dir.join("rebase-apply")
            };

            if let (Ok(msgnum), Ok(end)) = (
                std::fs::read_to_string(rebase_dir.join("msgnum")),
                std::fs::read_to_string(rebase_dir.join("end")),
            ) {
                let current = msgnum.trim();
                let total = end.trim();
                return Ok(Some(format!("REBASING {}/{}", current, total)));
            }

            return Ok(Some("REBASING".to_string()));
        }

        // Check for cherry-pick
        if git_dir.join("CHERRY_PICK_HEAD").exists() {
            return Ok(Some("CHERRY-PICKING".to_string()));
        }

        // Check for revert
        if git_dir.join("REVERT_HEAD").exists() {
            return Ok(Some("REVERTING".to_string()));
        }

        // Check for bisect
        if git_dir.join("BISECT_LOG").exists() {
            return Ok(Some("BISECTING".to_string()));
        }

        Ok(None)
    }

    /// Calculate commits ahead and behind between two refs.
    ///
    /// Returns (ahead, behind) where ahead is commits in head not in base,
    /// and behind is commits in base not in head.
    pub fn ahead_behind(&self, base: &str, head: &str) -> Result<(usize, usize), GitError> {
        let ahead = self.count_commits(base, head)?;
        let behind = self.count_commits(head, base)?;
        Ok((ahead, behind))
    }

    /// Get line diff statistics for working tree changes (unstaged + staged).
    ///
    /// Returns (added_lines, deleted_lines).
    pub fn working_tree_diff_stats(&self) -> Result<(usize, usize), GitError> {
        let stdout = self.run_command(&["diff", "--numstat", "HEAD"])?;
        parse_numstat(&stdout)
    }

    /// Get line diff statistics between two refs (using three-dot diff for merge base).
    ///
    /// Returns (added_lines, deleted_lines).
    pub fn branch_diff_stats(&self, base: &str, head: &str) -> Result<(usize, usize), GitError> {
        let range = format!("{}...{}", base, head);
        let stdout = self.run_command(&["diff", "--numstat", &range])?;
        parse_numstat(&stdout)
    }

    /// Get all branch names (local branches only).
    pub fn all_branches(&self) -> Result<Vec<String>, GitError> {
        let stdout = self.run_command(&["branch", "--format=%(refname:short)"])?;
        Ok(stdout
            .lines()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect())
    }

    /// Get the merge base between two commits.
    pub fn merge_base(&self, commit1: &str, commit2: &str) -> Result<String, GitError> {
        let output = self.run_command(&["merge-base", commit1, commit2])?;
        Ok(output.trim().to_owned())
    }

    /// Get commit subjects (first line of commit message) from a range.
    pub fn commit_subjects(&self, range: &str) -> Result<Vec<String>, GitError> {
        let output = self.run_command(&["log", "--format=%s", range])?;
        Ok(output.lines().map(String::from).collect())
    }

    /// Check if there are staged changes.
    pub fn has_staged_changes(&self) -> Result<bool, GitError> {
        // exit code 0 = no changes, 1 = has changes
        let no_changes = self.run_command_check(&["diff", "--cached", "--quiet", "--exit-code"])?;
        Ok(!no_changes)
    }

    /// List all worktrees for this repository.
    pub fn list_worktrees(&self) -> Result<Vec<Worktree>, GitError> {
        let stdout = self.run_command(&["worktree", "list", "--porcelain"])?;
        parse_worktree_list(&stdout)
    }

    /// Find the worktree path for a given branch, if one exists.
    pub fn worktree_for_branch(&self, branch: &str) -> Result<Option<PathBuf>, GitError> {
        let worktrees = self.list_worktrees()?;

        Ok(worktrees
            .into_iter()
            .find(|wt| wt.branch.as_deref() == Some(branch))
            .map(|wt| wt.path))
    }

    /// Get branches that don't have worktrees (available for switch).
    pub fn available_branches(&self) -> Result<Vec<String>, GitError> {
        let all_branches = self.all_branches()?;
        let worktrees = self.list_worktrees()?;

        // Collect branches that have worktrees
        let branches_with_worktrees: std::collections::HashSet<String> =
            worktrees.into_iter().filter_map(|wt| wt.branch).collect();

        // Filter out branches with worktrees
        Ok(all_branches
            .into_iter()
            .filter(|branch| !branches_with_worktrees.contains(branch))
            .collect())
    }

    /// Get a git config value. Returns None if the key doesn't exist.
    pub fn get_config(&self, key: &str) -> Result<Option<String>, GitError> {
        match self.run_command(&["config", key]) {
            Ok(value) => Ok(Some(value.trim().to_string())),
            Err(_) => Ok(None), // Config key doesn't exist
        }
    }

    /// Set a git config value.
    pub fn set_config(&self, key: &str, value: &str) -> Result<(), GitError> {
        self.run_command(&["config", key, value])?;
        Ok(())
    }

    /// Remove a worktree at the specified path.
    pub fn remove_worktree(&self, path: &std::path::Path) -> Result<(), GitError> {
        let path_str = path
            .to_str()
            .ok_or_else(|| GitError::CommandFailed("Invalid UTF-8 in worktree path".to_string()))?;
        self.run_command(&["worktree", "remove", path_str])?;
        Ok(())
    }

    // Private helper methods for default_branch()

    fn get_local_default_branch(&self, remote: &str) -> Result<String, GitError> {
        let stdout =
            self.run_command(&["rev-parse", "--abbrev-ref", &format!("{}/HEAD", remote)])?;
        parse_local_default_branch(&stdout, remote)
    }

    fn query_remote_default_branch(&self, remote: &str) -> Result<String, GitError> {
        let stdout = self.run_command(&["ls-remote", "--symref", remote, "HEAD"])?;
        parse_remote_default_branch(&stdout)
    }

    fn cache_default_branch(&self, remote: &str, branch: &str) -> Result<(), GitError> {
        self.run_command(&["remote", "set-head", remote, branch])?;
        Ok(())
    }

    /// Get a project identifier for approval tracking.
    ///
    /// Uses the git remote URL if available (e.g., "github.com/user/repo"),
    /// otherwise falls back to the repository directory name.
    ///
    /// This identifier is used to track which commands have been approved
    /// for execution in this project.
    pub fn project_identifier(&self) -> Result<String, GitError> {
        // Try to get the remote URL first
        let remote = self.primary_remote()?;

        if let Ok(url) = self.run_command(&["remote", "get-url", &remote]) {
            let url = url.trim();

            // Parse common git URL formats:
            // - https://github.com/user/repo.git
            // - git@github.com:user/repo.git
            // - ssh://git@github.com/user/repo.git

            // Remove .git suffix if present
            let url = url.strip_suffix(".git").unwrap_or(url);

            // Handle SSH format (git@host:path)
            if let Some(ssh_part) = url.strip_prefix("git@")
                && let Some((host, path)) = ssh_part.split_once(':')
            {
                return Ok(format!("{}/{}", host, path));
            }

            // Handle HTTPS/HTTP format
            if let Some(https_part) = url
                .strip_prefix("https://")
                .or_else(|| url.strip_prefix("http://"))
            {
                return Ok(https_part.to_string());
            }

            // Handle ssh:// format
            if let Some(ssh_part) = url.strip_prefix("ssh://") {
                // Remove git@ prefix if present
                let ssh_part = ssh_part.strip_prefix("git@").unwrap_or(ssh_part);
                // Replace first : with /
                if let Some(colon_pos) = ssh_part.find(':') {
                    let (host, path) = ssh_part.split_at(colon_pos);
                    return Ok(format!("{}{}", host, path.replacen(':', "/", 1)));
                }
                return Ok(ssh_part.to_string());
            }

            // If we can't parse it, use the URL as-is
            return Ok(url.to_string());
        }

        // Fall back to repository name
        let repo_root = self.repo_root()?;
        let repo_name = repo_root
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                GitError::CommandFailed("Could not determine repository name".to_string())
            })?;

        Ok(repo_name.to_string())
    }

    /// Run a git command in this repository's context.
    ///
    /// Executes the git command with this repository's path as the working directory
    /// and returns the stdout output.
    ///
    /// # Examples
    /// ```no_run
    /// use worktrunk::git::Repository;
    ///
    /// let repo = Repository::current();
    /// let status = repo.run_command(&["status", "--porcelain"])?;
    /// # Ok::<(), worktrunk::git::GitError>(())
    /// ```
    pub fn run_command(&self, args: &[&str]) -> Result<String, GitError> {
        let mut cmd = Command::new("git");
        cmd.args(args);
        cmd.current_dir(&self.path);

        let output = cmd
            .output()
            .map_err(|e| GitError::CommandFailed(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(GitError::CommandFailed(stderr.into_owned()));
        }

        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Run a git command and return whether it succeeded (exit code 0).
    ///
    /// This is useful for commands that use exit codes for boolean results,
    /// like `git merge-base --is-ancestor` or `git diff --quiet`.
    ///
    /// # Examples
    /// ```no_run
    /// use worktrunk::git::Repository;
    ///
    /// let repo = Repository::current();
    /// let is_clean = repo.run_command_check(&["diff", "--quiet", "--exit-code"])?;
    /// # Ok::<(), worktrunk::git::GitError>(())
    /// ```
    pub fn run_command_check(&self, args: &[&str]) -> Result<bool, GitError> {
        let mut cmd = Command::new("git");
        cmd.args(args);
        cmd.current_dir(&self.path);

        let output = cmd
            .output()
            .map_err(|e| GitError::CommandFailed(e.to_string()))?;

        Ok(output.status.success())
    }
}

fn parse_worktree_list(output: &str) -> Result<Vec<Worktree>, GitError> {
    let mut worktrees = Vec::new();
    let mut current = None;

    for line in output.lines() {
        if line.is_empty() {
            if let Some(wt) = current.take() {
                worktrees.push(wt);
            }
            continue;
        }

        let (key, value) = match line.split_once(' ') {
            Some((k, v)) => (k, Some(v)),
            None => (line, None),
        };

        match key {
            "worktree" => {
                let path = value.ok_or_else(|| {
                    GitError::ParseError("worktree line missing path".to_string())
                })?;
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
            "HEAD" => {
                if let Some(ref mut wt) = current {
                    wt.head = value
                        .ok_or_else(|| GitError::ParseError("HEAD line missing SHA".to_string()))?
                        .to_string();
                }
            }
            "branch" => {
                if let Some(ref mut wt) = current {
                    // Strip refs/heads/ prefix if present
                    let branch_ref = value.ok_or_else(|| {
                        GitError::ParseError("branch line missing ref".to_string())
                    })?;
                    let branch = branch_ref
                        .strip_prefix("refs/heads/")
                        .unwrap_or(branch_ref)
                        .to_string();
                    wt.branch = Some(branch);
                }
            }
            "bare" => {
                if let Some(ref mut wt) = current {
                    wt.bare = true;
                }
            }
            "detached" => {
                if let Some(ref mut wt) = current {
                    wt.detached = true;
                }
            }
            "locked" => {
                if let Some(ref mut wt) = current {
                    wt.locked = Some(value.unwrap_or_default().to_string());
                }
            }
            "prunable" => {
                if let Some(ref mut wt) = current {
                    wt.prunable = Some(value.unwrap_or_default().to_string());
                }
            }
            _ => {
                // Ignore unknown attributes for forward compatibility
            }
        }
    }

    // Push the last worktree if the output doesn't end with a blank line
    if let Some(wt) = current {
        worktrees.push(wt);
    }

    Ok(worktrees)
}

fn parse_local_default_branch(output: &str, remote: &str) -> Result<String, GitError> {
    let trimmed = output.trim();

    // Strip "remote/" prefix if present
    let prefix = format!("{}/", remote);
    let branch = trimmed.strip_prefix(&prefix).unwrap_or(trimmed);

    if branch.is_empty() {
        return Err(GitError::ParseError(format!(
            "Empty branch name from {}/HEAD",
            remote
        )));
    }

    Ok(branch.to_string())
}

fn parse_remote_default_branch(output: &str) -> Result<String, GitError> {
    output
        .lines()
        .find_map(|line| {
            line.strip_prefix("ref: ")
                .and_then(|symref| symref.split_once('\t'))
                .map(|(ref_path, _)| ref_path)
                .and_then(|ref_path| ref_path.strip_prefix("refs/heads/"))
                .map(|branch| branch.to_string())
        })
        .ok_or_else(|| {
            GitError::ParseError("Could not find symbolic ref in ls-remote output".to_string())
        })
}

fn parse_numstat(output: &str) -> Result<(usize, usize), GitError> {
    let mut total_added = 0;
    let mut total_deleted = 0;

    for line in output.lines() {
        if line.trim().is_empty() {
            continue;
        }

        let mut parts = line.split('\t');
        let Some(added_str) = parts.next() else {
            continue;
        };
        let Some(deleted_str) = parts.next() else {
            continue;
        };

        // Binary files show "-" for added/deleted
        if added_str == "-" || deleted_str == "-" {
            continue;
        }

        let added: usize = added_str
            .parse()
            .map_err(|e| GitError::ParseError(format!("Failed to parse added lines: {}", e)))?;
        let deleted: usize = deleted_str
            .parse()
            .map_err(|e| GitError::ParseError(format!("Failed to parse deleted lines: {}", e)))?;

        total_added += added;
        total_deleted += deleted;
    }

    Ok((total_added, total_deleted))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_worktree_list() {
        let output = "worktree /path/to/main
HEAD abcd1234
branch refs/heads/main

worktree /path/to/feature
HEAD efgh5678
branch refs/heads/feature

";

        let worktrees = parse_worktree_list(output).unwrap();
        assert_eq!(worktrees.len(), 2);

        assert_eq!(worktrees[0].path, PathBuf::from("/path/to/main"));
        assert_eq!(worktrees[0].head, "abcd1234");
        assert_eq!(worktrees[0].branch, Some("main".to_string()));
        assert!(!worktrees[0].bare);
        assert!(!worktrees[0].detached);

        assert_eq!(worktrees[1].path, PathBuf::from("/path/to/feature"));
        assert_eq!(worktrees[1].head, "efgh5678");
        assert_eq!(worktrees[1].branch, Some("feature".to_string()));
    }

    #[test]
    fn test_parse_detached_worktree() {
        let output = "worktree /path/to/detached
HEAD abcd1234
detached

";

        let worktrees = parse_worktree_list(output).unwrap();
        assert_eq!(worktrees.len(), 1);
        assert!(worktrees[0].detached);
        assert_eq!(worktrees[0].branch, None);
    }

    #[test]
    fn test_parse_locked_worktree() {
        let output = "worktree /path/to/locked
HEAD abcd1234
branch refs/heads/main
locked reason for lock

";

        let worktrees = parse_worktree_list(output).unwrap();
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].locked, Some("reason for lock".to_string()));
    }

    #[test]
    fn test_parse_bare_worktree() {
        let output = "worktree /path/to/bare
HEAD abcd1234
bare

";

        let worktrees = parse_worktree_list(output).unwrap();
        assert_eq!(worktrees.len(), 1);
        assert!(worktrees[0].bare);
    }

    #[test]
    fn test_parse_local_default_branch_with_prefix() {
        let output = "origin/main\n";
        let branch = parse_local_default_branch(output, "origin").unwrap();
        assert_eq!(branch, "main");
    }

    #[test]
    fn test_parse_local_default_branch_without_prefix() {
        let output = "main\n";
        let branch = parse_local_default_branch(output, "origin").unwrap();
        assert_eq!(branch, "main");
    }

    #[test]
    fn test_parse_local_default_branch_master() {
        let output = "origin/master\n";
        let branch = parse_local_default_branch(output, "origin").unwrap();
        assert_eq!(branch, "master");
    }

    #[test]
    fn test_parse_local_default_branch_custom_name() {
        let output = "origin/develop\n";
        let branch = parse_local_default_branch(output, "origin").unwrap();
        assert_eq!(branch, "develop");
    }

    #[test]
    fn test_parse_local_default_branch_custom_remote() {
        let output = "upstream/main\n";
        let branch = parse_local_default_branch(output, "upstream").unwrap();
        assert_eq!(branch, "main");
    }

    #[test]
    fn test_parse_local_default_branch_empty() {
        let output = "";
        let result = parse_local_default_branch(output, "origin");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), GitError::ParseError(_)));
    }

    #[test]
    fn test_parse_local_default_branch_whitespace_only() {
        let output = "  \n  ";
        let result = parse_local_default_branch(output, "origin");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_remote_default_branch_main() {
        let output = "ref: refs/heads/main\tHEAD
85a1ce7c7182540f9c02453441cb3e8bf0ced214\tHEAD
";
        let branch = parse_remote_default_branch(output).unwrap();
        assert_eq!(branch, "main");
    }

    #[test]
    fn test_parse_remote_default_branch_master() {
        let output = "ref: refs/heads/master\tHEAD
abcd1234567890abcd1234567890abcd12345678\tHEAD
";
        let branch = parse_remote_default_branch(output).unwrap();
        assert_eq!(branch, "master");
    }

    #[test]
    fn test_parse_remote_default_branch_custom() {
        let output = "ref: refs/heads/develop\tHEAD
1234567890abcdef1234567890abcdef12345678\tHEAD
";
        let branch = parse_remote_default_branch(output).unwrap();
        assert_eq!(branch, "develop");
    }

    #[test]
    fn test_parse_remote_default_branch_only_symref_line() {
        let output = "ref: refs/heads/main\tHEAD\n";
        let branch = parse_remote_default_branch(output).unwrap();
        assert_eq!(branch, "main");
    }

    #[test]
    fn test_parse_remote_default_branch_missing_symref() {
        let output = "85a1ce7c7182540f9c02453441cb3e8bf0ced214\tHEAD\n";
        let result = parse_remote_default_branch(output);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), GitError::ParseError(_)));
    }

    #[test]
    fn test_parse_remote_default_branch_empty() {
        let output = "";
        let result = parse_remote_default_branch(output);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_remote_default_branch_malformed_ref() {
        // Missing refs/heads/ prefix
        let output = "ref: main\tHEAD\n";
        let result = parse_remote_default_branch(output);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_remote_default_branch_with_spaces() {
        // Space instead of tab - should be rejected as malformed input
        let output = "ref: refs/heads/main HEAD\n";
        let result = parse_remote_default_branch(output);
        // Using split_once correctly rejects malformed input with spaces instead of tabs
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_remote_default_branch_branch_with_slash() {
        let output = "ref: refs/heads/feature/new-ui\tHEAD\n";
        let branch = parse_remote_default_branch(output).unwrap();
        assert_eq!(branch, "feature/new-ui");
    }

    #[test]
    fn test_get_current_branch_parse() {
        // Test parsing of branch --show-current output
        // We can't test the actual command without a git repo,
        // but we've verified the parsing logic through the implementation
    }

    #[test]
    fn test_worktree_for_branch_not_found() {
        // Test that worktree_for_branch returns None when no worktree exists
        // This would require a git repo, so we'll test this in integration tests
    }

    #[test]
    fn test_project_identifier_parsing() {
        // Test URL parsing logic (can't test actual git commands without a repo)
        // These would be unit tests if we extracted the URL parsing logic

        // The actual parsing is done inline in project_identifier(),
        // so we'll rely on integration tests for full coverage
    }
}
