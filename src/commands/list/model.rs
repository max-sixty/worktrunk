use std::path::PathBuf;
use worktrunk::git::{IntegrationReason, LineDiff, PrecomputedIntegration, check_integration};

use super::ci_status::PrStatus;
use super::columns::ColumnKind;

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
        _pr_status: &Option<Option<PrStatus>>,
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

        // CI column shows only the indicator (●), not text
        // Let render.rs handle it via render_indicator()
        let ci_status_display = None;

        Self {
            commits_display,
            branch_diff_display,
            upstream_display,
            ci_status_display,
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
    /// Diff between working tree and main branch.
    /// `None` means "not computed yet" or "not computed" (optimization: skipped when trees differ).
    /// `Some(Some((0, 0)))` means working tree matches main exactly.
    /// `Some(Some((a, d)))` means a lines added, d deleted vs main.
    /// `Some(None)` means computation was skipped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_tree_diff_with_main: Option<Option<LineDiff>>,
    /// Git operation in progress (rebase/merge)
    #[serde(skip_serializing_if = "git_operation_is_none")]
    pub git_operation: GitOperationState,
    pub is_main: bool,
    /// Whether this is the current worktree (matches $PWD)
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_current: bool,
    /// Whether this was the previous worktree (from WT_PREVIOUS_BRANCH)
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_previous: bool,
    /// Whether the worktree path doesn't match what the template would generate.
    /// Only true when: has branch name, not main worktree, and path differs from template.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub path_mismatch: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_diff_display: Option<String>,
}

impl WorktreeData {
    /// Create WorktreeData from a Worktree, with all computed fields set to None.
    pub(crate) fn from_worktree(
        wt: &worktrunk::git::Worktree,
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

#[derive(serde::Serialize, Clone, Default, Debug)]
pub struct CommitDetails {
    pub timestamp: i64,
    pub commit_message: String,
}

#[derive(serde::Serialize, Default, Copy, Clone, Debug)]
pub struct AheadBehind {
    pub ahead: usize,
    pub behind: usize,
}

#[derive(serde::Serialize, Default, Copy, Clone, Debug)]
pub struct BranchDiffTotals {
    #[serde(rename = "branch_diff")]
    pub diff: LineDiff,
}

#[derive(serde::Serialize, Default, Clone, Debug)]
pub struct UpstreamStatus {
    #[serde(rename = "upstream_remote")]
    pub(super) remote: Option<String>,
    #[serde(rename = "upstream_ahead")]
    pub(super) ahead: usize,
    #[serde(rename = "upstream_behind")]
    pub(super) behind: usize,
}

/// Active upstream tracking information
pub struct ActiveUpstream<'a> {
    pub remote: &'a str,
    pub ahead: usize,
    pub behind: usize,
}

impl UpstreamStatus {
    pub fn active(&self) -> Option<ActiveUpstream<'_>> {
        self.remote.as_deref().map(|remote| ActiveUpstream {
            remote,
            ahead: self.ahead,
            behind: self.behind,
        })
    }

    #[cfg(test)]
    pub(crate) fn from_parts(remote: Option<String>, ahead: usize, behind: usize) -> Self {
        Self {
            remote,
            ahead,
            behind,
        }
    }
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
    /// Whether HEAD's tree SHA matches main's tree SHA.
    /// True when committed content is identical regardless of commit history.
    /// Internal field used to compute `BranchState::Integrated(TreesMatch)`.
    #[serde(skip)]
    pub committed_trees_match: Option<bool>,
    /// Whether branch has file changes beyond the merge-base with main.
    /// False when three-dot diff (`main...branch`) is empty.
    /// Internal field used for integration detection (no unique content).
    #[serde(skip)]
    pub has_file_changes: Option<bool>,
    /// Whether merging branch into main would add changes (merge simulation).
    /// False when `git merge-tree --write-tree main branch` produces same tree as main.
    /// Catches squash-merged branches where main advanced.
    #[serde(skip)]
    pub would_merge_add: Option<bool>,
    /// Whether branch HEAD is an ancestor of main (or same commit).
    /// True means branch is already part of main's history.
    /// This is the cheapest integration check (~1ms).
    #[serde(skip)]
    pub is_ancestor: Option<bool>,

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

pub struct ListData {
    pub items: Vec<ListItem>,
    /// Path to the main worktree, used for computing relative paths in display.
    /// Currently only read by `select` command (unix-only).
    #[cfg_attr(windows, allow(dead_code))]
    pub main_worktree_path: std::path::PathBuf,
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
            upstream: None,
            pr_status: None,
            url: None,
            url_active: None,
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

    pub fn commit_details(&self) -> CommitDetails {
        self.commit.clone().unwrap_or_default()
    }

    pub fn counts(&self) -> AheadBehind {
        self.counts.unwrap_or_default()
    }

    pub fn branch_diff(&self) -> BranchDiffTotals {
        self.branch_diff.unwrap_or_default()
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
    /// - `Some(true)` - confirmed removable (branch integrated into main)
    /// - `Some(false)` - confirmed not removable (has unique work)
    /// - `None` - data still loading, cannot determine yet
    ///
    /// Checks (in order):
    /// 1. **Same commit** - branch HEAD is ancestor of main or same commit.
    ///    The branch is already part of main's history.
    /// 2. **No file changes** - three-dot diff (`main...branch`) is empty.
    ///    Catches squash-merged branches where commits exist but add no files.
    /// 3. **Tree matches main** - tree SHA equals main's tree SHA.
    ///    Catches rebased/squash-merged branches with identical content.
    /// 4. **Merge simulation** - merging branch into main wouldn't change main's
    ///    tree. Catches squash-merged branches where main has advanced.
    /// 5. **Working tree matches main** (worktrees only) - uncommitted changes
    ///    don't diverge from main.
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
        let mut parts: Vec<String> = Vec::new();

        // 1. Branch name
        parts.push(self.branch_name().to_string());

        // 2. Status symbols (compact, no grid alignment)
        if let Some(ref symbols) = self.status_symbols {
            let status = symbols.format_compact();
            if !status.is_empty() {
                parts.push(status);
            }
        }

        // 3. Working diff (worktrees only)
        // Prefix with @ ("at" current state) to distinguish from branch diff (^)
        if let Some(data) = self.worktree_data()
            && let Some(ref diff) = data.working_tree_diff
            && !diff.is_empty()
            && let Some(formatted) =
                ColumnKind::WorkingDiff.format_diff_plain(diff.added, diff.deleted)
        {
            parts.push(format!("@{formatted}"));
        }

        // 4. Commits ahead/behind main
        if let Some(formatted) =
            ColumnKind::AheadBehind.format_diff_plain(self.counts().ahead, self.counts().behind)
        {
            parts.push(formatted);
        }

        // 5. Branch diff vs main (line changes)
        // Prefix with ^ (main) to distinguish from working diff (@)
        let branch_diff = self.branch_diff();
        if !branch_diff.diff.is_empty()
            && let Some(formatted) = ColumnKind::BranchDiff
                .format_diff_plain(branch_diff.diff.added, branch_diff.diff.deleted)
        {
            parts.push(format!("^{formatted}"));
        }

        // 6. Upstream status
        if let Some(ref upstream) = self.upstream
            && let Some(active) = upstream.active()
            && let Some(formatted) =
                ColumnKind::Upstream.format_diff_plain(active.ahead, active.behind)
        {
            parts.push(formatted);
        }

        // 7. CI status
        if let Some(Some(ref pr_status)) = self.pr_status {
            parts.push(pr_status.format_indicator_with_options(include_links));
        }

        parts.join("  ")
    }

    /// Populate display fields for JSON output and statusline.
    ///
    /// Call after all computed fields (counts, diffs, upstream, CI) are available.
    pub fn finalize_display(&mut self) {
        use super::columns::ColumnKind;

        self.display = DisplayFields::from_common_fields(
            &self.counts,
            &self.branch_diff,
            &self.upstream,
            &self.pr_status,
        );
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

                // Worktree location state - priority: path_mismatch > prunable > locked
                let worktree_state = if data.path_mismatch {
                    WorktreeState::PathMismatch
                } else if data.prunable.is_some() {
                    WorktreeState::Prunable
                } else if data.locked.is_some() {
                    WorktreeState::Locked
                } else {
                    WorktreeState::None
                };

                // Operation state - priority: conflicts > rebase > merge
                let operation_state = if has_conflicts {
                    OperationState::Conflicts
                } else if data.git_operation == GitOperationState::Rebase {
                    OperationState::Rebase
                } else if data.git_operation == GitOperationState::Merge {
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
                let working_tree_matches_main = data
                    .working_tree_diff_with_main
                    .as_ref()
                    .and_then(|opt| opt.as_ref())
                    .is_some_and(|diff| diff.is_empty());
                let integration = self.check_integration_state(
                    data.is_main,
                    default_branch,
                    is_clean,
                    working_tree_matches_main,
                );

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
                    true,  // branches are always clean (no working tree)
                    false, // no working tree diff with main for branches
                );

                // Compute main state
                // Branches can't have is_same_commit_dirty (no working tree)
                let main_state = MainState::from_integration_and_counts(
                    false, // not main
                    has_merge_tree_conflicts,
                    integration,
                    false, // branches have no working tree, can't be dirty
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

    /// Check if branch content is integrated into main (safe to delete).
    ///
    /// Returns `Some(MainState)` only for truly integrated states:
    /// - `Empty` = same commit as main with clean working tree
    /// - `Integrated(...)` = content in main via different history
    ///
    /// Does NOT detect `SameCommit` (same commit with dirty working tree) -
    /// that's handled separately in the caller since it's not an integration state.
    fn check_integration_state(
        &self,
        is_main: bool,
        default_branch: Option<&str>,
        is_clean: bool,
        working_tree_matches_main: bool,
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
        let is_same_commit = self
            .counts
            .as_ref()
            .is_some_and(|c| c.ahead == 0 && c.behind == 0);

        // Use the shared integration check (same logic as wt remove)
        let mut provider = PrecomputedIntegration {
            is_same_commit,
            is_ancestor: self.is_ancestor.unwrap_or(false),
            has_added_changes: self.has_file_changes.unwrap_or(true), // default: assume has changes
            trees_match: self.committed_trees_match.unwrap_or(false),
            would_merge_add: self.would_merge_add.unwrap_or(true), // default: assume would add
        };
        let reason = check_integration(&mut provider);

        // Additional check for wt list: working tree (with uncommitted changes) matches main.
        // This is list-specific because wt remove requires a clean working tree anyway.
        let reason = reason.or(working_tree_matches_main.then_some(IntegrationReason::TreesMatch));

        // Convert to MainState, with SameCommit becoming Empty for display
        match reason {
            Some(IntegrationReason::SameCommit) => Some(MainState::Empty),
            Some(other) => Some(MainState::Integrated(other)),
            None => None,
        }
    }
}

/// Upstream divergence state relative to remote tracking branch.
///
/// Used only for upstream/remote divergence. Main branch divergence is now
/// handled by [`MainState`] which combines divergence with integration states.
///
/// | Variant   | Symbol |
/// |-----------|--------|
/// | None      | (empty) - no remote configured |
/// | InSync    | `\|`   - up-to-date with remote |
/// | Ahead     | `⇡`    - has unpushed commits   |
/// | Behind    | `⇣`    - missing remote commits |
/// | Diverged  | `⇅`    - both ahead and behind  |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Divergence {
    /// No remote tracking branch configured
    #[default]
    None,
    /// In sync with upstream remote
    InSync,
    /// Has commits the remote doesn't have
    Ahead,
    /// Missing commits from the remote
    Behind,
    /// Both ahead and behind the remote
    Diverged,
}

impl Divergence {
    /// Compute divergence state when a remote tracking branch exists.
    ///
    /// Returns `InSync` for 0/0 since we know a remote exists.
    /// For cases where there's no remote, use `Divergence::None` directly.
    pub fn from_counts_with_remote(ahead: usize, behind: usize) -> Self {
        match (ahead, behind) {
            (0, 0) => Self::InSync,
            (_, 0) => Self::Ahead,
            (0, _) => Self::Behind,
            _ => Self::Diverged,
        }
    }

    /// Get the display symbol for this divergence state.
    pub fn symbol(self) -> &'static str {
        match self {
            Self::None => "",
            Self::InSync => "|",
            Self::Ahead => "⇡",
            Self::Behind => "⇣",
            Self::Diverged => "⇅",
        }
    }

    /// Returns styled symbol (dimmed), or None for None variant.
    pub fn styled(self) -> Option<String> {
        use color_print::cformat;
        if self == Self::None {
            None
        } else {
            Some(cformat!("<dim>{}</>", self.symbol()))
        }
    }
}

/// Worktree state indicator
///
/// Shows the "location" state of a worktree or branch:
/// - For worktrees: whether the path matches the template, or has issues
/// - For branches (without worktree): shows / to distinguish from worktrees
///
/// Priority order for worktrees: PathMismatch > Prunable > Locked
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, strum::IntoStaticStr)]
pub enum WorktreeState {
    #[strum(serialize = "")]
    /// Normal worktree (path matches template, not locked or prunable)
    #[default]
    None,
    /// Path doesn't match what the template would generate (red flag = "not at home")
    PathMismatch,
    /// Prunable (worktree directory missing)
    Prunable,
    /// Locked (protected from removal)
    Locked,
    /// Branch indicator (for branches without worktrees)
    Branch,
}

impl std::fmt::Display for WorktreeState {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::None => Ok(()),
            Self::PathMismatch => write!(f, "⚑"),
            Self::Prunable => write!(f, "⊟"),
            Self::Locked => write!(f, "⊞"),
            Self::Branch => write!(f, "/"),
        }
    }
}

impl serde::Serialize for WorktreeState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

/// Main branch relationship state
///
/// Represents the combined relationship to the main branch in a single position.
/// Uses horizontal arrows (vs vertical arrows for Remote column).
///
/// Priority order determines which symbol is shown:
/// 1. IsMain (^) - this IS the main branch
/// 2. WouldConflict (✗) - merge-tree simulation shows conflicts
/// 3. Empty (_) - same commit as main AND clean working tree (safe to delete)
/// 4. SameCommit (–) - same commit as main with uncommitted changes
/// 5. Integrated (⊂) - content is in main via different history
/// 6. Diverged (↕) - both ahead and behind main
/// 7. Ahead (↑) - has commits main doesn't have
/// 8. Behind (↓) - missing commits from main
///
/// The `Integrated` variant carries an [`IntegrationReason`] explaining how the
/// content was integrated (ancestor, trees match, no added changes, or merge adds nothing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MainState {
    /// Normal working branch (up-to-date with main, no special state)
    #[default]
    None,
    /// This IS the main branch
    IsMain,
    /// Merge-tree conflicts with main (simulated via git merge-tree)
    WouldConflict,
    /// Branch HEAD is same commit as main AND working tree is clean (safe to delete)
    Empty,
    /// Branch HEAD is same commit as main but has uncommitted changes
    SameCommit,
    /// Content is integrated into main via different history
    Integrated(IntegrationReason),
    /// Both ahead and behind main
    Diverged,
    /// Has commits main doesn't have
    Ahead,
    /// Missing commits from main
    Behind,
}

impl std::fmt::Display for MainState {
    /// Single-stroke vertical arrows for Main column (vs double-stroke arrows for Remote column).
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::None => Ok(()),
            Self::IsMain => write!(f, "^"),
            Self::WouldConflict => write!(f, "✗"),
            Self::Empty => write!(f, "_"),
            Self::SameCommit => write!(f, "–"), // en-dash U+2013
            Self::Integrated(_) => write!(f, "⊂"),
            Self::Diverged => write!(f, "↕"),
            Self::Ahead => write!(f, "↑"),
            Self::Behind => write!(f, "↓"),
        }
    }
}

impl MainState {
    /// Returns styled symbol with appropriate color, or None for None variant.
    ///
    /// Color semantics:
    /// - WARNING (yellow): WouldConflict - potential problem needing attention
    /// - HINT (dimmed): All others - informational states
    pub fn styled(&self) -> Option<String> {
        use color_print::cformat;
        match self {
            Self::None => None,
            Self::WouldConflict => Some(cformat!("<yellow>{self}</>")),
            _ => Some(cformat!("<dim>{self}</>")),
        }
    }

    /// Returns the integration reason if this is an integrated state, None otherwise.
    pub fn integration_reason(&self) -> Option<IntegrationReason> {
        match self {
            Self::Integrated(reason) => Some(*reason),
            _ => None,
        }
    }

    /// Returns the JSON string representation for main_state field.
    pub fn as_json_str(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::IsMain => Some("is_main"),
            Self::WouldConflict => Some("would_conflict"),
            Self::Empty => Some("empty"),
            Self::SameCommit => Some("same_commit"),
            Self::Integrated(_) => Some("integrated"),
            Self::Diverged => Some("diverged"),
            Self::Ahead => Some("ahead"),
            Self::Behind => Some("behind"),
        }
    }

    /// Compute from divergence counts, integration state, and same-commit-dirty flag.
    ///
    /// Priority: IsMain > WouldConflict > integration > SameCommit > Diverged > Ahead > Behind
    pub fn from_integration_and_counts(
        is_main: bool,
        would_conflict: bool,
        integration: Option<MainState>,
        is_same_commit_dirty: bool,
        ahead: usize,
        behind: usize,
    ) -> Self {
        if is_main {
            Self::IsMain
        } else if would_conflict {
            Self::WouldConflict
        } else if let Some(state) = integration {
            state
        } else if is_same_commit_dirty {
            Self::SameCommit
        } else {
            match (ahead, behind) {
                (0, 0) => Self::None,
                (_, 0) => Self::Ahead,
                (0, _) => Self::Behind,
                _ => Self::Diverged,
            }
        }
    }
}

impl serde::Serialize for MainState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

/// Worktree operation state
///
/// Represents blocking git operations in progress that require resolution.
/// These take priority over all other states in the Worktree column.
///
/// Priority: Conflicts (✘) > Rebase (⤴) > Merge (⤵)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OperationState {
    /// No operation in progress
    #[default]
    None,
    /// Actual merge conflicts (unmerged paths in working tree)
    Conflicts,
    /// Rebase in progress
    Rebase,
    /// Merge in progress
    Merge,
}

impl std::fmt::Display for OperationState {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::None => Ok(()),
            Self::Conflicts => write!(f, "✘"),
            Self::Rebase => write!(f, "⤴"),
            Self::Merge => write!(f, "⤵"),
        }
    }
}

impl OperationState {
    /// Returns styled symbol with appropriate color, or None for None variant.
    ///
    /// Color semantics:
    /// - ERROR (red): Conflicts - blocking problems
    /// - WARNING (yellow): Rebase, Merge - active/stuck states
    pub fn styled(&self) -> Option<String> {
        use color_print::cformat;
        match self {
            Self::None => None,
            Self::Conflicts => Some(cformat!("<red>{self}</>")),
            Self::Rebase | Self::Merge => Some(cformat!("<yellow>{self}</>")),
        }
    }

    /// Returns the JSON string representation.
    pub fn as_json_str(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Conflicts => Some("conflicts"),
            Self::Rebase => Some("rebase"),
            Self::Merge => Some("merge"),
        }
    }
}

impl serde::Serialize for OperationState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

/// Git operation state for a worktree
///
/// Represents whether a worktree is in the middle of a git operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, strum::IntoStaticStr)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum GitOperationState {
    #[strum(serialize = "")]
    #[serde(rename = "")]
    #[default]
    None,
    /// Rebase in progress (rebase-merge or rebase-apply directory exists)
    Rebase,
    /// Merge in progress (MERGE_HEAD exists)
    Merge,
}

/// Tracks which status symbol positions are actually used across all items
/// and the maximum width needed for each position.
///
/// This allows the Status column to:
/// 1. Only allocate space for positions that have data
/// 2. Pad each position to a consistent width for vertical alignment
///
/// Stores maximum character width for each of 7 positions (including user marker).
/// A width of 0 means the position is unused.
#[derive(Debug, Clone, Copy, Default)]
pub struct PositionMask {
    /// Maximum width for each position: [0, 1, 2, 3, 4, 5, 6]
    /// 0 = position unused, >0 = max characters needed
    widths: [usize; 7],
}

impl PositionMask {
    // Render order indices (0-6) - symbols appear in this order left-to-right
    // Working tree split into 3 fixed positions for vertical alignment
    const STAGED: usize = 0; // + (staged changes)
    const MODIFIED: usize = 1; // ! (modified files)
    const UNTRACKED: usize = 2; // ? (untracked files)
    const WORKTREE_STATE: usize = 3; // Worktree: ✘⤴⤵/⚑⊟⊞
    const MAIN_STATE: usize = 4; // Main relationship: ^✗_⊂↕↑↓
    const UPSTREAM_DIVERGENCE: usize = 5; // Remote: |⇅⇡⇣
    const USER_MARKER: usize = 6;

    /// Full mask with all positions enabled (for JSON output and progressive rendering)
    /// Allocates realistic widths based on common symbol sizes to ensure proper grid alignment
    pub const FULL: Self = Self {
        widths: [
            1, // STAGED: + (1 char)
            1, // MODIFIED: ! (1 char)
            1, // UNTRACKED: ? (1 char)
            1, // WORKTREE_STATE: ✘⤴⤵/⚑⊟⊞ (1 char, priority: conflicts > rebase > merge > path_mismatch > prunable > locked > branch)
            1, // MAIN_STATE: ^✗_–⊂↕↑↓ (1 char, priority: is_main > would_conflict > empty > same_commit > integrated > diverged > ahead > behind)
            1, // UPSTREAM_DIVERGENCE: |⇡⇣⇅ (1 char)
            2, // USER_MARKER: single emoji or two chars (allocate 2)
        ],
    };

    /// Get the allocated width for a position
    pub(crate) fn width(&self, pos: usize) -> usize {
        self.widths[pos]
    }
}

/// Structured status symbols for aligned rendering
///
/// Symbols are categorized to enable vertical alignment in table output.
/// Display order (left to right):
/// - Working tree: +, !, ? (staged, modified, untracked - NOT mutually exclusive)
/// - Worktree state: ✘, ⤴, ⤵, /, ⚑, ⊟, ⊞ (operations + location)
/// - Main state: ^, ✗, _, ⊂, ↕, ↑, ↓ (relationship to main branch - single-stroke vertical arrows)
/// - Upstream divergence: |, ⇅, ⇡, ⇣ (relationship to remote - vertical arrows)
/// - User marker: custom labels, emoji
///
/// ## Mutual Exclusivity
///
/// **Worktree state (operations take priority over location):**
/// Priority: ✘ > ⤴ > ⤵ > ⚑ > ⊟ > ⊞ > /
/// - ✘: Actual conflicts (must resolve)
/// - ⤴: Rebase in progress
/// - ⤵: Merge in progress
/// - ⚑: Worktree path doesn't match branch name
/// - ⊟: Prunable (directory missing)
/// - ⊞: Locked worktree
/// - /: Branch without worktree
///
/// **Main state (single position with priority):**
/// Priority: ^ > ✗ > _ > – > ⊂ > ↕ > ↑ > ↓
/// - ^: This IS the main branch
/// - ✗: Would conflict if merged to main
/// - _: Same commit as main, clean working tree (removable)
/// - –: Same commit as main, uncommitted changes (NOT removable)
/// - ⊂: Content integrated (removable)
/// - ↕: Diverged from main
/// - ↑: Ahead of main
/// - ↓: Behind main
///
/// **Upstream divergence (enforced by type system):**
/// - |: In sync with remote
/// - ⇅: Diverged from remote
/// - ⇡: Ahead of remote
/// - ⇣: Behind remote
///
/// **NOT mutually exclusive (can co-occur):**
/// - Working tree symbols (+!?): Can have multiple types of changes
#[derive(Debug, Clone, Default)]
pub struct StatusSymbols {
    /// Main branch relationship state (single position, horizontal arrows)
    /// Priority: IsMain (^) > WouldConflict (✗) > Empty (_) > SameCommit (–) > Integrated (⊂) > Diverged (↕) > Ahead (↑) > Behind (↓)
    pub(crate) main_state: MainState,

    /// Worktree operation and location state (single position)
    /// Operations (✘⤴⤵) take priority over location states (/⚑⊟⊞)
    pub(crate) operation_state: OperationState,

    /// Worktree location state: / for branches, ⚑⊟⊞ for worktrees
    pub(crate) worktree_state: WorktreeState,

    /// Remote/upstream divergence state (mutually exclusive)
    pub(crate) upstream_divergence: Divergence,

    /// Working tree changes (NOT mutually exclusive, can have multiple)
    pub(crate) working_tree: WorkingTreeStatus,

    /// User-defined status annotation (custom labels, e.g., 💬, 🤖)
    pub(crate) user_marker: Option<String>,
}

impl StatusSymbols {
    /// Render symbols with selective alignment based on position mask
    ///
    /// Only includes positions present in the mask. This ensures vertical
    /// scannability - each symbol type appears at the same column position
    /// across all rows, while minimizing wasted space.
    ///
    /// See [`StatusSymbols`] struct doc for symbol categories.
    pub fn render_with_mask(&self, mask: &PositionMask) -> String {
        use worktrunk::styling::StyledLine;

        let mut result = String::with_capacity(64);

        if self.is_empty() {
            return result;
        }

        // Grid-based rendering: each position gets a fixed width for vertical alignment.
        // CRITICAL: Always use PositionMask::FULL for consistent spacing between progressive and final rendering.
        // The mask provides the maximum width needed for each position across all rows.
        // Accept wider Status column with whitespace as tradeoff for perfect alignment.
        for (pos, styled_content, has_data) in self.styled_symbols() {
            let allocated_width = mask.width(pos);

            if has_data {
                // Use StyledLine to handle width calculation (strips ANSI codes automatically)
                let mut segment = StyledLine::new();
                segment.push_raw(styled_content);
                segment.pad_to(allocated_width);
                result.push_str(&segment.render());
            } else {
                // Fill empty position with spaces for alignment
                for _ in 0..allocated_width {
                    result.push(' ');
                }
            }
        }

        result
    }

    /// Check if symbols are empty
    pub fn is_empty(&self) -> bool {
        self.main_state == MainState::None
            && self.operation_state == OperationState::None
            && self.worktree_state == WorktreeState::None
            && self.upstream_divergence == Divergence::None
            && !self.working_tree.is_dirty()
            && self.user_marker.is_none()
    }

    /// Render status symbols in compact form for statusline (no grid alignment).
    ///
    /// Uses the same styled symbols as `render_with_mask()`, just without padding.
    pub fn format_compact(&self) -> String {
        self.styled_symbols()
            .into_iter()
            .filter_map(|(_, styled, has_data)| has_data.then_some(styled))
            .collect()
    }

    /// Build styled symbols array with position indices.
    ///
    /// Returns: `[(position_mask, styled_string, has_data); 7]`
    ///
    /// Order: working_tree (+!?) → main_state → upstream_divergence → worktree_state → user_marker
    ///
    /// Styling follows semantic meaning:
    /// - Cyan: Working tree changes (activity indicator)
    /// - Red: Conflicts (blocking problems)
    /// - Yellow: Git operations, would_conflict, locked/prunable (states needing attention)
    /// - Dimmed: Main state symbols, divergence arrows, branch indicator (informational)
    fn styled_symbols(&self) -> [(usize, String, bool); 7] {
        use color_print::cformat;

        // Working tree symbols split into 3 fixed columns for vertical alignment
        let style_working = |has: bool, sym: char| -> (String, bool) {
            if has {
                (cformat!("<cyan>{sym}</>"), true)
            } else {
                (String::new(), false)
            }
        };
        let (staged_str, has_staged) = style_working(self.working_tree.staged, '+');
        let (modified_str, has_modified) = style_working(self.working_tree.modified, '!');
        let (untracked_str, has_untracked) = style_working(self.working_tree.untracked, '?');

        // Main state (merged column: ^✗_⊂↕↑↓)
        let (main_state_str, has_main_state) = self
            .main_state
            .styled()
            .map_or((String::new(), false), |s| (s, true));

        // Upstream divergence (|⇅⇡⇣)
        let (upstream_divergence_str, has_upstream_divergence) = self
            .upstream_divergence
            .styled()
            .map_or((String::new(), false), |s| (s, true));

        // Worktree state: operations (✘⤴⤵) take priority over location (/⚑⊟⊞)
        let (worktree_str, has_worktree) = if self.operation_state != OperationState::None {
            // Operation state takes priority
            (self.operation_state.styled().unwrap_or_default(), true)
        } else {
            // Fall back to location state
            match self.worktree_state {
                WorktreeState::None => (String::new(), false),
                // Branch indicator (/) is informational (dimmed)
                WorktreeState::Branch => (cformat!("<dim>{}</>", self.worktree_state), true),
                // Path mismatch (⚑) is a stronger warning (red)
                WorktreeState::PathMismatch => (cformat!("<red>{}</>", self.worktree_state), true),
                // Other worktree attrs (⊟⊞) are warnings (yellow)
                _ => (cformat!("<yellow>{}</>", self.worktree_state), true),
            }
        };

        let user_marker_str = self.user_marker.as_deref().unwrap_or("").to_string();

        // CRITICAL: Display order must match position indices for correct rendering.
        // Order: Working tree (0-2) → Worktree (3) → Main (4) → Remote (5) → User (6)
        [
            (PositionMask::STAGED, staged_str, has_staged),
            (PositionMask::MODIFIED, modified_str, has_modified),
            (PositionMask::UNTRACKED, untracked_str, has_untracked),
            (PositionMask::WORKTREE_STATE, worktree_str, has_worktree),
            (PositionMask::MAIN_STATE, main_state_str, has_main_state),
            (
                PositionMask::UPSTREAM_DIVERGENCE,
                upstream_divergence_str,
                has_upstream_divergence,
            ),
            (
                PositionMask::USER_MARKER,
                user_marker_str,
                self.user_marker.is_some(),
            ),
        ]
    }
}

/// Working tree changes as structured booleans
///
/// This is the canonical internal representation. Display strings are derived from this.
#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub struct WorkingTreeStatus {
    pub staged: bool,
    pub modified: bool,
    pub untracked: bool,
    pub renamed: bool,
    pub deleted: bool,
}

impl WorkingTreeStatus {
    /// Create from git status parsing results
    pub fn new(
        staged: bool,
        modified: bool,
        untracked: bool,
        renamed: bool,
        deleted: bool,
    ) -> Self {
        Self {
            staged,
            modified,
            untracked,
            renamed,
            deleted,
        }
    }

    /// Returns true if any changes are present
    pub fn is_dirty(&self) -> bool {
        self.staged || self.modified || self.untracked || self.renamed || self.deleted
    }

    /// Format as display string for JSON serialization and raw output (e.g., "+!?").
    ///
    /// For styled terminal rendering, use `StatusSymbols::styled_symbols()` instead.
    pub fn to_symbols(self) -> String {
        let mut s = String::with_capacity(5);
        if self.staged {
            s.push('+');
        }
        if self.modified {
            s.push('!');
        }
        if self.untracked {
            s.push('?');
        }
        if self.renamed {
            s.push('»');
        }
        if self.deleted {
            s.push('✘');
        }
        s
    }
}

/// Helper for serde skip_serializing_if
fn git_operation_is_none(state: &GitOperationState) -> bool {
    *state == GitOperationState::None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_working_tree_status_is_dirty() {
        // Empty status is not dirty
        assert!(!WorkingTreeStatus::default().is_dirty());

        // Each flag individually makes it dirty
        assert!(WorkingTreeStatus::new(true, false, false, false, false).is_dirty());
        assert!(WorkingTreeStatus::new(false, true, false, false, false).is_dirty());
        assert!(WorkingTreeStatus::new(false, false, true, false, false).is_dirty());
        assert!(WorkingTreeStatus::new(false, false, false, true, false).is_dirty());
        assert!(WorkingTreeStatus::new(false, false, false, false, true).is_dirty());

        // Multiple flags
        assert!(WorkingTreeStatus::new(true, true, true, true, true).is_dirty());
    }

    #[test]
    fn test_working_tree_status_to_symbols() {
        // Empty
        assert_eq!(WorkingTreeStatus::default().to_symbols(), "");

        // Individual symbols
        assert_eq!(
            WorkingTreeStatus::new(true, false, false, false, false).to_symbols(),
            "+"
        );
        assert_eq!(
            WorkingTreeStatus::new(false, true, false, false, false).to_symbols(),
            "!"
        );
        assert_eq!(
            WorkingTreeStatus::new(false, false, true, false, false).to_symbols(),
            "?"
        );
        assert_eq!(
            WorkingTreeStatus::new(false, false, false, true, false).to_symbols(),
            "»"
        );
        assert_eq!(
            WorkingTreeStatus::new(false, false, false, false, true).to_symbols(),
            "✘"
        );

        // Combined symbols (order: staged, modified, untracked, renamed, deleted)
        assert_eq!(
            WorkingTreeStatus::new(true, true, false, false, false).to_symbols(),
            "+!"
        );
        assert_eq!(
            WorkingTreeStatus::new(true, true, true, false, false).to_symbols(),
            "+!?"
        );
        assert_eq!(
            WorkingTreeStatus::new(true, true, true, true, true).to_symbols(),
            "+!?»✘"
        );
    }

    #[test]
    fn test_main_state_as_json_str() {
        assert_eq!(MainState::None.as_json_str(), None);
        assert_eq!(MainState::IsMain.as_json_str(), Some("is_main"));
        assert_eq!(
            MainState::WouldConflict.as_json_str(),
            Some("would_conflict")
        );
        assert_eq!(MainState::Empty.as_json_str(), Some("empty"));
        assert_eq!(MainState::SameCommit.as_json_str(), Some("same_commit"));
        assert_eq!(
            MainState::Integrated(IntegrationReason::TreesMatch).as_json_str(),
            Some("integrated")
        );
        assert_eq!(MainState::Diverged.as_json_str(), Some("diverged"));
        assert_eq!(MainState::Ahead.as_json_str(), Some("ahead"));
        assert_eq!(MainState::Behind.as_json_str(), Some("behind"));
    }

    #[test]
    fn test_integration_reason_into_static_str() {
        let s: &'static str = IntegrationReason::SameCommit.into();
        assert_eq!(s, "same-commit");
        let s: &'static str = IntegrationReason::Ancestor.into();
        assert_eq!(s, "ancestor");
        let s: &'static str = IntegrationReason::TreesMatch.into();
        assert_eq!(s, "trees-match");
        let s: &'static str = IntegrationReason::NoAddedChanges.into();
        assert_eq!(s, "no-added-changes");
        let s: &'static str = IntegrationReason::MergeAddsNothing.into();
        assert_eq!(s, "merge-adds-nothing");
    }

    #[test]
    fn test_main_state_integration_reason() {
        // Non-integrated states return None
        assert_eq!(MainState::None.integration_reason(), None);
        assert_eq!(MainState::IsMain.integration_reason(), None);
        assert_eq!(MainState::WouldConflict.integration_reason(), None);
        assert_eq!(MainState::Empty.integration_reason(), None);
        assert_eq!(MainState::SameCommit.integration_reason(), None);
        assert_eq!(MainState::Diverged.integration_reason(), None);
        assert_eq!(MainState::Ahead.integration_reason(), None);
        assert_eq!(MainState::Behind.integration_reason(), None);

        // Integrated states return the reason
        assert_eq!(
            MainState::Integrated(IntegrationReason::Ancestor).integration_reason(),
            Some(IntegrationReason::Ancestor)
        );
        assert_eq!(
            MainState::Integrated(IntegrationReason::TreesMatch).integration_reason(),
            Some(IntegrationReason::TreesMatch)
        );
        assert_eq!(
            MainState::Integrated(IntegrationReason::NoAddedChanges).integration_reason(),
            Some(IntegrationReason::NoAddedChanges)
        );
        assert_eq!(
            MainState::Integrated(IntegrationReason::MergeAddsNothing).integration_reason(),
            Some(IntegrationReason::MergeAddsNothing)
        );
    }

    #[test]
    fn test_main_state_from_integration_and_counts() {
        // IsMain takes priority
        assert!(matches!(
            MainState::from_integration_and_counts(true, false, None, false, 5, 3),
            MainState::IsMain
        ));

        // WouldConflict next
        assert!(matches!(
            MainState::from_integration_and_counts(false, true, None, false, 5, 3),
            MainState::WouldConflict
        ));

        // Empty (passed as integration state - same commit with clean working tree)
        assert!(matches!(
            MainState::from_integration_and_counts(
                false,
                false,
                Some(MainState::Empty),
                false,
                0,
                0
            ),
            MainState::Empty
        ));

        // Integrated (passed as integration state)
        assert!(matches!(
            MainState::from_integration_and_counts(
                false,
                false,
                Some(MainState::Integrated(IntegrationReason::Ancestor)),
                false,
                0,
                5
            ),
            MainState::Integrated(IntegrationReason::Ancestor)
        ));

        // SameCommit (via is_same_commit_dirty flag, NOT integration)
        assert!(matches!(
            MainState::from_integration_and_counts(false, false, None, true, 0, 0),
            MainState::SameCommit
        ));

        // Diverged (both ahead and behind)
        assert!(matches!(
            MainState::from_integration_and_counts(false, false, None, false, 3, 2),
            MainState::Diverged
        ));

        // Ahead only
        assert!(matches!(
            MainState::from_integration_and_counts(false, false, None, false, 3, 0),
            MainState::Ahead
        ));

        // Behind only
        assert!(matches!(
            MainState::from_integration_and_counts(false, false, None, false, 0, 2),
            MainState::Behind
        ));

        // None (in sync)
        assert!(matches!(
            MainState::from_integration_and_counts(false, false, None, false, 0, 0),
            MainState::None
        ));
    }

    #[test]
    fn test_operation_state_as_json_str() {
        assert_eq!(OperationState::None.as_json_str(), None);
        assert_eq!(OperationState::Conflicts.as_json_str(), Some("conflicts"));
        assert_eq!(OperationState::Rebase.as_json_str(), Some("rebase"));
        assert_eq!(OperationState::Merge.as_json_str(), Some("merge"));
    }

    #[test]
    fn test_check_integration_state_priority5_requires_clean() {
        // Priority 5 checks if working tree matches main.
        // It must also require is_clean to avoid marking worktrees with
        // uncommitted changes as integrated (which would incorrectly suggest
        // they're safe to remove).

        // Create a minimal ListItem for testing - only set fields that affect integration checks
        let mut item = ListItem::new_branch("abc123".to_string(), "feature".to_string());
        item.is_ancestor = Some(false); // not an ancestor (to skip priority 1-2)
        item.committed_trees_match = Some(false); // trees don't match (to skip priority 4)
        item.has_file_changes = None; // unknown (to skip priority 3)
        item.would_merge_add = None; // unknown (to skip priority 6)

        // Dirty working tree: should NOT return Integrated even though working tree matches main
        assert_eq!(
            item.check_integration_state(
                false,        // not main
                Some("main"), // has default branch
                false,        // is_clean = false (dirty working tree)
                true,         // working_tree_matches_main = true
            ),
            None,
            "Priority 5 should reject dirty working tree"
        );

        // Clean working tree: SHOULD return Integrated(TreesMatch)
        assert_eq!(
            item.check_integration_state(
                false,
                Some("main"),
                true, // is_clean = true
                true, // working_tree_matches_main = true
            ),
            Some(MainState::Integrated(IntegrationReason::TreesMatch)),
            "Priority 5 should accept clean working tree"
        );
    }

    #[test]
    fn test_check_integration_state_untracked_blocks_integration() {
        // When is_clean is computed at the call site, untracked files make is_clean=false.
        // This test verifies that is_clean=false blocks integration, which is what happens
        // when there are untracked files.

        let mut item = ListItem::new_branch("abc123".to_string(), "feature".to_string());
        item.is_ancestor = Some(false);
        item.committed_trees_match = Some(false);
        item.has_file_changes = None;
        item.would_merge_add = None;

        // is_clean=false (as computed when untracked files exist): should NOT return Integrated
        assert_eq!(
            item.check_integration_state(
                false,
                Some("main"),
                false, // is_clean = false (represents untracked files blocking integration)
                true,  // working_tree_matches_main = true
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
                true, // working_tree_matches_main = true
            ),
            Some(MainState::Integrated(IntegrationReason::TreesMatch)),
            "Clean working tree should show as integrated"
        );
    }

    // ============================================================================
    // Divergence Tests
    // ============================================================================

    #[test]
    fn test_divergence_from_counts_with_remote() {
        assert_eq!(
            Divergence::from_counts_with_remote(0, 0),
            Divergence::InSync
        );
        assert_eq!(Divergence::from_counts_with_remote(5, 0), Divergence::Ahead);
        assert_eq!(
            Divergence::from_counts_with_remote(0, 3),
            Divergence::Behind
        );
        assert_eq!(
            Divergence::from_counts_with_remote(5, 3),
            Divergence::Diverged
        );
    }

    #[test]
    fn test_divergence_symbol() {
        assert_eq!(Divergence::None.symbol(), "");
        assert_eq!(Divergence::InSync.symbol(), "|");
        assert_eq!(Divergence::Ahead.symbol(), "⇡");
        assert_eq!(Divergence::Behind.symbol(), "⇣");
        assert_eq!(Divergence::Diverged.symbol(), "⇅");
    }

    #[test]
    fn test_divergence_styled() {
        // None returns None
        assert!(Divergence::None.styled().is_none());

        // Other variants return styled strings
        let styled = Divergence::InSync.styled().unwrap();
        assert!(styled.contains("|"));

        let styled = Divergence::Ahead.styled().unwrap();
        assert!(styled.contains("⇡"));

        let styled = Divergence::Behind.styled().unwrap();
        assert!(styled.contains("⇣"));

        let styled = Divergence::Diverged.styled().unwrap();
        assert!(styled.contains("⇅"));
    }

    // ============================================================================
    // WorktreeState Tests
    // ============================================================================

    #[test]
    fn test_worktree_state_display() {
        assert_eq!(format!("{}", WorktreeState::None), "");
        assert_eq!(format!("{}", WorktreeState::PathMismatch), "⚑");
        assert_eq!(format!("{}", WorktreeState::Prunable), "⊟");
        assert_eq!(format!("{}", WorktreeState::Locked), "⊞");
        assert_eq!(format!("{}", WorktreeState::Branch), "/");
    }

    #[test]
    fn test_worktree_state_serialize() {
        // Serialize to JSON and check the string representation
        let json = serde_json::to_string(&WorktreeState::None).unwrap();
        assert_eq!(json, "\"\"");

        let json = serde_json::to_string(&WorktreeState::PathMismatch).unwrap();
        assert_eq!(json, "\"⚑\"");

        let json = serde_json::to_string(&WorktreeState::Branch).unwrap();
        assert_eq!(json, "\"/\"");
    }

    // ============================================================================
    // MainState Tests
    // ============================================================================

    #[test]
    fn test_main_state_display() {
        assert_eq!(format!("{}", MainState::None), "");
        assert_eq!(format!("{}", MainState::IsMain), "^");
        assert_eq!(format!("{}", MainState::WouldConflict), "✗");
        assert_eq!(format!("{}", MainState::Empty), "_");
        assert_eq!(format!("{}", MainState::SameCommit), "–"); // en-dash
        assert_eq!(
            format!("{}", MainState::Integrated(IntegrationReason::Ancestor)),
            "⊂"
        );
        assert_eq!(format!("{}", MainState::Diverged), "↕");
        assert_eq!(format!("{}", MainState::Ahead), "↑");
        assert_eq!(format!("{}", MainState::Behind), "↓");
    }

    #[test]
    fn test_main_state_styled() {
        // None returns None
        assert!(MainState::None.styled().is_none());

        // WouldConflict is yellow
        let styled = MainState::WouldConflict.styled().unwrap();
        assert!(styled.contains("✗"));

        // Other states are dimmed
        let styled = MainState::IsMain.styled().unwrap();
        assert!(styled.contains("^"));

        let styled = MainState::Ahead.styled().unwrap();
        assert!(styled.contains("↑"));
    }

    #[test]
    fn test_main_state_serialize() {
        let json = serde_json::to_string(&MainState::None).unwrap();
        assert_eq!(json, "\"\"");

        let json = serde_json::to_string(&MainState::IsMain).unwrap();
        assert_eq!(json, "\"^\"");

        let json = serde_json::to_string(&MainState::Diverged).unwrap();
        assert_eq!(json, "\"↕\"");
    }

    // ============================================================================
    // OperationState Tests
    // ============================================================================

    #[test]
    fn test_operation_state_display() {
        assert_eq!(format!("{}", OperationState::None), "");
        assert_eq!(format!("{}", OperationState::Conflicts), "✘");
        assert_eq!(format!("{}", OperationState::Rebase), "⤴");
        assert_eq!(format!("{}", OperationState::Merge), "⤵");
    }

    #[test]
    fn test_operation_state_styled() {
        // None returns None
        assert!(OperationState::None.styled().is_none());

        // Conflicts is red
        let styled = OperationState::Conflicts.styled().unwrap();
        assert!(styled.contains("✘"));

        // Rebase and Merge are yellow
        let styled = OperationState::Rebase.styled().unwrap();
        assert!(styled.contains("⤴"));

        let styled = OperationState::Merge.styled().unwrap();
        assert!(styled.contains("⤵"));
    }

    #[test]
    fn test_operation_state_serialize() {
        let json = serde_json::to_string(&OperationState::None).unwrap();
        assert_eq!(json, "\"\"");

        let json = serde_json::to_string(&OperationState::Conflicts).unwrap();
        assert_eq!(json, "\"✘\"");
    }

    // ============================================================================
    // StatusSymbols Tests
    // ============================================================================

    #[test]
    fn test_status_symbols_is_empty() {
        let symbols = StatusSymbols::default();
        assert!(symbols.is_empty());

        let symbols = StatusSymbols {
            main_state: MainState::Ahead,
            ..Default::default()
        };
        assert!(!symbols.is_empty());

        let symbols = StatusSymbols {
            operation_state: OperationState::Rebase,
            ..Default::default()
        };
        assert!(!symbols.is_empty());

        let symbols = StatusSymbols {
            worktree_state: WorktreeState::Locked,
            ..Default::default()
        };
        assert!(!symbols.is_empty());

        let symbols = StatusSymbols {
            upstream_divergence: Divergence::Ahead,
            ..Default::default()
        };
        assert!(!symbols.is_empty());

        let symbols = StatusSymbols {
            working_tree: WorkingTreeStatus::new(true, false, false, false, false),
            ..Default::default()
        };
        assert!(!symbols.is_empty());

        let symbols = StatusSymbols {
            user_marker: Some("🔥".to_string()),
            ..Default::default()
        };
        assert!(!symbols.is_empty());
    }

    #[test]
    fn test_status_symbols_format_compact() {
        // Empty symbols
        let symbols = StatusSymbols::default();
        assert_eq!(symbols.format_compact(), "");

        // Single symbol
        let symbols = StatusSymbols {
            main_state: MainState::Ahead,
            ..Default::default()
        };
        let compact = symbols.format_compact();
        assert!(compact.contains("↑"));

        // Multiple symbols
        let symbols = StatusSymbols {
            working_tree: WorkingTreeStatus::new(true, true, false, false, false),
            main_state: MainState::Ahead,
            ..Default::default()
        };
        let compact = symbols.format_compact();
        assert!(compact.contains("+"));
        assert!(compact.contains("!"));
        assert!(compact.contains("↑"));
    }

    #[test]
    fn test_status_symbols_render_with_mask() {
        let symbols = StatusSymbols {
            main_state: MainState::Ahead,
            ..Default::default()
        };
        let rendered = symbols.render_with_mask(&PositionMask::FULL);
        // Should have fixed-width output with spacing
        assert!(!rendered.is_empty());
        assert!(rendered.contains("↑"));
    }

    // ============================================================================
    // UpstreamStatus Tests
    // ============================================================================

    #[test]
    fn test_upstream_status_active_with_remote() {
        let status = UpstreamStatus::from_parts(Some("origin".to_string()), 3, 2);
        let active = status.active().unwrap();
        assert_eq!(active.remote, "origin");
        assert_eq!(active.ahead, 3);
        assert_eq!(active.behind, 2);
    }

    #[test]
    fn test_upstream_status_active_no_remote() {
        let status = UpstreamStatus::from_parts(None, 0, 0);
        assert!(status.active().is_none());
    }

    // ============================================================================
    // ListItem Tests
    // ============================================================================

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
    fn test_list_item_commit_details() {
        let item = ListItem::new_branch("abc123".to_string(), "feature".to_string());
        let details = item.commit_details();
        assert_eq!(details.timestamp, 0);
        assert_eq!(details.commit_message, "");
    }

    #[test]
    fn test_list_item_counts() {
        let item = ListItem::new_branch("abc123".to_string(), "feature".to_string());
        let counts = item.counts();
        assert_eq!(counts.ahead, 0);
        assert_eq!(counts.behind, 0);

        let mut item = ListItem::new_branch("abc123".to_string(), "feature".to_string());
        item.counts = Some(AheadBehind {
            ahead: 5,
            behind: 3,
        });
        let counts = item.counts();
        assert_eq!(counts.ahead, 5);
        assert_eq!(counts.behind, 3);
    }

    #[test]
    fn test_list_item_branch_diff() {
        let item = ListItem::new_branch("abc123".to_string(), "feature".to_string());
        let diff = item.branch_diff();
        assert!(diff.diff.is_empty());
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

    // ============================================================================
    // PositionMask Tests
    // ============================================================================

    #[test]
    fn test_position_mask_width() {
        let mask = PositionMask::FULL;
        // Check expected widths for each position
        assert_eq!(mask.width(PositionMask::STAGED), 1);
        assert_eq!(mask.width(PositionMask::MODIFIED), 1);
        assert_eq!(mask.width(PositionMask::UNTRACKED), 1);
        assert_eq!(mask.width(PositionMask::WORKTREE_STATE), 1);
        assert_eq!(mask.width(PositionMask::MAIN_STATE), 1);
        assert_eq!(mask.width(PositionMask::UPSTREAM_DIVERGENCE), 1);
        assert_eq!(mask.width(PositionMask::USER_MARKER), 2);
    }

    #[test]
    fn test_position_mask_default() {
        let mask = PositionMask::default();
        // Default has all widths at 0
        for i in 0..7 {
            assert_eq!(mask.width(i), 0);
        }
    }

    // ============================================================================
    // GitOperationState Tests
    // ============================================================================

    #[test]
    fn test_git_operation_state_default() {
        let state = GitOperationState::default();
        assert_eq!(state, GitOperationState::None);
    }

    #[test]
    fn test_git_operation_is_none() {
        assert!(git_operation_is_none(&GitOperationState::None));
        assert!(!git_operation_is_none(&GitOperationState::Rebase));
        assert!(!git_operation_is_none(&GitOperationState::Merge));
    }

    // ============================================================================
    // AheadBehind and BranchDiffTotals Tests
    // ============================================================================

    #[test]
    fn test_ahead_behind_default() {
        let ab = AheadBehind::default();
        assert_eq!(ab.ahead, 0);
        assert_eq!(ab.behind, 0);
    }

    #[test]
    fn test_branch_diff_totals_default() {
        let diff = BranchDiffTotals::default();
        assert!(diff.diff.is_empty());
    }

    // ============================================================================
    // CommitDetails Tests
    // ============================================================================

    #[test]
    fn test_commit_details_default() {
        let details = CommitDetails::default();
        assert_eq!(details.timestamp, 0);
        assert_eq!(details.commit_message, "");
    }
}
