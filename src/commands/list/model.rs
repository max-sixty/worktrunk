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
        }
    }
}

/// Type-specific data for worktrees
#[derive(Clone, serde::Serialize, Default)]
pub struct WorktreeData {
    pub path: PathBuf,
    pub bare: bool,
    pub detached: bool,
    #[serde(skip)]
    pub locked: Option<String>,
    #[serde(skip)]
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
    pub worktree_state: Option<String>,
    pub is_primary: bool,
    /// Working tree symbols (?, !, +, », ✘) - used for status computation, not serialized
    #[serde(skip)]
    pub(crate) working_tree_symbols: Option<String>,
    /// is_dirty flag - used for status computation, not serialized
    #[serde(skip)]
    pub(crate) is_dirty: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_diff_display: Option<String>,
}

impl WorktreeData {
    /// Create WorktreeData from a Worktree, with all computed fields set to None.
    pub(crate) fn from_worktree(wt: &worktrunk::git::Worktree, is_primary: bool) -> Self {
        Self {
            // Identity fields (known immediately from worktree list)
            path: wt.path.clone(),
            bare: wt.bare,
            detached: wt.detached,
            locked: wt.locked.clone(),
            prunable: wt.prunable.clone(),
            is_primary,

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
    // Currently, primary worktree omits counts/branch_diff (since it doesn't compare to itself),
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
    pub fn branch_name(&self) -> &str {
        self.branch.as_deref().unwrap_or("(detached)")
    }

    pub fn is_primary(&self) -> bool {
        matches!(&self.kind, ItemKind::Worktree(data) if data.is_primary)
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
        if self.is_primary() {
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
}

/// Main branch divergence state
///
/// Represents relationship to the main/primary branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MainDivergence {
    /// Up to date with main branch
    #[default]
    None,
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

/// Upstream/remote divergence state
///
/// Represents relationship to the remote tracking branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UpstreamDivergence {
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

/// Branch state including conflicts
///
/// These states are mutually exclusive:
/// - Conflicts (= or ≠) indicate changes that differ from main
/// - MatchesMain (≡) means identical to main (can't have conflicts)
/// - NoCommits (∅) means nothing ahead of main (can't conflict)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BranchState {
    /// Normal working branch (no conflicts, doesn't match main, has commits)
    #[default]
    #[serde(rename = "")]
    None,
    /// Actual merge conflicts with main (unmerged paths in working tree)
    #[serde(rename = "=")]
    Conflicts,
    /// Potential conflicts with main (detected via --full, using git merge-tree)
    /// TODO: Implement when --full mode is complete
    #[allow(dead_code)]
    #[serde(rename = "≠")]
    PotentialConflicts,
    /// Working tree identical to main branch
    #[serde(rename = "≡")]
    MatchesMain,
    /// No commits ahead and clean working tree (not matching main)
    #[serde(rename = "∅")]
    NoCommits,
}

impl std::fmt::Display for BranchState {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::None => Ok(()),
            Self::Conflicts => write!(f, "="),
            Self::PotentialConflicts => write!(f, "≠"),
            Self::MatchesMain => write!(f, "≡"),
            Self::NoCommits => write!(f, "∅"),
        }
    }
}

/// Git operation in progress
///
/// Represents active rebase or merge operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GitOperation {
    /// No git operation in progress
    #[default]
    None,
    /// Rebase in progress
    Rebase,
    /// Merge in progress
    Merge,
}

impl std::fmt::Display for GitOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::None => Ok(()),
            Self::Rebase => write!(f, "↻"),
            Self::Merge => write!(f, "⋈"),
        }
    }
}

impl serde::Serialize for GitOperation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

/// Tracks which status symbol positions are actually used across all items
/// and the maximum width needed for each position.
///
/// This allows the Status column to:
/// 1. Only allocate space for positions that have data
/// 2. Pad each position to a consistent width for vertical alignment
///
/// Stores maximum character width for each of 7 positions (including user status).
/// A width of 0 means the position is unused.
#[derive(Debug, Clone, Copy, Default)]
pub struct PositionMask {
    /// Maximum width for each position: [0, 1, 2, 3, 4, 5, 6]
    /// 0 = position unused, >0 = max characters needed
    widths: [usize; 7],
}

impl PositionMask {
    const POS_3_WORKING_TREE: usize = 0;
    const POS_0A_BRANCH_STATE: usize = 1;
    const POS_0C_GIT_OPERATION: usize = 2;
    const POS_1_MAIN_DIVERGENCE: usize = 3;
    const POS_2_UPSTREAM_DIVERGENCE: usize = 4;
    const POS_0D_WORKTREE_ATTRS: usize = 5;
    const POS_4_USER_STATUS: usize = 6;

    /// Full mask with all positions enabled (for JSON output and progressive rendering)
    /// Allocates realistic widths based on common symbol sizes to ensure proper grid alignment
    pub const FULL: Self = Self {
        widths: [
            5, // POS_3_WORKING_TREE: ?!+»✘ (max 5 symbols)
            1, // POS_0A_BRANCH_STATE: =≠≡∅ (1 char, mutually exclusive)
            1, // POS_0C_GIT_OPERATION: ↻ or ⋈ (1 char)
            1, // POS_1_MAIN_DIVERGENCE: ↑, ↓, ↕ (1 char)
            1, // POS_2_UPSTREAM_DIVERGENCE: ⇡, ⇣, ⇅ (1 char)
            1, // POS_0D_WORKTREE_ATTRS: ⎇ for branches, ⌫⊠ for worktrees (priority-only: prunable > locked)
            2, // POS_4_USER_STATUS: single emoji or two chars (allocate 2)
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
/// - Position 0a: Conflicts or branch state (=, ≠, ≡, ∅) - mutually exclusive
/// - Position 0c: Git operation (↻, ⋈)
/// - Position 0d: Item attributes (⎇ for branches, ⌫⊠ for worktrees - priority-only)
/// - Position 1: Main branch divergence (↑, ↓, ↕)
/// - Position 2: Remote/upstream divergence (⇡, ⇣, ⇅)
/// - Position 3: Working tree symbols (?, !, +, », ✘)
/// - Position 4: User status (custom labels, emoji)
///
/// ## Mutual Exclusivity
///
/// **Mutually exclusive (enforced by type system):**
/// - = vs ≠ vs ≡ vs ∅: BranchState enum (combined position)
/// - ↻ vs ⋈: Git operation (GitOperation enum)
/// - ↑ vs ↓ vs ↕: Main divergence (MainDivergence enum)
/// - ⇡ vs ⇣ vs ⇅: Upstream divergence (UpstreamDivergence enum)
///
/// **Priority-only (can co-occur but only highest priority shown):**
/// - ⌫ vs ⊠: Worktree attrs (priority: prunable ⌫ > locked ⊠)
/// - ⎇: Branch indicator (mutually exclusive with ⌫⊠ as branches can't have worktree attrs)
///
/// **NOT mutually exclusive (can co-occur):**
/// - All working tree symbols (?!+»✘): Can have multiple types of changes
#[derive(Debug, Clone, Default)]
pub struct StatusSymbols {
    /// Branch state including conflicts (mutually exclusive)
    /// Position 0a - BranchState enum
    /// Priority: Conflicts (=) > PotentialConflicts (≠) > MatchesMain (≡) > NoCommits (∅)
    pub(crate) branch_state: BranchState,

    /// Git operation in progress
    /// Position 0c - MUTUALLY EXCLUSIVE (enforced by enum)
    pub(crate) git_operation: GitOperation,

    /// Item type attributes: ⎇ for branches, ⌫⊠ for worktrees (priority-only: prunable > locked)
    /// Position 0d - Priority-only rendering (shows highest priority symbol when multiple states exist)
    pub(crate) worktree_attrs: String,

    /// Worktree locked status - None for branches, Some("reason") or None for worktrees
    pub(crate) locked: Option<String>,

    /// Worktree prunable status - None for branches, Some("reason") or None for worktrees
    pub(crate) prunable: Option<String>,

    /// Main branch divergence state
    /// Position 1 - MUTUALLY EXCLUSIVE (enforced by enum)
    pub(crate) main_divergence: MainDivergence,

    /// Remote/upstream divergence state
    /// Position 2 - MUTUALLY EXCLUSIVE (enforced by enum)
    pub(crate) upstream_divergence: UpstreamDivergence,

    /// Working tree changes: ?, !, +, », ✘
    /// Position 3+ - NOT mutually exclusive (can have "?!+" etc.)
    pub(crate) working_tree: String,

    /// User-defined status annotation
    /// Position 4 - Custom labels (e.g., 💬, 🤖)
    pub(crate) user_status: Option<String>,
}

impl StatusSymbols {
    /// Render symbols with full alignment (all positions)
    ///
    /// This is used for the display fields in JSON output. Skipped on Windows
    /// to avoid an unused/dead-code warning in clippy (the interactive selector
    /// that calls this exists only on Unix).
    #[cfg(unix)]
    pub fn render(&self) -> String {
        self.render_with_mask(&PositionMask::FULL)
    }

    /// Render symbols with selective alignment based on position mask
    ///
    /// Aligns all symbol types at fixed positions, but only includes positions
    /// that are present in the mask:
    /// - Position 0a: Conflicts or branch state (=, ≠, ≡, ∅)
    /// - Position 0c: Git operation (↻, ⋈, or space)
    /// - Position 0d: Worktree attributes (⊠⚠ or space)
    /// - Position 1: Main divergence (↑, ↓, ↕, or space)
    /// - Position 2: Upstream divergence (⇡, ⇣, ⇅, or space)
    /// - Position 3: Working tree symbols (?, !, +, », ✘)
    /// - Position 4: User status (custom labels, emoji)
    ///
    /// This ensures vertical scannability - each symbol type appears at the same
    /// column position across all rows, while minimizing wasted space.
    pub fn render_with_mask(&self, mask: &PositionMask) -> String {
        use unicode_width::UnicodeWidthStr;
        use worktrunk::styling::{CYAN, ERROR, HINT, WARNING};

        let mut result = String::with_capacity(12);

        if self.is_empty() {
            return result;
        }

        // Build list of (position_index, content, has_data) tuples
        // Ordered by importance/actionability
        // Apply colors based on semantic meaning:
        // - Red (ERROR): Actual conflicts (blocking problems)
        // - Yellow (WARNING): Potential conflicts, git operations, locked/prunable (active/stuck states)
        // - Cyan: Working tree changes (activity)
        // - Dimmed (HINT): Branch state symbols that indicate removability
        let branch_state_str = match self.branch_state {
            BranchState::Conflicts => format!("{ERROR}={ERROR:#}"),
            BranchState::PotentialConflicts => format!("{WARNING}≠{WARNING:#}"),
            BranchState::MatchesMain => format!("{HINT}≡{HINT:#}"),
            BranchState::NoCommits => format!("{HINT}∅{HINT:#}"),
            BranchState::None => String::new(),
        };
        let git_operation_str = if self.git_operation != GitOperation::None {
            format!("{WARNING}{}{WARNING:#}", self.git_operation)
        } else {
            String::new()
        };
        let main_divergence_str = self.main_divergence.to_string();
        let upstream_divergence_str = self.upstream_divergence.to_string();
        let working_tree_str = if !self.working_tree.is_empty() {
            format!("{CYAN}{}{CYAN:#}", self.working_tree)
        } else {
            String::new()
        };
        let worktree_attrs_str = if !self.worktree_attrs.is_empty() {
            // Branch indicator (⎇) is informational (dimmed), worktree attrs (⌫⊠) are warnings (yellow)
            if self.worktree_attrs == "⎇" {
                format!("{HINT}{}{HINT:#}", self.worktree_attrs)
            } else {
                format!("{WARNING}{}{WARNING:#}", self.worktree_attrs)
            }
        } else {
            String::new()
        };
        let user_status_str = self.user_status.as_deref().unwrap_or("").to_string();

        // Track (position, styled_content, visual_width, has_data)
        // visual_width is the actual display width without ANSI codes
        //
        // CRITICAL: Display order is working_tree first, then other symbols.
        // NEVER change this order - it ensures progressive and final rendering match exactly.
        // Tests will break if you change this, but that's expected - update the tests, not this order.
        let positions_data: [(usize, &str, usize, bool); 7] = [
            (
                PositionMask::POS_3_WORKING_TREE,
                working_tree_str.as_str(),
                self.working_tree.width(),
                !self.working_tree.is_empty(),
            ),
            (
                PositionMask::POS_0A_BRANCH_STATE,
                branch_state_str.as_str(),
                if self.branch_state != BranchState::None {
                    1
                } else {
                    0
                },
                self.branch_state != BranchState::None,
            ),
            (
                PositionMask::POS_0C_GIT_OPERATION,
                git_operation_str.as_str(),
                self.git_operation.to_string().width(),
                self.git_operation != GitOperation::None,
            ),
            (
                PositionMask::POS_1_MAIN_DIVERGENCE,
                main_divergence_str.as_str(),
                self.main_divergence.to_string().width(),
                self.main_divergence != MainDivergence::None,
            ),
            (
                PositionMask::POS_2_UPSTREAM_DIVERGENCE,
                upstream_divergence_str.as_str(),
                self.upstream_divergence.to_string().width(),
                self.upstream_divergence != UpstreamDivergence::None,
            ),
            (
                PositionMask::POS_0D_WORKTREE_ATTRS,
                worktree_attrs_str.as_str(),
                self.worktree_attrs.width(),
                !self.worktree_attrs.is_empty(),
            ),
            (
                PositionMask::POS_4_USER_STATUS,
                user_status_str.as_str(),
                self.user_status.as_ref().map(|s| s.width()).unwrap_or(0),
                self.user_status.is_some(),
            ),
        ];

        // Grid-based rendering: each position gets a fixed width for vertical alignment.
        // CRITICAL: Always use PositionMask::FULL for consistent spacing between progressive and final rendering.
        // The mask provides the maximum width needed for each position across all rows.
        // Accept wider Status column with whitespace as tradeoff for perfect alignment.
        for (pos, styled_content, visual_width, has_data) in positions_data.iter() {
            let allocated_width = mask.width(*pos);

            if *has_data {
                result.push_str(styled_content);
                // Pad to allocated width for alignment
                let padding = allocated_width.saturating_sub(*visual_width);
                for _ in 0..padding {
                    result.push(' ');
                }
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
        self.branch_state == BranchState::None
            && self.git_operation == GitOperation::None
            && self.worktree_attrs.is_empty()
            && self.main_divergence == MainDivergence::None
            && self.upstream_divergence == UpstreamDivergence::None
            && self.working_tree.is_empty()
            && self.user_status.is_none()
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

/// Worktree attributes in status (locked/prunable info)
#[derive(Debug, Clone, serde::Serialize)]
struct WorktreeAttrsStatus {
    locked: Option<String>,
    prunable: Option<String>,
}

/// Status variant names (for queryability)
#[derive(Debug, Clone, serde::Serialize)]
struct StatusValues {
    branch_state: &'static str,
    git_operation: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    worktree_attrs: Option<WorktreeAttrsStatus>,
    main_divergence: &'static str,
    upstream_divergence: &'static str,
    working_tree: WorkingTreeChanges,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_status: Option<String>,
}

/// Status symbols (for display)
#[derive(Debug, Clone, serde::Serialize)]
struct StatusSymbolsOnly {
    branch_state: String,
    git_operation: String,
    worktree_attrs: String,
    main_divergence: String,
    upstream_divergence: String,
    working_tree: String,
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

        // Status variant names
        let branch_state_variant = match self.branch_state {
            BranchState::None => "",
            BranchState::Conflicts => "Conflicts",
            BranchState::PotentialConflicts => "PotentialConflicts",
            BranchState::MatchesMain => "MatchesMain",
            BranchState::NoCommits => "NoCommits",
        };

        let git_operation_variant = match self.git_operation {
            GitOperation::None => "",
            GitOperation::Rebase => "Rebase",
            GitOperation::Merge => "Merge",
        };

        let main_divergence_variant = match self.main_divergence {
            MainDivergence::None => "",
            MainDivergence::Ahead => "Ahead",
            MainDivergence::Behind => "Behind",
            MainDivergence::Diverged => "Diverged",
        };

        let upstream_divergence_variant = match self.upstream_divergence {
            UpstreamDivergence::None => "",
            UpstreamDivergence::Ahead => "Ahead",
            UpstreamDivergence::Behind => "Behind",
            UpstreamDivergence::Diverged => "Diverged",
        };

        // Create worktree_attrs status if this is a worktree (has locked/prunable)
        let worktree_attrs_status = if self.locked.is_some() || self.prunable.is_some() {
            Some(WorktreeAttrsStatus {
                locked: self.locked.clone(),
                prunable: self.prunable.clone(),
            })
        } else {
            None
        };

        let status_values = StatusValues {
            branch_state: branch_state_variant,
            git_operation: git_operation_variant,
            worktree_attrs: worktree_attrs_status,
            main_divergence: main_divergence_variant,
            upstream_divergence: upstream_divergence_variant,
            working_tree: WorkingTreeChanges::from_symbols(&self.working_tree),
            user_status: self.user_status.clone(),
        };

        let status_symbols = StatusSymbolsOnly {
            branch_state: self.branch_state.to_string(),
            git_operation: self.git_operation.to_string(),
            worktree_attrs: self.worktree_attrs.clone(),
            main_divergence: self.main_divergence.to_string(),
            upstream_divergence: self.upstream_divergence.to_string(),
            working_tree: self.working_tree.clone(),
            user_status: self.user_status.clone(),
        };

        state.serialize_field("status", &status_values)?;
        state.serialize_field("status_symbols", &status_symbols)?;

        state.end()
    }
}
