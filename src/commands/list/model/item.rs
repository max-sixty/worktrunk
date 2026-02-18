//! Core list item types for worktrees and branches.
//!
//! This module contains the main data structures used to represent
//! worktrees and branches in `wt list` output.

use std::path::PathBuf;

use worktrunk::git::{IntegrationReason, IntegrationSignals, LineDiff, check_integration};

use super::state::{ActiveGitOperation, Divergence, MainState, OperationState, WorktreeState};
use super::stats::{AheadBehind, BranchDiffTotals, CommitDetails, UpstreamStatus};
use super::status_symbols::{StatusSymbols, WorkingTreeStatus};
use crate::commands::list::ci_status::PrStatus;
use crate::commands::list::columns::ColumnKind;

/// Display fields shared between WorktreeInfo and BranchInfo
/// These contain formatted strings with ANSI colors for json-pretty output
#[derive(Clone, serde::Serialize, Default)]
pub struct DisplayFields {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commits_display: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch_diff_display: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_display: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ci_status_display: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_display: Option<String>,
    /// Pre-formatted single-line representation for statusline tools.
    /// Format: `branch  status  @working  commits  ^branch_diff  upstream  ci` (2-space separators)
    ///
    /// Use via JSON: `wt list --format=json | jq '.[] | select(.is_current) | .statusline'`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub statusline: Option<String>,
}

impl DisplayFields {
    pub(crate) fn from_common_fields(
        counts: &Option<AheadBehind>,
        branch_diff: &Option<BranchDiffTotals>,
        upstream: &Option<UpstreamStatus>,
    ) -> Self {
        let commits_display = counts
            .as_ref()
            .and_then(|c| ColumnKind::AheadBehind.format_diff_plain(c.ahead, c.behind));

        let branch_diff_display = branch_diff.as_ref().and_then(|bd| {
            ColumnKind::BranchDiff.format_diff_plain(bd.diff.added, bd.diff.deleted)
        });

        let upstream_display = upstream.as_ref().and_then(|u| {
            u.active().and_then(|active| {
                ColumnKind::Upstream.format_diff_plain(active.ahead, active.behind)
            })
        });

        Self {
            commits_display,
            branch_diff_display,
            upstream_display,
            // CI renders via render_indicator() in render.rs, not as display text
            ci_status_display: None,
            status_display: None,
            statusline: None,
        }
    }
}

/// Type-specific data for worktrees
#[derive(Clone, serde::Serialize, Default)]
pub struct WorktreeData {
    pub path: PathBuf,
    pub detached: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locked: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prunable: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_tree_diff: Option<LineDiff>,
    /// Git operation in progress (rebase/merge)
    #[serde(skip_serializing_if = "ActiveGitOperation::is_none")]
    pub git_operation: ActiveGitOperation,
    pub is_main: bool,
    /// Whether this is the current worktree (matches repo discovery path: PWD or `-C`)
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_current: bool,
    /// Whether this was the previous worktree (from `worktrunk.history`)
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_previous: bool,
    /// Whether the worktree is at an unexpected location (branch-worktree mismatch).
    /// Only true when: has branch name, not main worktree, and path differs from template.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub branch_worktree_mismatch: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_diff_display: Option<String>,
}

impl WorktreeData {
    /// Returns true if this worktree is prunable (directory deleted but git still tracks metadata).
    pub fn is_prunable(&self) -> bool {
        self.prunable.is_some()
    }

    /// Create WorktreeData from a WorktreeInfo, with all computed fields set to None.
    pub(crate) fn from_worktree(
        wt: &worktrunk::git::WorktreeInfo,
        is_main: bool,
        is_current: bool,
        is_previous: bool,
    ) -> Self {
        Self {
            // Identity fields (known immediately from worktree list)
            path: wt.path.clone(),
            detached: wt.detached,
            locked: wt.locked.clone(),
            prunable: wt.prunable.clone(),
            is_main,
            is_current,
            is_previous,

            // Computed fields start as None (filled progressively)
            ..Default::default()
        }
    }
}

/// Discriminator for item type (worktree vs branch)
///
/// WorktreeData is boxed to reduce the size of ItemKind enum (304 bytes → 24 bytes).
/// This reduces stack pressure when passing ListItem by value and improves cache locality
/// in `Vec<ListItem>` by keeping the discriminant and common fields together.
#[derive(serde::Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ItemKind {
    Worktree(Box<WorktreeData>),
    Branch,
}

/// Unified item for displaying worktrees and branches in the same table
#[derive(serde::Serialize)]
pub struct ListItem {
    // Common fields (present for both worktrees and branches)
    #[serde(rename = "head_sha")]
    pub head: String,
    /// Branch name - None for detached worktrees
    pub branch: Option<String>,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub commit: Option<CommitDetails>,

    // TODO: Evaluate if skipping these fields in JSON when None is correct behavior.
    // Currently, main worktree omits counts/branch_diff (since it doesn't compare to itself),
    // but consumers may expect these fields to always be present (even if zero).
    // Consider: always include with default values vs current "omit when not computed" approach.
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub counts: Option<AheadBehind>,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub branch_diff: Option<BranchDiffTotals>,
    /// Whether HEAD's tree SHA matches the integration target's tree SHA.
    /// True when committed content is identical regardless of commit history.
    /// Internal field used to compute `BranchState::Integrated(TreesMatch)`.
    #[serde(skip)]
    pub committed_trees_match: Option<bool>,
    /// Whether branch has file changes beyond the merge-base with the integration target.
    /// False when three-dot diff (`<integration-target>...branch`) is empty.
    /// Internal field used for integration detection (no unique content).
    #[serde(skip)]
    pub has_file_changes: Option<bool>,
    /// Whether merging branch into the integration target would add changes (merge simulation).
    /// False when `git merge-tree --write-tree <integration-target> branch` produces the same tree
    /// as the integration target. Catches squash-merged branches where the integration target advanced.
    #[serde(skip)]
    pub would_merge_add: Option<bool>,
    /// Whether branch HEAD is an ancestor of the integration target (or same commit).
    /// True means branch is already part of the integration target's history.
    /// This is the cheapest integration check (~1ms).
    #[serde(skip)]
    pub is_ancestor: Option<bool>,
    /// Whether this branch is an orphan (no common ancestor with default branch).
    /// Orphan branches have independent history and can't compute meaningful ahead/behind counts.
    #[serde(skip)]
    pub is_orphan: Option<bool>,

    // TODO: Same concern as counts/branch_diff above - should upstream fields always be present?
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<UpstreamStatus>,

    /// CI/PR status: None = not loaded, Some(None) = no CI, Some(Some(status)) = has CI
    pub pr_status: Option<Option<PrStatus>>,

    /// Dev server URL computed from project config template
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Whether the URL's port is actively listening
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url_active: Option<bool>,

    /// LLM-generated branch summary: None = not loaded, Some(None) = no summary, Some(Some) = has summary
    #[serde(skip)]
    pub summary: Option<Option<String>>,

    /// Git status symbols - None until all dependencies are ready.
    /// Note: This field is not serialized directly. JSON output converts to JsonItem first.
    #[serde(skip)]
    pub status_symbols: Option<StatusSymbols>,

    // Display fields for json-pretty format (with ANSI colors)
    #[serde(flatten)]
    pub display: DisplayFields,

    // Type-specific data (worktree vs branch)
    #[serde(flatten)]
    pub kind: ItemKind,
}

/// Container for list command results.
pub struct ListData {
    pub items: Vec<ListItem>,
    /// Path to the main worktree, used for computing relative paths in display.
    #[cfg_attr(windows, allow(dead_code))] // Used only by select module (unix-only)
    pub main_worktree_path: std::path::PathBuf,
    /// Tasks that were skipped during collection (includes runtime gating like
    /// SummaryGenerate disabled when no LLM configured). Callers that recalculate
    /// layout (e.g., the picker at a different width) should use this set.
    #[cfg_attr(windows, allow(dead_code))] // Used only by select module (unix-only)
    pub skip_tasks: std::collections::HashSet<super::super::collect::TaskKind>,
}

impl ListItem {
    /// Create a ListItem for a branch (not a worktree)
    pub(crate) fn new_branch(head: String, branch: String) -> Self {
        Self {
            head,
            branch: Some(branch),
            commit: None,
            counts: None,
            branch_diff: None,
            committed_trees_match: None,
            has_file_changes: None,
            would_merge_add: None,
            is_ancestor: None,
            is_orphan: None,
            upstream: None,
            pr_status: None,
            url: None,
            url_active: None,
            summary: None,
            status_symbols: None,
            display: DisplayFields::default(),
            kind: ItemKind::Branch,
        }
    }

    pub fn branch_name(&self) -> &str {
        self.branch.as_deref().unwrap_or("(detached)")
    }

    pub fn is_main(&self) -> bool {
        matches!(&self.kind, ItemKind::Worktree(data) if data.is_main)
    }

    pub fn head(&self) -> &str {
        &self.head
    }

    pub fn branch_diff(&self) -> Option<&BranchDiffTotals> {
        self.branch_diff.as_ref()
    }

    pub fn upstream(&self) -> UpstreamStatus {
        self.upstream.clone().unwrap_or_default()
    }

    pub fn worktree_data(&self) -> Option<&WorktreeData> {
        match &self.kind {
            ItemKind::Worktree(data) => Some(data),
            ItemKind::Branch => None,
        }
    }

    pub fn worktree_data_mut(&mut self) -> Option<&mut WorktreeData> {
        match &mut self.kind {
            ItemKind::Worktree(data) => Some(data),
            ItemKind::Branch => None,
        }
    }

    pub fn worktree_path(&self) -> Option<&PathBuf> {
        self.worktree_data().map(|data| &data.path)
    }

    /// Determine if the item contains no unique work and can likely be removed.
    ///
    /// Returns:
    /// - `Some(true)` - confirmed removable (branch integrated into integration target)
    /// - `Some(false)` - confirmed not removable (has unique work)
    /// - `None` - data still loading, cannot determine yet
    ///
    /// Checks (in order):
    /// 1. **Same commit** - ahead/behind vs default branch is 0.
    ///    The branch is already part of the default branch's history.
    /// 2. **No file changes** - three-dot diff (`<integration-target>...branch`) is empty.
    ///    Catches squash-merged branches where commits exist but add no files.
    /// 3. **Tree matches integration target** - tree SHA equals the target's tree SHA.
    ///    Catches rebased/squash-merged branches with identical content.
    /// 4. **Merge simulation** - merging branch into the integration target wouldn't change the
    ///    target's tree. Catches squash-merged branches where the integration target advanced.
    /// 5. **Working tree matches default branch** (worktrees only) - uncommitted changes
    ///    don't diverge from the default branch.
    pub(crate) fn is_potentially_removable(&self) -> Option<bool> {
        // Use already-computed status_symbols if available
        let main_state = self.status_symbols.as_ref()?.main_state;
        // SameCommit excluded: has uncommitted work that would be lost
        Some(matches!(
            main_state,
            MainState::Empty | MainState::Integrated(_)
        ))
    }

    /// Whether the branch/path text should be dimmed in list output.
    ///
    /// Returns true only when we have confirmed the item is removable.
    /// Returns false when data is still loading (prevents UI flash).
    pub(crate) fn should_dim(&self) -> bool {
        self.is_potentially_removable() == Some(true)
    }

    /// Format this item as a single-line statusline string with clickable links.
    ///
    /// Format: `branch  status  @working  commits  ^branch_diff  upstream  ci`
    /// Uses 2-space separators between non-empty parts.
    pub fn format_statusline(&self) -> String {
        self.format_statusline_with_options(true)
    }

    /// Format this item as a single-line statusline string with link control.
    ///
    /// When `include_links` is false, CI indicators are colored but not clickable.
    /// Used for environments that don't support OSC 8 hyperlinks (e.g., Claude Code).
    pub fn format_statusline_with_options(&self, include_links: bool) -> String {
        use super::statusline_segment::StatuslineSegment;
        StatuslineSegment::join(&self.format_statusline_segments(include_links))
    }

    /// Format this item as prioritized segments for smart truncation.
    ///
    /// Returns segments with priorities matching `wt list` column priorities.
    /// Use [`super::statusline_segment::StatuslineSegment::fit_to_width`] to truncate intelligently.
    pub fn format_statusline_segments(
        &self,
        include_links: bool,
    ) -> Vec<super::statusline_segment::StatuslineSegment> {
        use super::statusline_segment::StatuslineSegment;

        let mut segments = Vec::new();

        // 1. Branch name (priority 1)
        segments.push(StatuslineSegment::from_column(
            self.branch_name().to_string(),
            ColumnKind::Branch,
        ));

        // 2. Status symbols (priority 2)
        if let Some(ref symbols) = self.status_symbols {
            let status = symbols.format_compact();
            if !status.is_empty() {
                segments.push(StatuslineSegment::from_column(status, ColumnKind::Status));
            }
        }

        // 3. Working diff (priority 3)
        if let Some(data) = self.worktree_data()
            && let Some(ref diff) = data.working_tree_diff
            && !diff.is_empty()
            && let Some(formatted) =
                ColumnKind::WorkingDiff.format_diff_plain(diff.added, diff.deleted)
        {
            segments.push(StatuslineSegment::from_column(
                format!("@{formatted}"),
                ColumnKind::WorkingDiff,
            ));
        }

        // 4. Commits ahead/behind main (priority 4)
        if let Some(counts) = self.counts
            && let Some(formatted) =
                ColumnKind::AheadBehind.format_diff_plain(counts.ahead, counts.behind)
        {
            segments.push(StatuslineSegment::from_column(
                formatted,
                ColumnKind::AheadBehind,
            ));
        }

        // 5. Branch diff vs main (priority 5)
        if let Some(branch_diff) = self.branch_diff()
            && !branch_diff.diff.is_empty()
            && let Some(formatted) = ColumnKind::BranchDiff
                .format_diff_plain(branch_diff.diff.added, branch_diff.diff.deleted)
        {
            segments.push(StatuslineSegment::from_column(
                format!("^{formatted}"),
                ColumnKind::BranchDiff,
            ));
        }

        // 6. Upstream status (priority 7)
        if let Some(ref upstream) = self.upstream
            && let Some(active) = upstream.active()
            && let Some(formatted) =
                ColumnKind::Upstream.format_diff_plain(active.ahead, active.behind)
        {
            segments.push(StatuslineSegment::from_column(
                formatted,
                ColumnKind::Upstream,
            ));
        }

        // 7. CI status (priority 9)
        if let Some(Some(ref pr_status)) = self.pr_status {
            segments.push(StatuslineSegment::from_column(
                pr_status.format_indicator(include_links),
                ColumnKind::CiStatus,
            ));
        }

        // 8. URL (priority 8)
        if let Some(ref url) = self.url {
            segments.push(StatuslineSegment::from_column(url.clone(), ColumnKind::Url));
        }

        segments
    }

    /// Populate display fields for JSON output and statusline.
    ///
    /// Call after all computed fields (counts, diffs, upstream, CI) are available.
    pub fn finalize_display(&mut self) {
        self.display =
            DisplayFields::from_common_fields(&self.counts, &self.branch_diff, &self.upstream);
        self.display.statusline = Some(self.format_statusline());

        if let ItemKind::Worktree(ref mut wt_data) = self.kind
            && let Some(ref working_tree_diff) = wt_data.working_tree_diff
        {
            wt_data.working_diff_display = ColumnKind::WorkingDiff
                .format_diff_plain(working_tree_diff.added, working_tree_diff.deleted);
        }
    }

    /// Compute status symbols for this item.
    ///
    /// This is idempotent and can be called multiple times as new data arrives.
    /// It will recompute with the latest available data.
    ///
    /// Branches get a subset of status symbols (no working tree changes or worktree attrs).
    pub(crate) fn compute_status_symbols(
        &mut self,
        default_branch: Option<&str>,
        has_merge_tree_conflicts: bool,
        user_marker: Option<String>,
        working_tree_status: Option<WorkingTreeStatus>,
        has_conflicts: bool,
    ) {
        // Common fields for both worktrees and branches
        let default_counts = AheadBehind::default();
        let default_upstream = UpstreamStatus::default();
        let counts = self.counts.as_ref().unwrap_or(&default_counts);
        let upstream = self.upstream.as_ref().unwrap_or(&default_upstream);
        let upstream_divergence = match upstream.active() {
            None => Divergence::None,
            Some(active) => Divergence::from_counts_with_remote(active.ahead, active.behind),
        };

        match &self.kind {
            ItemKind::Worktree(data) => {
                // Full status computation for worktrees

                // Worktree location state - priority: branch_worktree_mismatch > prunable > locked
                let worktree_state = if data.branch_worktree_mismatch {
                    WorktreeState::BranchWorktreeMismatch
                } else if data.is_prunable() {
                    WorktreeState::Prunable
                } else if data.locked.is_some() {
                    WorktreeState::Locked
                } else {
                    WorktreeState::None
                };

                // Operation state - priority: conflicts > rebase > merge
                let operation_state = if has_conflicts {
                    OperationState::Conflicts
                } else if data.git_operation == ActiveGitOperation::Rebase {
                    OperationState::Rebase
                } else if data.git_operation == ActiveGitOperation::Merge {
                    OperationState::Merge
                } else {
                    OperationState::None
                };

                // Check if content is integrated into main (safe to delete)
                let has_untracked = working_tree_status.is_some_and(|s| s.untracked);
                // is_clean requires working_tree_diff to be loaded AND empty, plus no untracked.
                // Don't assume clean when unknown to avoid premature integration state
                // (which would cause UI flash during progressive loading).
                let is_clean = data
                    .working_tree_diff
                    .as_ref()
                    .is_some_and(|d| d.is_empty())
                    && !has_untracked;
                let integration =
                    self.check_integration_state(data.is_main, default_branch, is_clean);

                // Separately detect SameCommit: same commit as main but with uncommitted work
                // This is NOT an integration state (has work that would be lost on delete)
                // Use ahead==0 && behind==0 (vs stats_base/main) to detect same commit
                let has_tracked_changes = data
                    .working_tree_diff
                    .as_ref()
                    .is_some_and(|d| !d.is_empty());
                let is_same_commit_dirty = counts.ahead == 0
                    && counts.behind == 0
                    && (has_tracked_changes || has_untracked);

                // Compute main state: combines is_main, would_conflict, integration, and divergence
                let main_state = MainState::from_integration_and_counts(
                    data.is_main,
                    has_merge_tree_conflicts,
                    integration,
                    is_same_commit_dirty,
                    self.is_orphan.unwrap_or(false),
                    counts.ahead,
                    counts.behind,
                );

                self.status_symbols = Some(StatusSymbols {
                    main_state,
                    operation_state,
                    worktree_state,
                    upstream_divergence,
                    working_tree: working_tree_status.unwrap_or_default(),
                    user_marker,
                });
            }
            ItemKind::Branch => {
                // Simplified status computation for branches
                // Only compute symbols that apply to branches (no working tree, git operation, or worktree attrs)

                // Branches don't have working trees, so always clean
                let integration = self.check_integration_state(
                    false, // branches are never main worktree
                    default_branch,
                    true, // branches are always clean (no working tree)
                );

                // Compute main state
                // Branches can't have is_same_commit_dirty (no working tree)
                let main_state = MainState::from_integration_and_counts(
                    false, // not main
                    has_merge_tree_conflicts,
                    integration,
                    false, // branches have no working tree, can't be dirty
                    self.is_orphan.unwrap_or(false),
                    counts.ahead,
                    counts.behind,
                );

                self.status_symbols = Some(StatusSymbols {
                    main_state,
                    operation_state: OperationState::None,
                    worktree_state: WorktreeState::Branch,
                    upstream_divergence,
                    working_tree: WorkingTreeStatus::default(),
                    user_marker,
                });
            }
        }
    }

    /// Check if branch content is integrated into the default branch (safe to delete).
    ///
    /// Returns `Some(MainState)` only for truly integrated states:
    /// - `Empty` = same commit as default branch with clean working tree
    /// - `Integrated(...)` = content in default branch via different history
    ///
    /// Does NOT detect `SameCommit` (same commit with dirty working tree) -
    /// that's handled separately in the caller since it's not an integration state.
    fn check_integration_state(
        &self,
        is_main: bool,
        default_branch: Option<&str>,
        is_clean: bool,
    ) -> Option<MainState> {
        if is_main || default_branch.is_none() {
            return None;
        }

        // Only show integration state if working tree is clean.
        // Dirty working tree means there's work that would be lost on removal.
        if !is_clean {
            return None;
        }

        // Compute is_same_commit from ahead/behind counts (vs stats_base/main)
        // This detects "same commit as main" for the _ symbol
        let is_same_commit = self.counts.as_ref().map(|c| c.ahead == 0 && c.behind == 0);

        // Use the shared integration check (same logic as wt remove)
        let signals = IntegrationSignals {
            is_same_commit,
            is_ancestor: self.is_ancestor,
            has_added_changes: self.has_file_changes,
            trees_match: self.committed_trees_match,
            would_merge_add: self.would_merge_add,
        };
        let reason = check_integration(&signals);

        // Convert to MainState, with SameCommit becoming Empty for display
        match reason {
            Some(IntegrationReason::SameCommit) => Some(MainState::Empty),
            Some(other) => Some(MainState::Integrated(other)),
            None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_item_branch_name() {
        let item = ListItem::new_branch("abc123".to_string(), "feature".to_string());
        assert_eq!(item.branch_name(), "feature");

        let mut item = ListItem::new_branch("abc123".to_string(), "feature".to_string());
        item.branch = None; // Simulate detached
        assert_eq!(item.branch_name(), "(detached)");
    }

    #[test]
    fn test_list_item_head() {
        let item = ListItem::new_branch("abc123def".to_string(), "feature".to_string());
        assert_eq!(item.head(), "abc123def");
    }

    #[test]
    fn test_list_item_counts() {
        // New items have no counts computed yet
        let item = ListItem::new_branch("abc123".to_string(), "feature".to_string());
        assert!(item.counts.is_none());

        // After setting counts, they're accessible
        let mut item = ListItem::new_branch("abc123".to_string(), "feature".to_string());
        item.counts = Some(AheadBehind {
            ahead: 5,
            behind: 3,
        });
        let counts = item.counts.unwrap();
        assert_eq!(counts.ahead, 5);
        assert_eq!(counts.behind, 3);
    }

    #[test]
    fn test_list_item_branch_diff() {
        let item = ListItem::new_branch("abc123".to_string(), "feature".to_string());
        // New items have no branch_diff computed yet
        assert!(item.branch_diff().is_none());
    }

    #[test]
    fn test_list_item_upstream() {
        let item = ListItem::new_branch("abc123".to_string(), "feature".to_string());
        let upstream = item.upstream();
        assert!(upstream.remote.is_none());
    }

    #[test]
    fn test_list_item_worktree_data() {
        // Branch item has no worktree data
        let item = ListItem::new_branch("abc123".to_string(), "feature".to_string());
        assert!(item.worktree_data().is_none());
        assert!(item.worktree_path().is_none());
    }

    #[test]
    fn test_list_item_should_dim() {
        // No status_symbols = should NOT dim (data still loading)
        let item = ListItem::new_branch("abc123".to_string(), "feature".to_string());
        assert!(!item.should_dim());
    }

    #[test]
    fn test_check_integration_state_requires_clean() {
        // Integration checks require is_clean to avoid marking worktrees with
        // uncommitted changes as integrated (which would incorrectly suggest
        // they're safe to remove).

        // Create a minimal ListItem for testing - set committed_trees_match = true
        let mut item = ListItem::new_branch("abc123".to_string(), "feature".to_string());
        item.is_ancestor = Some(false); // not an ancestor (to skip priority 1-2)
        item.committed_trees_match = Some(true); // trees match (priority 4)
        item.has_file_changes = None; // unknown (to skip priority 3)
        item.would_merge_add = None; // unknown (to skip priority 6)

        // Dirty working tree: should NOT return Integrated
        assert_eq!(
            item.check_integration_state(
                false,        // not main
                Some("main"), // has default branch
                false,        // is_clean = false (dirty working tree)
            ),
            None,
            "Integration should reject dirty working tree"
        );

        // Clean working tree: SHOULD return Integrated(TreesMatch)
        assert_eq!(
            item.check_integration_state(
                false,
                Some("main"),
                true, // is_clean = true
            ),
            Some(MainState::Integrated(IntegrationReason::TreesMatch)),
            "Integration should accept clean working tree with matching trees"
        );
    }

    #[test]
    fn test_check_integration_state_untracked_blocks_integration() {
        // When is_clean is computed at the call site, untracked files make is_clean=false.
        // This test verifies that is_clean=false blocks integration, which is what happens
        // when there are untracked files.

        let mut item = ListItem::new_branch("abc123".to_string(), "feature".to_string());
        item.is_ancestor = Some(false);
        item.committed_trees_match = Some(true); // trees match (would show integration if clean)
        item.has_file_changes = None;
        item.would_merge_add = None;

        // is_clean=false (as computed when untracked files exist): should NOT return Integrated
        assert_eq!(
            item.check_integration_state(
                false,
                Some("main"),
                false, // is_clean = false (represents untracked files blocking integration)
            ),
            None,
            "Dirty working tree (untracked files) should block integration"
        );

        // is_clean=true: SHOULD return Integrated
        assert_eq!(
            item.check_integration_state(
                false,
                Some("main"),
                true, // is_clean = true
            ),
            Some(MainState::Integrated(IntegrationReason::TreesMatch)),
            "Clean working tree should show as integrated"
        );
    }
}
