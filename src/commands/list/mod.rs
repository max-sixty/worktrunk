mod layout;
mod render;

#[cfg(test)]
mod spacing_test;

use rayon::prelude::*;
use worktrunk::git::{GitError, Repository};
use worktrunk::styling::{WARNING, WARNING_EMOJI, eprintln};

use layout::calculate_responsive_layout;
use render::{format_header_line, format_list_item_line};

#[derive(serde::Serialize)]
pub struct WorktreeInfo {
    pub worktree: worktrunk::git::Worktree,
    pub timestamp: i64,
    pub commit_message: String,
    pub ahead: usize,
    pub behind: usize,
    pub working_tree_diff: (usize, usize),
    pub branch_diff: (usize, usize),
    pub is_primary: bool,
    pub upstream_remote: Option<String>,
    pub upstream_ahead: usize,
    pub upstream_behind: usize,
    pub worktree_state: Option<String>,
}

#[derive(serde::Serialize)]
pub struct BranchInfo {
    pub name: String,
    pub head: String,
    pub timestamp: i64,
    pub commit_message: String,
    pub ahead: usize,
    pub behind: usize,
    pub branch_diff: (usize, usize),
    pub upstream_remote: Option<String>,
    pub upstream_ahead: usize,
    pub upstream_behind: usize,
}

/// Unified type for displaying worktrees and branches in the same table
#[derive(serde::Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ListItem {
    Worktree(WorktreeInfo),
    Branch(BranchInfo),
}

impl ListItem {
    pub fn timestamp(&self) -> i64 {
        match self {
            ListItem::Worktree(wt) => wt.timestamp,
            ListItem::Branch(br) => br.timestamp,
        }
    }

    pub fn worktree_info(&self) -> Option<&WorktreeInfo> {
        match self {
            ListItem::Worktree(wt) => Some(wt),
            ListItem::Branch(_) => None,
        }
    }

    pub fn branch_name(&self) -> &str {
        match self {
            ListItem::Worktree(wt) => wt.worktree.branch.as_deref().unwrap_or("(detached)"),
            ListItem::Branch(br) => &br.name,
        }
    }

    pub fn commit_head(&self) -> &str {
        match self {
            ListItem::Worktree(wt) => &wt.worktree.head,
            ListItem::Branch(br) => &br.head,
        }
    }

    pub fn commit_message(&self) -> &str {
        match self {
            ListItem::Worktree(wt) => &wt.commit_message,
            ListItem::Branch(br) => &br.commit_message,
        }
    }

    pub fn ahead(&self) -> usize {
        match self {
            ListItem::Worktree(wt) => wt.ahead,
            ListItem::Branch(br) => br.ahead,
        }
    }

    pub fn behind(&self) -> usize {
        match self {
            ListItem::Worktree(wt) => wt.behind,
            ListItem::Branch(br) => br.behind,
        }
    }

    pub fn is_primary(&self) -> bool {
        match self {
            ListItem::Worktree(wt) => wt.is_primary,
            ListItem::Branch(_) => false,
        }
    }

    pub fn branch_diff(&self) -> (usize, usize) {
        match self {
            ListItem::Worktree(wt) => wt.branch_diff,
            ListItem::Branch(br) => br.branch_diff,
        }
    }

    pub fn working_tree_diff(&self) -> Option<(usize, usize)> {
        match self {
            ListItem::Worktree(wt) => Some(wt.working_tree_diff),
            ListItem::Branch(_) => None,
        }
    }

    pub fn upstream_info(&self) -> Option<(&str, usize, usize)> {
        match self {
            ListItem::Worktree(wt) => {
                if wt.upstream_ahead > 0 || wt.upstream_behind > 0 {
                    Some((
                        wt.upstream_remote.as_deref().unwrap_or("origin"),
                        wt.upstream_ahead,
                        wt.upstream_behind,
                    ))
                } else {
                    None
                }
            }
            ListItem::Branch(br) => {
                if br.upstream_ahead > 0 || br.upstream_behind > 0 {
                    Some((
                        br.upstream_remote.as_deref().unwrap_or("origin"),
                        br.upstream_ahead,
                        br.upstream_behind,
                    ))
                } else {
                    None
                }
            }
        }
    }
}

impl BranchInfo {
    /// Create BranchInfo from a branch name, enriching it with git metadata
    fn from_branch(
        branch: &str,
        repo: &Repository,
        primary_branch: Option<&str>,
    ) -> Result<Self, GitError> {
        // Get the commit SHA for this branch
        let head = repo.run_command(&["rev-parse", branch])?.trim().to_string();

        // Get commit timestamp
        let timestamp = repo.commit_timestamp(&head)?;

        // Get commit message
        let commit_message = repo.commit_message(&head)?;

        // Calculate ahead/behind relative to primary branch
        let (ahead, behind) = if let Some(pb) = primary_branch {
            repo.ahead_behind(pb, &head)?
        } else {
            (0, 0)
        };

        // Get branch diff stats (line diff relative to primary)
        let branch_diff = if let Some(pb) = primary_branch {
            repo.branch_diff_stats(pb, &head)?
        } else {
            (0, 0)
        };

        // Get upstream tracking info
        let (upstream_remote, upstream_ahead, upstream_behind) =
            match repo.upstream_branch(branch).ok().flatten() {
                Some(upstream_branch) => {
                    let remote = upstream_branch
                        .split_once('/')
                        .map(|(remote, _)| remote)
                        .unwrap_or("origin")
                        .to_string();
                    let (ahead, behind) = repo.ahead_behind(&upstream_branch, &head)?;
                    (Some(remote), ahead, behind)
                }
                None => (None, 0, 0),
            };

        Ok(BranchInfo {
            name: branch.to_string(),
            head,
            timestamp,
            commit_message,
            ahead,
            behind,
            branch_diff,
            upstream_remote,
            upstream_ahead,
            upstream_behind,
        })
    }
}

impl WorktreeInfo {
    /// Create WorktreeInfo from a Worktree, enriching it with git metadata
    fn from_worktree(
        wt: &worktrunk::git::Worktree,
        primary: &worktrunk::git::Worktree,
    ) -> Result<Self, GitError> {
        let wt_repo = Repository::at(&wt.path);
        let is_primary = wt.path == primary.path;

        // Get commit timestamp
        let timestamp = wt_repo.commit_timestamp(&wt.head)?;

        // Get commit message
        let commit_message = wt_repo.commit_message(&wt.head)?;

        // Calculate ahead/behind relative to primary branch (only if primary has a branch)
        let (ahead, behind) = if is_primary {
            (0, 0)
        } else if let Some(pb) = primary.branch.as_deref() {
            wt_repo.ahead_behind(pb, &wt.head)?
        } else {
            (0, 0)
        };
        let working_tree_diff = wt_repo.working_tree_diff_stats()?;

        // Get branch diff stats (downstream of primary, only if primary has a branch)
        let branch_diff = if is_primary {
            (0, 0)
        } else if let Some(pb) = primary.branch.as_deref() {
            wt_repo.branch_diff_stats(pb, &wt.head)?
        } else {
            (0, 0)
        };

        // Get upstream tracking info
        let (upstream_remote, upstream_ahead, upstream_behind) = match wt
            .branch
            .as_ref()
            .and_then(|b| wt_repo.upstream_branch(b).ok().flatten())
        {
            Some(upstream_branch) => {
                // Extract remote name from "origin/main" -> "origin"
                let remote = upstream_branch
                    .split_once('/')
                    .map(|(remote, _)| remote)
                    .unwrap_or("origin")
                    .to_string();
                let (ahead, behind) = wt_repo.ahead_behind(&upstream_branch, &wt.head)?;
                (Some(remote), ahead, behind)
            }
            None => (None, 0, 0),
        };

        // Get worktree state (merge/rebase/etc)
        let worktree_state = wt_repo.worktree_state()?;

        Ok(WorktreeInfo {
            worktree: wt.clone(),
            timestamp,
            commit_message,
            ahead,
            behind,
            working_tree_diff,
            branch_diff,
            is_primary,
            upstream_remote,
            upstream_ahead,
            upstream_behind,
            worktree_state,
        })
    }
}

pub fn handle_list(format: crate::OutputFormat, show_branches: bool) -> Result<(), GitError> {
    let repo = Repository::current();
    let worktrees = repo.list_worktrees()?;

    if worktrees.is_empty() {
        return Ok(());
    }

    // First worktree is the primary - clone it for use in closure
    let primary = worktrees[0].clone();

    // Get current worktree to identify active one
    let current_worktree_path = repo.worktree_root().ok();

    // Gather enhanced information for all worktrees in parallel
    //
    // Parallelization strategy: Use Rayon to process worktrees concurrently.
    // Each worktree requires ~5 git operations (timestamp, ahead/behind, diffs).
    //
    // Benchmark results: See benches/list.rs for sequential vs parallel comparison.
    //
    // Decision: Always use parallel for simplicity and 2+ worktree performance.
    // Rayon overhead (~1-2ms) is acceptable for single-worktree case.
    //
    // TODO: Could parallelize the 5 git commands within each worktree if needed,
    // but worktree-level parallelism provides the best cost/benefit tradeoff
    let worktree_infos: Vec<WorktreeInfo> = worktrees
        .par_iter()
        .map(|wt| WorktreeInfo::from_worktree(wt, &primary))
        .collect::<Result<Vec<_>, _>>()?;

    // Build list of items to display (worktrees + optional branches)
    let mut items: Vec<ListItem> = worktree_infos.into_iter().map(ListItem::Worktree).collect();

    // Add branches if requested
    if show_branches {
        let available_branches = repo.available_branches()?;
        let primary_branch = primary.branch.as_deref();
        for branch in available_branches {
            match BranchInfo::from_branch(&branch, &repo, primary_branch) {
                Ok(branch_info) => items.push(ListItem::Branch(branch_info)),
                Err(e) => {
                    let warning_bold = WARNING.bold();
                    eprintln!(
                        "{WARNING_EMOJI} Failed to enrich branch {warning_bold}{branch}{warning_bold:#}: {e}"
                    );
                }
            }
        }
    }

    // Sort by most recent commit (descending)
    items.sort_by_key(|b| std::cmp::Reverse(b.timestamp()));

    match format {
        crate::OutputFormat::Json => {
            // Output JSON format
            let json = serde_json::to_string_pretty(&items).map_err(|e| {
                GitError::CommandFailed(format!("Failed to serialize to JSON: {}", e))
            })?;
            println!("{}", json);
        }
        crate::OutputFormat::Table => {
            // Calculate responsive layout based on terminal width
            let layout = calculate_responsive_layout(&items);

            // Display header
            format_header_line(&layout);

            // Display formatted output
            for item in &items {
                format_list_item_line(item, &layout, current_worktree_path.as_ref());
            }
        }
    }

    Ok(())
}
