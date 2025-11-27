use std::path::PathBuf;
use worktrunk::git::LineDiff;

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
    /// Format: `branch  status  ±working  commits  upstream  ci` (2-space separators)
    ///
    /// Use via JSON: `wt list --format=json | jq '.[] | select(.is_current) | .status_line'`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_line: Option<String>,
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
            u.active().and_then(|(_, upstream_ahead, upstream_behind)| {
                ColumnKind::Upstream.format_diff_plain(upstream_ahead, upstream_behind)
            })
        });

        // CI column shows only the indicator (●/○/◐), not text
        // Let render.rs handle it via render_indicator()
        let ci_status_display = None;

        Self {
            commits_display,
            branch_diff_display,
            upstream_display,
            ci_status_display,
            status_display: None,
            status_line: None,
        }
    }
}

/// Type-specific data for worktrees
#[derive(Clone, serde::Serialize, Default)]
pub struct WorktreeData {
    pub path: PathBuf,
    pub bare: bool,
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
            bare: wt.bare,
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

impl AheadBehind {
    /// Compute divergence states from ahead/behind counts and upstream status.
    pub fn compute_divergences(
        &self,
        upstream: &UpstreamStatus,
    ) -> (MainDivergence, UpstreamDivergence) {
        let main_divergence = MainDivergence::from_counts(self.ahead, self.behind);
        let (upstream_ahead, upstream_behind) =
            upstream.active().map(|(_, a, b)| (a, b)).unwrap_or((0, 0));
        let upstream_divergence = UpstreamDivergence::from_counts(upstream_ahead, upstream_behind);

        (main_divergence, upstream_divergence)
    }
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

impl UpstreamStatus {
    pub fn active(&self) -> Option<(&str, usize, usize)> {
        self.remote
            .as_deref()
            .map(|remote| (remote, self.ahead, self.behind))
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

    // TODO: Same concern as counts/branch_diff above - should upstream fields always be present?
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<UpstreamStatus>,

    /// CI/PR status: None = not loaded, Some(None) = no CI, Some(Some(status)) = has CI
    pub pr_status: Option<Option<PrStatus>>,
    /// Git status symbols - None until all dependencies are ready
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
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
            upstream: None,
            pr_status: None,
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

    pub fn worktree_path(&self) -> Option<&PathBuf> {
        self.worktree_data().map(|data| &data.path)
    }

    pub fn pr_status(&self) -> Option<Option<&PrStatus>> {
        self.pr_status.as_ref().map(|opt| opt.as_ref())
    }

    /// Determine if the item contains no unique work and can likely be removed.
    pub(crate) fn is_potentially_removable(&self) -> bool {
        if self.is_main() {
            return false;
        }

        let counts = self.counts();

        if let Some(data) = self.worktree_data() {
            let no_commits_and_clean = counts.ahead == 0
                && data
                    .working_tree_diff
                    .as_ref()
                    .map(|d| d.is_empty())
                    .unwrap_or(true);
            let matches_main = data
                .working_tree_diff_with_main
                .and_then(|opt_diff| opt_diff)
                .map(|diff| diff.is_empty())
                .unwrap_or(false);
            no_commits_and_clean || matches_main
        } else {
            counts.ahead == 0
        }
    }

    /// Format this item as a single-line statusline string.
    ///
    /// Format: `branch  status  ±working  commits  upstream  ci`
    /// Uses 2-space separators between non-empty parts.
    pub fn format_statusline(&self) -> String {
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
        if let Some(data) = self.worktree_data()
            && let Some(ref diff) = data.working_tree_diff
            && !diff.is_empty()
            && let Some(formatted) =
                ColumnKind::WorkingDiff.format_diff_plain(diff.added, diff.deleted)
        {
            parts.push(format!("±{formatted}"));
        }

        // 4. Commits ahead/behind main
        if let Some(formatted) =
            ColumnKind::AheadBehind.format_diff_plain(self.counts().ahead, self.counts().behind)
        {
            parts.push(formatted);
        }

        // 5. Upstream status
        if let Some(ref upstream) = self.upstream
            && let Some((_, ahead, behind)) = upstream.active()
            && let Some(formatted) = ColumnKind::Upstream.format_diff_plain(ahead, behind)
        {
            parts.push(formatted);
        }

        // 6. CI status
        if let Some(Some(ref pr_status)) = self.pr_status {
            parts.push(pr_status.format_indicator());
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
        self.display.status_line = Some(self.format_statusline());

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
    // TODO(status-indicator): show a status glyph when a worktree's checked-out branch
    // differs from the branch name we associate with it (e.g., worktree exists but on another branch).
    pub(crate) fn compute_status_symbols(
        &mut self,
        default_branch: Option<&str>,
        has_merge_tree_conflicts: bool,
        user_status: Option<String>,
        working_tree_symbols: Option<&str>,
        has_conflicts: bool,
    ) {
        // Common fields for both worktrees and branches
        let default_counts = AheadBehind::default();
        let default_upstream = UpstreamStatus::default();
        let counts = self.counts.as_ref().unwrap_or(&default_counts);
        let upstream = self.upstream.as_ref().unwrap_or(&default_upstream);
        let (main_divergence, upstream_divergence) = counts.compute_divergences(upstream);

        match &self.kind {
            ItemKind::Worktree(data) => {
                // Full status computation for worktrees
                // Use default_branch directly (None for main worktree)

                // Worktree state - priority: path_mismatch > prunable > locked
                let worktree_state = if data.path_mismatch {
                    WorktreeState::PathMismatch
                } else if data.prunable.is_some() {
                    WorktreeState::Prunable
                } else if data.locked.is_some() {
                    WorktreeState::Locked
                } else {
                    WorktreeState::None
                };

                // Determine base branch state (only for non-main worktrees with base branch)
                let base_state = determine_worktree_base_state(
                    data.is_main,
                    default_branch,
                    counts.ahead,
                    data.working_tree_diff.as_ref(),
                    &data.working_tree_diff_with_main,
                );

                // Apply priority: Conflicts > Rebase > Merge > MergeTreeConflicts > base_state
                let branch_op_state = if has_conflicts {
                    BranchOpState::Conflicts
                } else if data.git_operation == GitOperationState::Rebase {
                    BranchOpState::Rebase
                } else if data.git_operation == GitOperationState::Merge {
                    BranchOpState::Merge
                } else if has_merge_tree_conflicts {
                    BranchOpState::MergeTreeConflicts
                } else {
                    base_state
                };

                // Override main_divergence for the main worktree
                let main_divergence = if data.is_main {
                    MainDivergence::IsMain
                } else {
                    main_divergence
                };

                self.status_symbols = Some(StatusSymbols {
                    branch_op_state,
                    worktree_state,
                    main_divergence,
                    upstream_divergence,
                    working_tree: working_tree_symbols.unwrap_or("").to_string(),
                    user_status,
                });
            }
            ItemKind::Branch => {
                // Simplified status computation for branches
                // Only compute symbols that apply to branches (no working tree, git operation, or worktree attrs)

                // Branch op state - branches can only show MergeTreeConflicts or NoCommits
                // (MatchesMain only applies to worktrees since branches don't have working trees)
                let branch_op_state = if has_merge_tree_conflicts {
                    BranchOpState::MergeTreeConflicts
                } else if let Some(ref c) = self.counts {
                    if c.ahead == 0 {
                        BranchOpState::NoCommits
                    } else {
                        BranchOpState::None
                    }
                } else {
                    BranchOpState::None
                };

                self.status_symbols = Some(StatusSymbols {
                    branch_op_state,
                    worktree_state: WorktreeState::Branch,
                    main_divergence,
                    upstream_divergence,
                    working_tree: String::new(),
                    user_status,
                });
            }
        }
    }
}

/// Determine branch state for a worktree.
///
/// Returns:
/// - `BranchOpState::None` if main worktree or no base branch
/// - `BranchOpState::NoCommits` if no commits ahead and working tree is clean
/// - `BranchOpState::MatchesMain` if ahead == 0 but dirty tree happens to match main (rare)
/// - `BranchOpState::None` otherwise
fn determine_worktree_base_state(
    is_main: bool,
    default_branch: Option<&str>,
    ahead: usize,
    working_tree_diff: Option<&LineDiff>,
    working_tree_diff_with_main: &Option<Option<LineDiff>>,
) -> BranchOpState {
    if is_main || default_branch.is_none() {
        return BranchOpState::None;
    }

    let is_clean = working_tree_diff.map(|d| d.is_empty()).unwrap_or(true);

    // Check if no commits and clean working tree (most common removable state)
    if ahead == 0 && is_clean {
        return BranchOpState::NoCommits;
    }

    // Check if working tree matches main exactly (requires diff with main to be computed)
    // This is rare - only when ahead == 0, tree is dirty, but dirty changes happen to match main
    if let Some(Some(mdiff)) = working_tree_diff_with_main.as_ref()
        && mdiff.is_empty()
        && ahead == 0
    {
        BranchOpState::MatchesMain
    } else {
        BranchOpState::None
    }
}

/// Main branch divergence state
///
/// Represents relationship to the main/primary branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, strum::IntoStaticStr)]
pub enum MainDivergence {
    #[strum(serialize = "")]
    /// Up to date with main branch
    #[default]
    None,
    /// This is the main/default branch itself
    IsMain,
    /// Ahead of main (has commits main doesn't have)
    Ahead,
    /// Behind main (missing commits from main)
    Behind,
    /// Diverged (both ahead and behind main)
    Diverged,
}

impl std::fmt::Display for MainDivergence {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::None => Ok(()),
            Self::IsMain => write!(f, "^"),
            Self::Ahead => write!(f, "↑"),
            Self::Behind => write!(f, "↓"),
            Self::Diverged => write!(f, "↕"),
        }
    }
}

impl serde::Serialize for MainDivergence {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Serialize as empty string for None, or the character for other variants
        serializer.serialize_str(&self.to_string())
    }
}

impl MainDivergence {
    /// Compute divergence state from ahead/behind counts.
    ///
    /// Note: This cannot produce `IsMain` - that variant is set explicitly
    /// when the worktree is on the main branch.
    pub fn from_counts(ahead: usize, behind: usize) -> Self {
        match (ahead, behind) {
            (0, 0) => Self::None,
            (_, 0) => Self::Ahead,
            (0, _) => Self::Behind,
            _ => Self::Diverged,
        }
    }

    /// Returns styled symbol (dimmed), or None for None variant.
    pub fn styled(&self) -> Option<String> {
        use worktrunk::styling::HINT;
        if *self == Self::None {
            None
        } else {
            Some(format!("{HINT}{self}{HINT:#}"))
        }
    }
}

/// Upstream/remote divergence state
///
/// Represents relationship to the remote tracking branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, strum::IntoStaticStr)]
pub enum UpstreamDivergence {
    #[strum(serialize = "")]
    /// Up to date with remote
    #[default]
    None,
    /// Ahead of remote (has commits remote doesn't have)
    Ahead,
    /// Behind remote (missing commits from remote)
    Behind,
    /// Diverged (both ahead and behind remote)
    Diverged,
}

impl std::fmt::Display for UpstreamDivergence {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::None => Ok(()),
            Self::Ahead => write!(f, "⇡"),
            Self::Behind => write!(f, "⇣"),
            Self::Diverged => write!(f, "⇅"),
        }
    }
}

impl serde::Serialize for UpstreamDivergence {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Serialize as empty string for None, or the character for other variants
        serializer.serialize_str(&self.to_string())
    }
}

impl UpstreamDivergence {
    /// Compute divergence state from ahead/behind counts.
    pub fn from_counts(ahead: usize, behind: usize) -> Self {
        match (ahead, behind) {
            (0, 0) => Self::None,
            (_, 0) => Self::Ahead,
            (0, _) => Self::Behind,
            _ => Self::Diverged,
        }
    }

    /// Returns styled symbol (dimmed), or None for None variant.
    pub fn styled(&self) -> Option<String> {
        use worktrunk::styling::HINT;
        if *self == Self::None {
            None
        } else {
            Some(format!("{HINT}{self}{HINT:#}"))
        }
    }
}

/// Worktree state indicator
///
/// Shows the "location" state of a worktree or branch:
/// - For worktrees: whether the path matches the template, or has issues
/// - For branches (without worktree): shows ⎇ to distinguish from worktrees
///
/// Priority order for worktrees: PathMismatch > Prunable > Locked
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, strum::IntoStaticStr)]
pub enum WorktreeState {
    #[strum(serialize = "")]
    /// Normal worktree (path matches template, not locked or prunable)
    #[default]
    None,
    /// Path doesn't match what the template would generate (white flag = "not at home")
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
            Self::PathMismatch => write!(f, "⚐"),
            Self::Prunable => write!(f, "⌫"),
            Self::Locked => write!(f, "⊠"),
            Self::Branch => write!(f, "⎇"),
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

/// Combined branch and operation state
///
/// Represents the primary state of a branch/worktree in a single position.
/// Priority order determines which symbol is shown when multiple conditions apply:
/// 1. Conflicts (✖) - blocking, must resolve
/// 2. Rebase (↻) - active operation
/// 3. Merge (⋈) - active operation
/// 4. MergeTreeConflicts (⚠) - potential problem
/// 5. MatchesMain (≡) - removable
/// 6. NoCommits (_) - removable
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, strum::IntoStaticStr)]
pub enum BranchOpState {
    #[strum(serialize = "")]
    /// Normal working branch
    #[default]
    None,
    /// Actual merge conflicts with main (unmerged paths in working tree)
    Conflicts,
    /// Rebase in progress
    Rebase,
    /// Merge in progress
    Merge,
    /// Merge-tree conflicts with main (simulated via git merge-tree)
    MergeTreeConflicts,
    /// Working tree identical to main branch
    MatchesMain,
    /// No commits ahead and clean working tree (not matching main)
    NoCommits,
}

impl std::fmt::Display for BranchOpState {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::None => Ok(()),
            Self::Conflicts => write!(f, "✖"),
            Self::Rebase => write!(f, "↻"),
            Self::Merge => write!(f, "⋈"),
            Self::MergeTreeConflicts => write!(f, "⚠"),
            Self::MatchesMain => write!(f, "≡"),
            Self::NoCommits => write!(f, "_"),
        }
    }
}

impl BranchOpState {
    /// Returns styled symbol with appropriate color, or None for None variant.
    ///
    /// Color semantics:
    /// - ERROR (red): Conflicts - blocking problems
    /// - WARNING (yellow): Rebase, Merge, MergeTreeConflicts - active/stuck states
    /// - HINT (dimmed): MatchesMain, NoCommits - low urgency removability indicators
    pub fn styled(&self) -> Option<String> {
        use worktrunk::styling::{ERROR, HINT, WARNING};
        match self {
            Self::None => None,
            Self::Conflicts => Some(format!("{ERROR}{self}{ERROR:#}")),
            Self::Rebase | Self::Merge | Self::MergeTreeConflicts => {
                Some(format!("{WARNING}{self}{WARNING:#}"))
            }
            Self::MatchesMain | Self::NoCommits => Some(format!("{HINT}{self}{HINT:#}")),
        }
    }
}

impl serde::Serialize for BranchOpState {
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
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
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
/// Stores maximum character width for each of 8 positions (including user status).
/// A width of 0 means the position is unused.
#[derive(Debug, Clone, Copy, Default)]
pub struct PositionMask {
    /// Maximum width for each position: [0, 1, 2, 3, 4, 5, 6, 7]
    /// 0 = position unused, >0 = max characters needed
    widths: [usize; 8],
}

impl PositionMask {
    // Render order indices (0-7) - symbols appear in this order left-to-right
    // Working tree split into 3 fixed positions for vertical alignment
    const STAGED: usize = 0; // + (staged changes)
    const MODIFIED: usize = 1; // ! (modified files)
    const UNTRACKED: usize = 2; // ? (untracked files)
    const BRANCH_OP_STATE: usize = 3; // Combined: branch state + git operation
    const MAIN_DIVERGENCE: usize = 4;
    const UPSTREAM_DIVERGENCE: usize = 5;
    const WORKTREE_STATE: usize = 6;
    const USER_STATUS: usize = 7;

    /// Full mask with all positions enabled (for JSON output and progressive rendering)
    /// Allocates realistic widths based on common symbol sizes to ensure proper grid alignment
    pub const FULL: Self = Self {
        widths: [
            1, // STAGED: + (1 char)
            1, // MODIFIED: ! (1 char)
            1, // UNTRACKED: ? (1 char)
            1, // BRANCH_OP_STATE: ✖↻⋈⚠≡_ (1 char, priority: conflicts > rebase > merge > merge-tree > no-commits > matches)
            1, // MAIN_DIVERGENCE: ^, ↑, ↓, ↕ (1 char)
            1, // UPSTREAM_DIVERGENCE: ⇡, ⇣, ⇅ (1 char)
            1, // WORKTREE_STATE: ⎇ for branches, ⚐⌫⊠ for worktrees (priority: path_mismatch > prunable > locked)
            2, // USER_STATUS: single emoji or two chars (allocate 2)
        ],
    };

    /// Get the allocated width for a position
    pub(crate) fn width(&self, pos: usize) -> usize {
        self.widths[pos]
    }
}

/// Structured status symbols for aligned rendering
///
/// Symbols are categorized to enable vertical alignment in table output:
/// - Working tree: +, !, ? (staged, modified, untracked - priority order)
/// - Branch/op state: ✖, ↻, ⋈, ⚠, ≡, _ (combined position with priority)
/// - Main divergence: ^, ↑, ↓, ↕
/// - Upstream divergence: ⇡, ⇣, ⇅
/// - Worktree state: ⎇ for branches, ⚐⌫⊠ for worktrees (priority-only)
/// - User status: custom labels, emoji
///
/// ## Mutual Exclusivity
///
/// **Combined with priority (branch state + git operation):**
/// Priority: ✖ > ↻ > ⋈ > ⚠ > ≡ > _
/// - ✖: Actual conflicts (must resolve)
/// - ↻: Rebase in progress
/// - ⋈: Merge in progress
/// - ⚠: Merge-tree conflicts (potential problem)
/// - ≡: Matches main (removable)
/// - _: No commits (removable)
///
/// **Mutually exclusive (enforced by type system):**
/// - ^ vs ↑ vs ↓ vs ↕: Main divergence (MainDivergence enum)
/// - ⇡ vs ⇣ vs ⇅: Upstream divergence (UpstreamDivergence enum)
///
/// **Priority-only (can co-occur but only highest priority shown):**
/// - ⚐ vs ⌫ vs ⊠: Worktree attrs (priority: path_mismatch ⚐ > prunable ⌫ > locked ⊠)
/// - ⎇: Branch indicator (mutually exclusive with ⚐⌫⊠ as branches can't have worktree attrs)
///
/// **NOT mutually exclusive (can co-occur):**
/// - Working tree symbols (+!?): Can have multiple types of changes
#[derive(Debug, Clone, Default)]
pub struct StatusSymbols {
    /// Combined branch and operation state (mutually exclusive with priority)
    /// Priority: Conflicts (✖) > Rebase (↻) > Merge (⋈) > MergeTreeConflicts (⚠) > MatchesMain (≡) > NoCommits (_)
    pub(crate) branch_op_state: BranchOpState,

    /// Worktree state: ⎇ for branches, ⚐⌫⊠ for worktrees (priority: path_mismatch > prunable > locked)
    pub(crate) worktree_state: WorktreeState,

    /// Main branch divergence state (mutually exclusive)
    pub(crate) main_divergence: MainDivergence,

    /// Remote/upstream divergence state (mutually exclusive)
    pub(crate) upstream_divergence: UpstreamDivergence,

    /// Working tree changes: +, !, ? (NOT mutually exclusive, can have multiple)
    pub(crate) working_tree: String,

    /// User-defined status annotation (custom labels, e.g., 💬, 🤖)
    pub(crate) user_status: Option<String>,
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
        use worktrunk::styling::{CYAN, HINT, StyledLine, WARNING};

        let mut result = String::with_capacity(64);

        if self.is_empty() {
            return result;
        }

        // Build list of (position_index, content, has_data) tuples
        // Ordered by importance/actionability
        // Apply colors based on semantic meaning:
        // - Red (ERROR): Actual conflicts (blocking problems)
        // - Yellow (WARNING): Git operations, locked/prunable (stuck states needing attention)
        // - Cyan: Working tree changes (activity indicator)
        // - Dimmed (HINT): Branch state symbols that indicate removability + divergence arrows (low urgency)
        let (branch_op_state_str, has_branch_op_state) = self
            .branch_op_state
            .styled()
            .map_or((String::new(), false), |s| (s, true));
        let (main_divergence_str, has_main_divergence) = self
            .main_divergence
            .styled()
            .map_or((String::new(), false), |s| (s, true));
        let (upstream_divergence_str, has_upstream_divergence) = self
            .upstream_divergence
            .styled()
            .map_or((String::new(), false), |s| (s, true));
        // Working tree symbols split into 3 fixed columns for vertical alignment
        let style_working = |sym: char| -> (String, bool) {
            if self.working_tree.contains(sym) {
                (format!("{CYAN}{sym}{CYAN:#}"), true)
            } else {
                (String::new(), false)
            }
        };
        let (staged_str, has_staged) = style_working('+');
        let (modified_str, has_modified) = style_working('!');
        let (untracked_str, has_untracked) = style_working('?');
        let worktree_state_str = match self.worktree_state {
            WorktreeState::None => String::new(),
            // Branch indicator (⎇) is informational (dimmed)
            WorktreeState::Branch => format!("{HINT}{}{HINT:#}", self.worktree_state),
            // Worktree attrs (⚐⌫⊠) are warnings (yellow)
            _ => format!("{WARNING}{}{WARNING:#}", self.worktree_state),
        };
        let user_status_str = self.user_status.as_deref().unwrap_or("").to_string();

        // Position data: (position_mask, styled_content, has_data)
        // StyledLine handles width tracking automatically via .width()
        //
        // CRITICAL: Display order is working_tree first (staged, modified, untracked), then other symbols.
        // NEVER change this order - it ensures progressive and final rendering match exactly.
        // Tests will break if you change this, but that's expected - update the tests, not this order.
        let positions_data: [(usize, String, bool); 8] = [
            (PositionMask::STAGED, staged_str, has_staged),
            (PositionMask::MODIFIED, modified_str, has_modified),
            (PositionMask::UNTRACKED, untracked_str, has_untracked),
            (
                PositionMask::BRANCH_OP_STATE,
                branch_op_state_str,
                has_branch_op_state,
            ),
            (
                PositionMask::MAIN_DIVERGENCE,
                main_divergence_str,
                has_main_divergence,
            ),
            (
                PositionMask::UPSTREAM_DIVERGENCE,
                upstream_divergence_str,
                has_upstream_divergence,
            ),
            (
                PositionMask::WORKTREE_STATE,
                worktree_state_str,
                self.worktree_state != WorktreeState::None,
            ),
            (
                PositionMask::USER_STATUS,
                user_status_str,
                self.user_status.is_some(),
            ),
        ];

        // Grid-based rendering: each position gets a fixed width for vertical alignment.
        // CRITICAL: Always use PositionMask::FULL for consistent spacing between progressive and final rendering.
        // The mask provides the maximum width needed for each position across all rows.
        // Accept wider Status column with whitespace as tradeoff for perfect alignment.
        for (pos, styled_content, has_data) in positions_data {
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
        self.branch_op_state == BranchOpState::None
            && self.worktree_state == WorktreeState::None
            && self.main_divergence == MainDivergence::None
            && self.upstream_divergence == UpstreamDivergence::None
            && self.working_tree.is_empty()
            && self.user_status.is_none()
    }

    /// Render status symbols in compact form for statusline (no grid alignment).
    pub fn format_compact(&self) -> String {
        use worktrunk::styling::{CYAN, WARNING};

        let mut result = String::new();

        // Working tree symbols (compact, no padding) - cyan for activity
        if !self.working_tree.is_empty() {
            result.push_str(&format!("{CYAN}{}{CYAN:#}", self.working_tree));
        }

        // Branch/op state
        if let Some(styled) = self.branch_op_state.styled() {
            result.push_str(&styled);
        }

        // Worktree state (path mismatch/locked/prunable) - skip branch indicator (⎇)
        // Note: ⎇ only appears for branch-only items, never for worktrees (statusline context)
        if matches!(
            self.worktree_state,
            WorktreeState::PathMismatch | WorktreeState::Prunable | WorktreeState::Locked
        ) {
            result.push_str(&format!("{WARNING}{}{WARNING:#}", self.worktree_state));
        }

        // User status
        if let Some(ref user_status) = self.user_status {
            result.push_str(user_status);
        }

        result
    }
}

/// Working tree changes parsed into structured booleans
#[derive(Debug, Clone, serde::Serialize)]
struct WorkingTreeChanges {
    untracked: bool,
    modified: bool,
    staged: bool,
    renamed: bool,
    deleted: bool,
}

impl WorkingTreeChanges {
    fn from_symbols(symbols: &str) -> Self {
        Self {
            untracked: symbols.contains('?'),
            modified: symbols.contains('!'),
            staged: symbols.contains('+'),
            renamed: symbols.contains('»'),
            deleted: symbols.contains('✘'),
        }
    }
}

/// Status variant names (for queryability)
///
/// Field order matches display order in STATUS SYMBOLS: working_tree → branch_op_state → ...
#[derive(Debug, Clone, serde::Serialize)]
struct QueryableStatus {
    working_tree: WorkingTreeChanges,
    branch_op_state: &'static str,
    main_divergence: &'static str,
    upstream_divergence: &'static str,
    worktree_state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_status: Option<String>,
}

/// Status symbols (for display)
///
/// Field order matches display order in STATUS SYMBOLS: working_tree → branch_op_state → ...
#[derive(Debug, Clone, serde::Serialize)]
struct DisplaySymbols {
    working_tree: String,
    branch_op_state: String,
    main_divergence: String,
    upstream_divergence: String,
    worktree_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_status: Option<String>,
}

impl serde::Serialize for StatusSymbols {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("StatusSymbols", 2)?;

        // Status variant names (derived via strum::IntoStaticStr)
        let branch_op_state_variant: &'static str = self.branch_op_state.into();
        let main_divergence_variant: &'static str = self.main_divergence.into();
        let upstream_divergence_variant: &'static str = self.upstream_divergence.into();

        // Worktree state (derived via strum::IntoStaticStr)
        let worktree_state_variant: &'static str = self.worktree_state.into();

        let queryable_status = QueryableStatus {
            working_tree: WorkingTreeChanges::from_symbols(&self.working_tree),
            branch_op_state: branch_op_state_variant,
            main_divergence: main_divergence_variant,
            upstream_divergence: upstream_divergence_variant,
            worktree_state: worktree_state_variant,
            user_status: self.user_status.clone(),
        };

        let display_symbols = DisplaySymbols {
            working_tree: self.working_tree.clone(),
            branch_op_state: self.branch_op_state.to_string(),
            main_divergence: self.main_divergence.to_string(),
            upstream_divergence: self.upstream_divergence.to_string(),
            worktree_state: self.worktree_state.to_string(),
            user_status: self.user_status.clone(),
        };

        state.serialize_field("status", &queryable_status)?;
        state.serialize_field("status_symbols", &display_symbols)?;

        state.end()
    }
}

/// Helper for serde skip_serializing_if
fn git_operation_is_none(state: &GitOperationState) -> bool {
    *state == GitOperationState::None
}
