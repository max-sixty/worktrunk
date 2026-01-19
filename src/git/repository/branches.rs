//! Branch-related operations for Repository.
//!
//! For single-branch operations, see [`super::Branch`].
//! This module contains multi-branch operations (listing, filtering, etc.).

use std::collections::HashSet;

use super::{BranchCategory, CompletionBranch, Repository};

impl Repository {
    /// Check if a git reference exists (branch, tag, commit SHA, HEAD, etc.).
    ///
    /// Accepts any valid commit-ish: branch names, tags, HEAD, commit SHAs,
    /// and relative refs like HEAD~2.
    pub fn ref_exists(&self, reference: &str) -> anyhow::Result<bool> {
        // Use rev-parse to check if the reference resolves to a valid commit
        // The ^{commit} suffix ensures we get the commit object, not a tag
        Ok(self
            .run_command(&[
                "rev-parse",
                "--verify",
                &format!("{}^{{commit}}", reference),
            ])
            .is_ok())
    }

    /// Get all branch names (local branches only).
    pub fn all_branches(&self) -> anyhow::Result<Vec<String>> {
        let stdout = self.run_command(&[
            "branch",
            "--sort=-committerdate",
            "--format=%(refname:lstrip=2)",
        ])?;
        Ok(stdout
            .lines()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect())
    }

    /// List all local branches.
    pub(super) fn local_branches(&self) -> anyhow::Result<Vec<String>> {
        // Use lstrip=2 instead of refname:short - git adds "heads/" prefix to short
        // names when disambiguation is needed (e.g., branch "foo" + remote "foo").
        let stdout = self.run_command(&["branch", "--format=%(refname:lstrip=2)"])?;
        Ok(stdout.lines().map(|s| s.trim().to_string()).collect())
    }

    /// List all local branches with their HEAD commit SHA.
    /// Returns a vector of (branch_name, commit_sha) tuples.
    pub fn list_local_branches(&self) -> anyhow::Result<Vec<(String, String)>> {
        let output = self.run_command(&[
            "for-each-ref",
            "--format=%(refname:lstrip=2) %(objectname)",
            "refs/heads/",
        ])?;

        let branches: Vec<(String, String)> = output
            .lines()
            .filter_map(|line| {
                let (branch, sha) = line.split_once(' ')?;
                Some((branch.to_string(), sha.to_string()))
            })
            .collect();

        Ok(branches)
    }

    /// List remote branches from all remotes, excluding HEAD refs.
    ///
    /// Returns (branch_name, commit_sha) pairs for remote branches.
    /// Branch names are in the form "origin/feature", not "feature".
    pub fn list_remote_branches(&self) -> anyhow::Result<Vec<(String, String)>> {
        let output = self.run_command(&[
            "for-each-ref",
            "--format=%(refname:lstrip=2) %(objectname)",
            "refs/remotes/",
        ])?;

        let branches: Vec<(String, String)> = output
            .lines()
            .filter_map(|line| {
                let (branch_name, sha) = line.split_once(' ')?;
                // Skip <remote>/HEAD (symref)
                if branch_name.ends_with("/HEAD") {
                    None
                } else {
                    Some((branch_name.to_string(), sha.to_string()))
                }
            })
            .collect();

        Ok(branches)
    }

    /// List all upstream tracking refs that local branches are tracking.
    ///
    /// Returns a set of upstream refs like "origin/main", "origin/feature".
    /// Useful for filtering remote branches to only show those not tracked locally.
    pub fn list_tracked_upstreams(&self) -> anyhow::Result<HashSet<String>> {
        let output =
            self.run_command(&["for-each-ref", "--format=%(upstream:short)", "refs/heads/"])?;

        let upstreams: HashSet<String> = output
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| line.to_string())
            .collect();

        Ok(upstreams)
    }

    /// List remote branches that aren't tracked by any local branch.
    ///
    /// Returns (branch_name, commit_sha) pairs for remote branches that have no
    /// corresponding local tracking branch.
    pub fn list_untracked_remote_branches(&self) -> anyhow::Result<Vec<(String, String)>> {
        let all_remote_branches = self.list_remote_branches()?;
        let tracked_upstreams = self.list_tracked_upstreams()?;

        let remote_branches: Vec<_> = all_remote_branches
            .into_iter()
            .filter(|(remote_branch_name, _)| !tracked_upstreams.contains(remote_branch_name))
            .collect();

        Ok(remote_branches)
    }

    /// Get branches that don't have worktrees (available for switch).
    pub fn available_branches(&self) -> anyhow::Result<Vec<String>> {
        let all_branches = self.all_branches()?;
        let worktrees = self.list_worktrees()?;

        // Collect branches that have worktrees
        let branches_with_worktrees: HashSet<String> = worktrees
            .iter()
            .filter_map(|wt| wt.branch.clone())
            .collect();

        // Filter out branches with worktrees
        Ok(all_branches
            .into_iter()
            .filter(|branch| !branches_with_worktrees.contains(branch))
            .collect())
    }

    /// Get branches with metadata for shell completions.
    ///
    /// Returns branches in completion order: worktrees first, then local branches,
    /// then remote-only branches. Each category is sorted by recency.
    ///
    /// Searches all remotes (matching git's checkout behavior). If the same branch
    /// exists on multiple remotes, returns the most recently committed version.
    ///
    /// For remote branches, returns the local name (e.g., "fix" not "origin/fix")
    /// since `git worktree add path fix` auto-creates a tracking branch.
    pub fn branches_for_completion(&self) -> anyhow::Result<Vec<CompletionBranch>> {
        // Get worktree branches
        let worktrees = self.list_worktrees()?;
        let worktree_branches: HashSet<String> = worktrees
            .iter()
            .filter_map(|wt| wt.branch.clone())
            .collect();

        // Get local branches with timestamps
        let local_output = self.run_command(&[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname:lstrip=2)\t%(committerdate:unix)",
            "refs/heads/",
        ])?;

        let local_branches: Vec<(String, i64)> = local_output
            .lines()
            .filter_map(|line| {
                let (name, timestamp_str) = line.split_once('\t')?;
                let timestamp = timestamp_str.parse().unwrap_or(0);
                Some((name.to_string(), timestamp))
            })
            .collect();

        let local_branch_names: HashSet<String> =
            local_branches.iter().map(|(n, _)| n.clone()).collect();

        // Get remote branches with timestamps from all remotes
        // Matches git's behavior: searches all remotes for branch names
        let remote_output = self.run_command(&[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname:lstrip=2)\t%(committerdate:unix)",
            "refs/remotes/",
        ])?;

        // Track seen branch names to deduplicate (same branch on multiple remotes)
        let mut seen_branches: HashSet<String> = HashSet::new();
        let remote_branches: Vec<(String, String, i64)> = remote_output
            .lines()
            .filter_map(|line| {
                // Format: "<remote>/<branch>\t<timestamp>"
                let (full_name, timestamp_str) = line.split_once('\t')?;

                // Parse <remote>/<branch> - find first slash to split
                let (remote_name, local_name) = full_name.split_once('/')?;

                // Skip <remote>/HEAD
                if local_name == "HEAD" {
                    return None;
                }
                // Skip if local branch exists (user should use local)
                if local_branch_names.contains(local_name) {
                    return None;
                }
                // Skip if already seen (same branch on another remote)
                if !seen_branches.insert(local_name.to_string()) {
                    return None;
                }

                let timestamp = timestamp_str.parse().unwrap_or(0);
                Some((local_name.to_string(), remote_name.to_string(), timestamp))
            })
            .collect();

        // Build result: worktrees first, then local, then remote
        let mut result = Vec::new();

        // Worktree branches (sorted by recency from local_branches order)
        for (name, timestamp) in &local_branches {
            if worktree_branches.contains(name) {
                result.push(CompletionBranch {
                    name: name.clone(),
                    timestamp: *timestamp,
                    category: BranchCategory::Worktree,
                });
            }
        }

        // Local branches without worktrees
        for (name, timestamp) in &local_branches {
            if !worktree_branches.contains(name) {
                result.push(CompletionBranch {
                    name: name.clone(),
                    timestamp: *timestamp,
                    category: BranchCategory::Local,
                });
            }
        }

        // Remote-only branches
        for (local_name, remote_name, timestamp) in remote_branches {
            result.push(CompletionBranch {
                name: local_name,
                timestamp,
                category: BranchCategory::Remote(remote_name),
            });
        }

        Ok(result)
    }
}
