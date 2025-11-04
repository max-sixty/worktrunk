use rayon::prelude::*;
use std::path::PathBuf;
use worktrunk::git::{GitError, Repository};
use worktrunk::styling::{HINT, HINT_EMOJI, WARNING, WARNING_EMOJI, println};

use super::ci_status::PrStatus;

/// Display fields shared between WorktreeInfo and BranchInfo
/// These contain formatted strings with ANSI colors for json-pretty output
#[derive(serde::Serialize, Default)]
pub struct DisplayFields {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commits_display: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch_diff_display: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_display: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ci_status_display: Option<String>,
}

#[derive(serde::Serialize)]
pub struct WorktreeInfo {
    pub worktree: worktrunk::git::Worktree,
    #[serde(flatten)]
    pub commit: CommitDetails,
    #[serde(flatten)]
    pub counts: AheadBehind,
    pub working_tree_diff: (usize, usize),
    /// Diff between working tree and main branch.
    /// `None` means "not computed" (optimization: skipped when trees differ).
    /// `Some((0, 0))` means working tree matches main exactly.
    /// `Some((a, d))` means a lines added, d deleted vs main.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_tree_diff_with_main: Option<(usize, usize)>,
    #[serde(flatten)]
    pub branch_diff: BranchDiffTotals,
    pub is_primary: bool,
    #[serde(flatten)]
    pub upstream: UpstreamStatus,
    pub worktree_state: Option<String>,
    pub pr_status: Option<PrStatus>,
    pub has_conflicts: bool,
    /// Git status symbols (=, ↑, ↓, ⇡, ⇣, ?, !, +, », ✘) indicating working tree state
    pub status_symbols: String,

    // Display fields for json-pretty format (with ANSI colors)
    #[serde(flatten)]
    pub display: DisplayFields,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_diff_display: Option<String>,
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
    pub pr_status: Option<PrStatus>,
    pub has_conflicts: bool,

    // Display fields for json-pretty format (with ANSI colors)
    #[serde(flatten)]
    pub display: DisplayFields,
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

    pub fn pr_status(&self) -> Option<&PrStatus> {
        match self {
            ListItem::Worktree(info) => info.pr_status.as_ref(),
            ListItem::Branch(info) => info.pr_status.as_ref(),
        }
    }

    pub fn has_conflicts(&self) -> bool {
        match self {
            ListItem::Worktree(info) => info.has_conflicts,
            ListItem::Branch(info) => info.has_conflicts,
        }
    }
}

impl BranchInfo {
    /// Create BranchInfo from a branch name, enriching it with git metadata
    fn from_branch(
        branch: &str,
        repo: &Repository,
        primary_branch: Option<&str>,
        fetch_ci: bool,
        check_conflicts: bool,
    ) -> Result<Self, GitError> {
        // Get the commit SHA for this branch
        let head = repo.run_command(&["rev-parse", branch])?.trim().to_string();

        let commit = CommitDetails::gather(repo, &head)?;
        let counts = AheadBehind::compute(repo, primary_branch, &head)?;
        let branch_diff = BranchDiffTotals::compute(repo, primary_branch, &head)?;
        let upstream = UpstreamStatus::calculate(repo, Some(branch), &head)?;

        let pr_status = if fetch_ci {
            PrStatus::detect(branch, &head)
        } else {
            None
        };

        let has_conflicts = if check_conflicts {
            if let Some(base) = primary_branch {
                repo.has_merge_conflicts(base, &head)?
            } else {
                false
            }
        } else {
            false
        };

        Ok(BranchInfo {
            name: branch.to_string(),
            head,
            commit,
            counts,
            branch_diff,
            upstream,
            pr_status,
            has_conflicts,
            display: DisplayFields::default(),
        })
    }
}

/// Git status information parsed from `git status --porcelain`
// TODO: Consider using a struct with bool fields instead of String for symbols
// (has_untracked, has_modified, has_staged, has_renamed, has_deleted, has_conflicts,
//  main_ahead, main_behind, upstream_ahead, upstream_behind)
// Would enable querying individual states, but currently only used for display.
struct GitStatusInfo {
    /// Whether the working tree has any changes (staged or unstaged)
    is_dirty: bool,
    /// Status symbols: = (conflicts), ↑ (ahead of main), ↓ (behind main), ⇡ (ahead of remote), ⇣ (behind remote), ? (untracked), ! (modified), + (staged), » (renamed), ✘ (deleted)
    symbols: String,
}

/// Parse git status --porcelain output to determine dirty state and status symbols
/// This combines the dirty check and symbol computation in a single git command
fn parse_git_status(
    repo: &Repository,
    main_ahead: usize,
    main_behind: usize,
    upstream_ahead: usize,
    upstream_behind: usize,
) -> Result<GitStatusInfo, GitError> {
    let status_output = repo.run_command(&["status", "--porcelain"])?;

    let mut has_conflicts = false;
    let mut has_untracked = false;
    let mut has_modified = false;
    let mut has_staged = false;
    let mut has_renamed = false;
    let mut has_deleted = false;
    let mut is_dirty = false;

    for line in status_output.lines() {
        if line.len() < 2 {
            continue;
        }

        is_dirty = true; // Any line means changes exist

        // Get status codes (first two bytes for ASCII compatibility)
        let bytes = line.as_bytes();
        let index_status = bytes[0] as char;
        let worktree_status = bytes[1] as char;

        // Unmerged paths (actual conflicts in working tree)
        // U = unmerged, D = both deleted, A = both added
        if index_status == 'U'
            || worktree_status == 'U'
            || (index_status == 'D' && worktree_status == 'D')
            || (index_status == 'A' && worktree_status == 'A')
        {
            has_conflicts = true;
        }

        // Untracked files
        if index_status == '?' && worktree_status == '?' {
            has_untracked = true;
        }

        // Modified (unstaged changes in working tree)
        if worktree_status == 'M' {
            has_modified = true;
        }

        // Staged files (changes in index)
        // Includes: A (added), M (modified), C (copied), but excludes D/R
        if index_status == 'A' || index_status == 'M' || index_status == 'C' {
            has_staged = true;
        }

        // Renamed files (staged rename)
        if index_status == 'R' {
            has_renamed = true;
        }

        // Deleted files (staged or unstaged)
        if index_status == 'D' || worktree_status == 'D' {
            has_deleted = true;
        }
    }

    let mut symbols = String::with_capacity(10);

    // Symbol order: conflicts (blocking) → branch divergence → working tree changes
    // = (conflicts), ↑↓ (vs main), ⇡⇣ (vs remote), ?!+»✘ (working tree state)
    if has_conflicts {
        symbols.push('=');
    }
    // Main branch: simple arrows
    if main_ahead > 0 {
        symbols.push('↑');
    }
    if main_behind > 0 {
        symbols.push('↓');
    }
    // Upstream/remote: double arrows
    if upstream_ahead > 0 {
        symbols.push('⇡');
    }
    if upstream_behind > 0 {
        symbols.push('⇣');
    }
    if has_untracked {
        symbols.push('?');
    }
    if has_modified {
        symbols.push('!');
    }
    if has_staged {
        symbols.push('+');
    }
    if has_renamed {
        symbols.push('»');
    }
    if has_deleted {
        symbols.push('✘');
    }

    Ok(GitStatusInfo { is_dirty, symbols })
}

impl WorktreeInfo {
    /// Create WorktreeInfo from a Worktree, enriching it with git metadata
    fn from_worktree(
        wt: &worktrunk::git::Worktree,
        primary: &worktrunk::git::Worktree,
        fetch_ci: bool,
        check_conflicts: bool,
    ) -> Result<Self, GitError> {
        let wt_repo = Repository::at(&wt.path);
        let is_primary = wt.path == primary.path;

        let commit = CommitDetails::gather(&wt_repo, &wt.head)?;
        let base_branch = primary.branch.as_deref().filter(|_| !is_primary);
        let counts = AheadBehind::compute(&wt_repo, base_branch, &wt.head)?;
        let upstream = UpstreamStatus::calculate(&wt_repo, wt.branch.as_deref(), &wt.head)?;

        // Parse git status once for both dirty check and status symbols
        // Pass both main and upstream ahead/behind counts
        let (upstream_ahead, upstream_behind) = upstream
            .active()
            .map(|(_, ahead, behind)| (ahead, behind))
            .unwrap_or((0, 0));
        let status_info = parse_git_status(
            &wt_repo,
            counts.ahead,
            counts.behind,
            upstream_ahead,
            upstream_behind,
        )?;

        let working_tree_diff = if status_info.is_dirty {
            wt_repo.working_tree_diff_stats()?
        } else {
            (0, 0) // Clean working tree
        };

        // Use tree equality check instead of expensive diff for "matches main"
        let working_tree_diff_with_main = if let Some(base) = base_branch {
            // Get tree hashes for HEAD and base branch
            let head_tree = wt_repo
                .run_command(&["rev-parse", "HEAD^{tree}"])?
                .trim()
                .to_string();
            let base_tree = wt_repo
                .run_command(&["rev-parse", &format!("{}^{{tree}}", base)])?
                .trim()
                .to_string();

            if head_tree == base_tree {
                // Trees are identical - check if working tree is also clean
                if status_info.is_dirty {
                    // Rare case: trees match but working tree has uncommitted changes
                    // Need to compute actual diff to get accurate line counts
                    Some(wt_repo.working_tree_diff_vs_ref(base)?)
                } else {
                    // Trees match and working tree is clean → matches main exactly
                    Some((0, 0))
                }
            } else {
                // Trees differ - skip the expensive scan
                // Return None to indicate "not computed" (optimization)
                None
            }
        } else {
            Some((0, 0)) // Primary worktree always matches itself
        };
        let branch_diff = BranchDiffTotals::compute(&wt_repo, base_branch, &wt.head)?;

        // Get worktree state (merge/rebase/etc)
        let worktree_state = wt_repo.worktree_state()?;

        let pr_status = if fetch_ci {
            wt.branch
                .as_deref()
                .and_then(|branch| PrStatus::detect(branch, &wt.head))
        } else {
            None
        };

        let has_conflicts = if check_conflicts {
            if let Some(base) = base_branch {
                wt_repo.has_merge_conflicts(base, &wt.head)?
            } else {
                false
            }
        } else {
            false
        };

        Ok(WorktreeInfo {
            worktree: wt.clone(),
            commit,
            counts,
            working_tree_diff,
            working_tree_diff_with_main,
            branch_diff,
            is_primary,
            upstream,
            worktree_state,
            pr_status,
            has_conflicts,
            status_symbols: status_info.symbols,
            display: DisplayFields::default(),
            working_diff_display: None,
        })
    }
}

/// Gather list data (worktrees + optional branches).
pub fn gather_list_data(
    repo: &Repository,
    show_branches: bool,
    fetch_ci: bool,
    check_conflicts: bool,
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
    let worktree_results: Vec<Result<WorktreeInfo, GitError>> = worktrees
        .par_iter()
        .map(|wt| WorktreeInfo::from_worktree(wt, &primary, fetch_ci, check_conflicts))
        .collect();

    // Build list of items to display (worktrees + optional branches)
    let mut items: Vec<ListItem> = Vec::new();

    // Process worktree results
    for result in worktree_results {
        match result {
            Ok(info) => items.push(ListItem::Worktree(info)),
            Err(e) => {
                // Worktree enrichment failures are critical - propagate error
                return Err(e);
            }
        }
    }

    // Process branches in parallel if requested
    if show_branches {
        let available_branches = repo.available_branches()?;
        let primary_branch = primary.branch.as_deref();

        let branch_results: Vec<(String, Result<BranchInfo, GitError>)> = available_branches
            .par_iter()
            .map(|branch| {
                let result = BranchInfo::from_branch(
                    branch,
                    repo,
                    primary_branch,
                    fetch_ci,
                    check_conflicts,
                );
                (branch.clone(), result)
            })
            .collect();

        for (branch, result) in branch_results {
            match result {
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

    // Sort by:
    // 1. Main worktree (primary) always first
    // 2. Current worktree second (if not main)
    // 3. Remaining worktrees by age (most recent first)
    items.sort_by_key(|item| {
        let is_primary = item.is_primary();
        let is_current = item
            .worktree_path()
            .and_then(|p| current_worktree_path.as_ref().map(|cp| p == cp))
            .unwrap_or(false);

        // Primary sort key: 0 = main, 1 = current (non-main), 2 = others
        let priority = if is_primary {
            0
        } else if is_current {
            1
        } else {
            2
        };

        // Secondary sort: timestamp (reversed for descending order)
        (priority, std::cmp::Reverse(item.commit_timestamp()))
    });

    Ok(Some(ListData {
        items,
        current_worktree_path,
    }))
}
