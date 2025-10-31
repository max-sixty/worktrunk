use rayon::prelude::*;
use std::path::PathBuf;
use worktrunk::git::{GitError, Repository};
use worktrunk::styling::{HINT, HINT_EMOJI, WARNING, WARNING_EMOJI, println};

#[derive(serde::Serialize)]
pub struct WorktreeInfo {
    pub worktree: worktrunk::git::Worktree,
    #[serde(flatten)]
    pub commit: CommitDetails,
    #[serde(flatten)]
    pub counts: AheadBehind,
    pub working_tree_diff: (usize, usize),
    #[serde(flatten)]
    pub branch_diff: BranchDiffTotals,
    pub is_primary: bool,
    #[serde(flatten)]
    pub upstream: UpstreamStatus,
    pub worktree_state: Option<String>,
}

#[derive(serde::Serialize)]
pub struct BranchInfo {
    pub name: String,
    pub head: String,
    #[serde(flatten)]
    pub commit: CommitDetails,
    #[serde(flatten)]
    pub counts: AheadBehind,
    #[serde(flatten)]
    pub branch_diff: BranchDiffTotals,
    #[serde(flatten)]
    pub upstream: UpstreamStatus,
}

#[derive(serde::Serialize, Clone)]
pub struct CommitDetails {
    pub timestamp: i64,
    pub commit_message: String,
}

impl CommitDetails {
    fn gather(repo: &Repository, head: &str) -> Result<Self, GitError> {
        Ok(Self {
            timestamp: repo.commit_timestamp(head)?,
            commit_message: repo.commit_message(head)?,
        })
    }
}

#[derive(serde::Serialize, Default, Clone)]
pub struct AheadBehind {
    pub ahead: usize,
    pub behind: usize,
}

impl AheadBehind {
    fn compute(repo: &Repository, base: Option<&str>, head: &str) -> Result<Self, GitError> {
        let Some(base) = base else {
            return Ok(Self::default());
        };

        let (ahead, behind) = repo.ahead_behind(base, head)?;
        Ok(Self { ahead, behind })
    }
}

#[derive(serde::Serialize, Default, Clone)]
pub struct BranchDiffTotals {
    #[serde(rename = "branch_diff")]
    pub diff: (usize, usize),
}

impl BranchDiffTotals {
    fn compute(repo: &Repository, base: Option<&str>, head: &str) -> Result<Self, GitError> {
        let Some(base) = base else {
            return Ok(Self::default());
        };

        let diff = repo.branch_diff_stats(base, head)?;
        Ok(Self { diff })
    }
}

#[derive(serde::Serialize, Default, Clone)]
pub struct UpstreamStatus {
    #[serde(rename = "upstream_remote")]
    remote: Option<String>,
    #[serde(rename = "upstream_ahead")]
    ahead: usize,
    #[serde(rename = "upstream_behind")]
    behind: usize,
}

impl UpstreamStatus {
    fn calculate(repo: &Repository, branch: Option<&str>, head: &str) -> Result<Self, GitError> {
        let Some(branch) = branch else {
            return Ok(Self::default());
        };

        match repo.upstream_branch(branch) {
            Ok(Some(upstream_branch)) => {
                let remote = upstream_branch
                    .split_once('/')
                    .map(|(remote, _)| remote)
                    .unwrap_or("origin")
                    .to_string();
                let (ahead, behind) = repo.ahead_behind(&upstream_branch, head)?;
                Ok(Self {
                    remote: Some(remote),
                    ahead,
                    behind,
                })
            }
            _ => Ok(Self::default()),
        }
    }

    pub fn active(&self) -> Option<(&str, usize, usize)> {
        self.remote
            .as_deref()
            .map(|remote| (remote, self.ahead, self.behind))
    }

    #[cfg(test)]
    pub fn from_parts(remote: Option<String>, ahead: usize, behind: usize) -> Self {
        Self {
            remote,
            ahead,
            behind,
        }
    }
}

/// Unified type for displaying worktrees and branches in the same table
#[derive(serde::Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ListItem {
    Worktree(WorktreeInfo),
    Branch(BranchInfo),
}

pub struct ListData {
    pub items: Vec<ListItem>,
    pub current_worktree_path: Option<PathBuf>,
}

impl ListItem {
    pub fn branch_name(&self) -> &str {
        match self {
            ListItem::Worktree(wt) => wt.worktree.branch.as_deref().unwrap_or("(detached)"),
            ListItem::Branch(br) => &br.name,
        }
    }

    pub fn is_primary(&self) -> bool {
        matches!(self, ListItem::Worktree(wt) if wt.is_primary)
    }

    pub fn commit_timestamp(&self) -> i64 {
        match self {
            ListItem::Worktree(info) => info.commit.timestamp,
            ListItem::Branch(info) => info.commit.timestamp,
        }
    }

    pub fn head(&self) -> &str {
        match self {
            ListItem::Worktree(info) => &info.worktree.head,
            ListItem::Branch(info) => &info.head,
        }
    }

    pub fn commit_details(&self) -> &CommitDetails {
        match self {
            ListItem::Worktree(info) => &info.commit,
            ListItem::Branch(info) => &info.commit,
        }
    }

    pub fn counts(&self) -> &AheadBehind {
        match self {
            ListItem::Worktree(info) => &info.counts,
            ListItem::Branch(info) => &info.counts,
        }
    }

    pub fn branch_diff(&self) -> &BranchDiffTotals {
        match self {
            ListItem::Worktree(info) => &info.branch_diff,
            ListItem::Branch(info) => &info.branch_diff,
        }
    }

    pub fn upstream(&self) -> &UpstreamStatus {
        match self {
            ListItem::Worktree(info) => &info.upstream,
            ListItem::Branch(info) => &info.upstream,
        }
    }

    pub fn worktree_info(&self) -> Option<&WorktreeInfo> {
        match self {
            ListItem::Worktree(info) => Some(info),
            ListItem::Branch(_) => None,
        }
    }

    pub fn worktree_path(&self) -> Option<&PathBuf> {
        self.worktree_info().map(|info| &info.worktree.path)
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

        let commit = CommitDetails::gather(repo, &head)?;
        let counts = AheadBehind::compute(repo, primary_branch, &head)?;
        let branch_diff = BranchDiffTotals::compute(repo, primary_branch, &head)?;
        let upstream = UpstreamStatus::calculate(repo, Some(branch), &head)?;

        Ok(BranchInfo {
            name: branch.to_string(),
            head,
            commit,
            counts,
            branch_diff,
            upstream,
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

        let commit = CommitDetails::gather(&wt_repo, &wt.head)?;
        let base_branch = primary.branch.as_deref().filter(|_| !is_primary);
        let counts = AheadBehind::compute(&wt_repo, base_branch, &wt.head)?;

        let working_tree_diff = wt_repo.working_tree_diff_stats()?;
        let branch_diff = BranchDiffTotals::compute(&wt_repo, base_branch, &wt.head)?;
        let upstream = UpstreamStatus::calculate(&wt_repo, wt.branch.as_deref(), &wt.head)?;

        // Get worktree state (merge/rebase/etc)
        let worktree_state = wt_repo.worktree_state()?;

        Ok(WorktreeInfo {
            worktree: wt.clone(),
            commit,
            counts,
            working_tree_diff,
            branch_diff,
            is_primary,
            upstream,
            worktree_state,
        })
    }
}

/// Gather list data (worktrees + optional branches).
pub fn gather_list_data(
    repo: &Repository,
    show_branches: bool,
) -> Result<Option<ListData>, GitError> {
    let worktrees = repo.list_worktrees()?;

    if worktrees.is_empty() {
        return Ok(None);
    }

    // First worktree is the primary - clone it for use in closure
    let primary = worktrees[0].clone();

    // Get current worktree to identify active one
    let current_worktree_path = repo.worktree_root().ok();

    // Gather enhanced information for all worktrees in parallel
    let worktree_infos: Vec<WorktreeInfo> = worktrees
        .par_iter()
        .map(|wt| WorktreeInfo::from_worktree(wt, &primary))
        .collect::<Result<Vec<_>, _>>()?;

    // Build list of items to display (worktrees + optional branches)
    let mut items: Vec<ListItem> = worktree_infos.into_iter().map(ListItem::Worktree).collect();

    if show_branches {
        let available_branches = repo.available_branches()?;
        let primary_branch = primary.branch.as_deref();
        for branch in available_branches {
            match BranchInfo::from_branch(&branch, repo, primary_branch) {
                Ok(branch_info) => items.push(ListItem::Branch(branch_info)),
                Err(e) => {
                    let warning_bold = WARNING.bold();
                    println!(
                        "{WARNING_EMOJI} {WARNING}Failed to enrich branch {warning_bold}{branch}{warning_bold:#}: {e}{WARNING:#}"
                    );
                    println!(
                        "{HINT_EMOJI} {HINT}This branch will be shown with limited information{HINT:#}"
                    );
                }
            }
        }
    }

    // Sort by most recent commit (descending)
    items.sort_by_key(|item| std::cmp::Reverse(item.commit_timestamp()));

    Ok(Some(ListData {
        items,
        current_worktree_path,
    }))
}
