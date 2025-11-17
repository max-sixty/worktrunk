use std::path::PathBuf;
use worktrunk::git::{GitError, LineDiff, Repository};

use super::ci_status::PrStatus;
use super::columns::ColumnKind;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_display: Option<String>,
}

impl DisplayFields {
    pub(crate) fn from_common_fields(
        counts: &AheadBehind,
        branch_diff: &BranchDiffTotals,
        upstream: &UpstreamStatus,
        pr_status: &Option<PrStatus>,
    ) -> Self {
        let commits_display =
            ColumnKind::AheadBehind.format_diff_plain(counts.ahead, counts.behind);

        let branch_diff_display = ColumnKind::BranchDiff
            .format_diff_plain(branch_diff.diff.added, branch_diff.diff.deleted);

        let upstream_display =
            upstream
                .active()
                .and_then(|(_, upstream_ahead, upstream_behind)| {
                    ColumnKind::Upstream.format_diff_plain(upstream_ahead, upstream_behind)
                });

        let ci_status_display = pr_status.as_ref().map(PrStatus::format_plain);

        Self {
            commits_display,
            branch_diff_display,
            upstream_display,
            ci_status_display,
            status_display: None,
        }
    }
}

#[derive(serde::Serialize)]
pub struct WorktreeInfo {
    // Worktree identity fields (flattened from worktrunk::git::Worktree)
    pub path: PathBuf,
    #[serde(rename = "head_sha")]
    pub head: String,
    pub branch: Option<String>,
    pub bare: bool,
    pub detached: bool,
    pub locked: Option<String>,
    pub prunable: Option<String>,

    // Commit details
    #[serde(flatten)]
    pub commit: CommitDetails,

    // Divergence from main
    #[serde(flatten)]
    pub counts: AheadBehind,
    #[serde(flatten)]
    pub branch_diff: BranchDiffTotals,

    // Working tree state
    pub working_tree_diff: LineDiff,
    /// Diff between working tree and main branch.
    /// `None` means "not computed" (optimization: skipped when trees differ).
    /// `Some((0, 0))` means working tree matches main exactly.
    /// `Some((a, d))` means a lines added, d deleted vs main.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_tree_diff_with_main: Option<LineDiff>,
    pub worktree_state: Option<String>,
    pub has_conflicts: bool,

    // Metadata
    pub is_primary: bool,

    // Remote/upstream
    #[serde(flatten)]
    pub upstream: UpstreamStatus,

    // Status
    pub pr_status: Option<PrStatus>,
    /// Git status symbols (=, ↑, ↓, ⇡, ⇣, ?, !, +, », ✘) including user-defined status
    pub status_symbols: StatusSymbols,

    // Display fields for json-pretty format (with ANSI colors)
    #[serde(flatten)]
    pub display: DisplayFields,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_diff_display: Option<String>,
}

#[derive(serde::Serialize)]
pub struct BranchInfo {
    pub name: String,
    #[serde(rename = "head_sha")]
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
    /// User-defined status from `worktrunk.status.<branch>` git config
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_status: Option<String>,

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
    pub diff: LineDiff,
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
#[allow(clippy::large_enum_variant)]
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
            ListItem::Worktree(wt) => wt.branch.as_deref().unwrap_or("(detached)"),
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
            ListItem::Worktree(info) => &info.head,
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
        self.worktree_info().map(|info| &info.path)
    }

    pub fn pr_status(&self) -> Option<&PrStatus> {
        match self {
            ListItem::Worktree(info) => info.pr_status.as_ref(),
            ListItem::Branch(info) => info.pr_status.as_ref(),
        }
    }

    /// Determine if the item contains no unique work and can likely be removed.
    pub(crate) fn is_potentially_removable(&self) -> bool {
        if self.is_primary() {
            return false;
        }

        let counts = self.counts();

        if let Some(info) = self.worktree_info() {
            let no_commits_and_clean = counts.ahead == 0 && info.working_tree_diff.is_empty();
            let matches_main = info
                .working_tree_diff_with_main
                .is_some_and(|diff| diff.is_empty());
            no_commits_and_clean || matches_main
        } else {
            counts.ahead == 0
        }
    }
}

impl BranchInfo {
    /// Create BranchInfo from a branch name, enriching it with git metadata
    pub(crate) fn from_branch(
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

        // Read user-defined status from git config (branch-keyed only, no worktree)
        let user_status = repo.branch_keyed_status(branch);

        // Create display fields with status
        // For branches without worktrees, status is just user status or "·"
        let status_display = Some(user_status.clone().unwrap_or_else(|| "·".to_string()));

        let display = DisplayFields {
            status_display,
            ..Default::default()
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
            user_status,
            display,
        })
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

/// Branch state relative to main/primary branch
///
/// Represents whether the branch matches main or has no commits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BranchState {
    /// Normal branch state (neither matches main nor empty)
    #[default]
    None,
    /// Working tree identical to main branch
    MatchesMain,
    /// No commits ahead and clean working tree (not matching main)
    NoCommits,
}

impl std::fmt::Display for BranchState {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::None => Ok(()),
            Self::MatchesMain => write!(f, "≡"),
            Self::NoCommits => write!(f, "∅"),
        }
    }
}

impl serde::Serialize for BranchState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
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
/// Stores maximum character width for each of 8 positions (including user status).
/// A width of 0 means the position is unused.
#[derive(Debug, Clone, Copy, Default)]
pub struct PositionMask {
    /// Maximum width for each position: [0, 1, 2, 3, 4, 5, 6, 7]
    /// 0 = position unused, >0 = max characters needed
    widths: [usize; 8],
}

impl PositionMask {
    const POS_3_WORKING_TREE: usize = 0;
    const POS_0A_CONFLICTS: usize = 1;
    const POS_0C_GIT_OPERATION: usize = 2;
    const POS_1_MAIN_DIVERGENCE: usize = 3;
    const POS_2_UPSTREAM_DIVERGENCE: usize = 4;
    const POS_0B_BRANCH_STATE: usize = 5;
    const POS_0D_WORKTREE_ATTRS: usize = 6;
    const POS_4_USER_STATUS: usize = 7;

    /// Full mask with all positions enabled (for JSON output)
    /// Uses width of 1 for each position as a placeholder
    pub const FULL: Self = Self { widths: [1; 8] };

    /// Merge this mask with another, keeping the maximum width for each position
    pub fn merge(&mut self, other: &Self) {
        for (i, &other_width) in other.widths.iter().enumerate() {
            self.widths[i] = self.widths[i].max(other_width);
        }
    }

    /// Check if a position is included in the mask (width > 0)
    fn includes(&self, pos: usize) -> bool {
        self.widths[pos] > 0
    }

    /// Get the width allocated for a position
    fn width(&self, pos: usize) -> usize {
        self.widths[pos]
    }
}

/// Structured status symbols for aligned rendering
///
/// Symbols are categorized to enable vertical alignment in table output:
/// - Position 0a: Conflicts (=)
/// - Position 0b: Branch state (≡, ∅)
/// - Position 0c: Git operation (↻, ⋈)
/// - Position 0d: Worktree attributes (◇, ⊠, ⚠)
/// - Position 1: Main branch divergence (↑, ↓, ↕)
/// - Position 2: Remote/upstream divergence (⇡, ⇣, ⇅)
/// - Position 3: Working tree symbols (?, !, +, », ✘)
/// - Position 4: User status (custom labels, emoji)
///
/// ## Mutual Exclusivity
///
/// **Mutually exclusive (enforced by type system):**
/// - ≡ vs ∅: Branch state (BranchState enum)
/// - ↻ vs ⋈: Git operation (GitOperation enum)
/// - ↑ vs ↓ vs ↕: Main divergence (MainDivergence enum)
/// - ⇡ vs ⇣ vs ⇅: Upstream divergence (UpstreamDivergence enum)
///
/// **NOT mutually exclusive (can co-occur):**
/// - = can occur with any other symbol
/// - ◇, ⊠, ⚠: Worktree can be bare+locked, bare+prunable, etc.
/// - All working tree symbols (?!+»✘): Can have multiple types of changes
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct StatusSymbols {
    /// Merge conflicts indicator
    /// Position 0a - Boolean flag (= or empty)
    pub(crate) has_conflicts: bool,

    /// Branch state relative to main
    /// Position 0b - MUTUALLY EXCLUSIVE (enforced by enum)
    pub(crate) branch_state: BranchState,

    /// Git operation in progress
    /// Position 0c - MUTUALLY EXCLUSIVE (enforced by enum)
    pub(crate) git_operation: GitOperation,

    /// Worktree attributes: ◇, ⊠, ⚠
    /// Position 0d - NOT mutually exclusive (can combine like "◇⊠")
    pub(crate) worktree_attrs: String,

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
    /// This is used for the display fields in JSON output.
    /// For table rendering with selective positions, use `render_with_mask()`.
    pub fn render(&self) -> String {
        self.render_with_mask(&PositionMask::FULL)
    }

    /// Render symbols with selective alignment based on position mask
    ///
    /// Aligns all symbol types at fixed positions, but only includes positions
    /// that are present in the mask:
    /// - Position 0a: Conflicts (= or space)
    /// - Position 0b: Branch state (≡, ∅, or space)
    /// - Position 0c: Git operation (↻, ⋈, or space)
    /// - Position 0d: Worktree attributes (◇⊠⚠ or space)
    /// - Position 1: Main divergence (↑, ↓, ↕, or space)
    /// - Position 2: Upstream divergence (⇡, ⇣, ⇅, or space)
    /// - Position 3: Working tree symbols (?, !, +, », ✘)
    /// - Position 4: User status (custom labels, emoji)
    ///
    /// This ensures vertical scannability - each symbol type appears at the same
    /// column position across all rows, while minimizing wasted space.
    pub fn render_with_mask(&self, mask: &PositionMask) -> String {
        let mut result = String::with_capacity(12);

        if self.is_empty() {
            return result;
        }

        // Build list of (position_index, content, has_data) tuples
        // Ordered by importance/actionability
        let conflicts_str = if self.has_conflicts {
            "=".to_string()
        } else {
            String::new()
        };
        let git_operation_str = self.git_operation.to_string();
        let main_divergence_str = self.main_divergence.to_string();
        let upstream_divergence_str = self.upstream_divergence.to_string();
        let branch_state_str = self.branch_state.to_string();
        let user_status_str = self.user_status.as_deref().unwrap_or("").to_string();

        let positions_data = [
            (
                PositionMask::POS_3_WORKING_TREE,
                &self.working_tree,
                !self.working_tree.is_empty(),
            ),
            (
                PositionMask::POS_0A_CONFLICTS,
                &conflicts_str,
                self.has_conflicts,
            ),
            (
                PositionMask::POS_0C_GIT_OPERATION,
                &git_operation_str,
                self.git_operation != GitOperation::None,
            ),
            (
                PositionMask::POS_1_MAIN_DIVERGENCE,
                &main_divergence_str,
                self.main_divergence != MainDivergence::None,
            ),
            (
                PositionMask::POS_2_UPSTREAM_DIVERGENCE,
                &upstream_divergence_str,
                self.upstream_divergence != UpstreamDivergence::None,
            ),
            (
                PositionMask::POS_0B_BRANCH_STATE,
                &branch_state_str,
                self.branch_state != BranchState::None,
            ),
            (
                PositionMask::POS_0D_WORKTREE_ATTRS,
                &self.worktree_attrs,
                !self.worktree_attrs.is_empty(),
            ),
            (
                PositionMask::POS_4_USER_STATUS,
                &user_status_str,
                self.user_status.is_some(),
            ),
        ];

        // Grid-based rendering with padding: each position gets a fixed-width column
        // - If row has content at position: append content, then pad to allocated width
        // - If row has no content at position: fill with spaces to allocated width
        use unicode_width::UnicodeWidthStr;

        for (pos, content, has_data) in positions_data.iter() {
            if !mask.includes(*pos) {
                continue; // Skip positions not in mask
            }

            let allocated_width = mask.width(*pos);

            if *has_data {
                result.push_str(content);
                // Pad to allocated width (use saturating_sub to handle edge cases)
                let content_width = content.width();
                let padding = allocated_width.saturating_sub(content_width);
                for _ in 0..padding {
                    result.push(' ');
                }
            } else {
                // Fill empty column with spaces
                for _ in 0..allocated_width {
                    result.push(' ');
                }
            }
        }

        result
    }

    /// Derive a position mask that tracks which symbol slots contain data.
    pub fn position_mask(&self) -> PositionMask {
        use unicode_width::UnicodeWidthStr;

        let mut widths = [0; 8];

        widths[PositionMask::POS_3_WORKING_TREE] = self.working_tree.width();
        widths[PositionMask::POS_0A_CONFLICTS] = if self.has_conflicts { 1 } else { 0 };
        widths[PositionMask::POS_0C_GIT_OPERATION] = self.git_operation.to_string().width();
        widths[PositionMask::POS_1_MAIN_DIVERGENCE] = self.main_divergence.to_string().width();
        widths[PositionMask::POS_2_UPSTREAM_DIVERGENCE] =
            self.upstream_divergence.to_string().width();
        widths[PositionMask::POS_0B_BRANCH_STATE] = self.branch_state.to_string().width();
        widths[PositionMask::POS_0D_WORKTREE_ATTRS] = self.worktree_attrs.width();
        widths[PositionMask::POS_4_USER_STATUS] =
            self.user_status.as_ref().map(|s| s.width()).unwrap_or(0);

        PositionMask { widths }
    }

    /// Check if symbols are empty
    pub fn is_empty(&self) -> bool {
        !self.has_conflicts
            && self.branch_state == BranchState::None
            && self.git_operation == GitOperation::None
            && self.worktree_attrs.is_empty()
            && self.main_divergence == MainDivergence::None
            && self.upstream_divergence == UpstreamDivergence::None
            && self.working_tree.is_empty()
            && self.user_status.is_none()
    }
}

/// Git status information parsed from `git status --porcelain`
struct GitStatusInfo {
    /// Whether the working tree has any changes (staged or unstaged)
    is_dirty: bool,
    /// Status symbols (structured for alignment)
    symbols: StatusSymbols,
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

    // Build working tree string
    let mut working_tree = String::new();
    if has_untracked {
        working_tree.push('?');
    }
    if has_modified {
        working_tree.push('!');
    }
    if has_staged {
        working_tree.push('+');
    }
    if has_renamed {
        working_tree.push('»');
    }
    if has_deleted {
        working_tree.push('✘');
    }

    // Build structured symbols for aligned rendering
    let symbols = StatusSymbols {
        has_conflicts,
        main_divergence: match (main_ahead > 0, main_behind > 0) {
            (true, true) => MainDivergence::Diverged, // Both ahead and behind
            (true, false) => MainDivergence::Ahead,   // Ahead only
            (false, true) => MainDivergence::Behind,  // Behind only
            (false, false) => MainDivergence::None,   // Up to date
        },
        upstream_divergence: match (upstream_ahead > 0, upstream_behind > 0) {
            (true, true) => UpstreamDivergence::Diverged, // Both ahead and behind
            (true, false) => UpstreamDivergence::Ahead,   // Ahead only
            (false, true) => UpstreamDivergence::Behind,  // Behind only
            (false, false) => UpstreamDivergence::None,   // Up to date
        },
        working_tree,
        ..Default::default()
    };

    Ok(GitStatusInfo { is_dirty, symbols })
}

impl WorktreeInfo {
    /// Create WorktreeInfo from a Worktree, enriching it with git metadata
    pub(crate) fn from_worktree(
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
            LineDiff::default() // Clean working tree
        };

        // Use tree equality check instead of expensive diff for "matches main"
        let working_tree_diff_with_main =
            wt_repo.working_tree_diff_with_base(base_branch, status_info.is_dirty)?;
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

        // Build complete status symbols with type-safe enums
        // Order: = ≡∅ ↻⋈ ◇⊠⚠ | ↑↓ | ⇡⇣ | ?!+»✘
        //        ^0a 0b 0c 0d^  ^1^  ^2^  ^3+^
        let mut symbols = status_info.symbols;

        // Add merge conflicts indicator if this branch has conflicts with base
        // (different from git status conflicts which are already in has_conflicts)
        if has_conflicts {
            symbols.has_conflicts = true;
        }

        // Branch state: ≡ matches main, ∅ no commits (mutually exclusive)
        if !is_primary {
            if working_tree_diff_with_main.is_some_and(|diff| diff.is_empty()) {
                symbols.branch_state = BranchState::MatchesMain;
            } else if counts.ahead == 0 && working_tree_diff.is_empty() {
                symbols.branch_state = BranchState::NoCommits;
            }
        }

        // Git operation: ↻ rebase, ⋈ merge (mutually exclusive)
        if let Some(state) = &worktree_state {
            if state.contains("rebase") {
                symbols.git_operation = GitOperation::Rebase;
            } else if state.contains("merge") {
                symbols.git_operation = GitOperation::Merge;
            }
        }

        // Worktree attributes: ◇ bare, ⊠ locked, ⚠ prunable (can combine)
        if wt.bare {
            symbols.worktree_attrs.push('◇');
        }
        if wt.locked.is_some() {
            symbols.worktree_attrs.push('⊠');
        }
        if wt.prunable.is_some() {
            symbols.worktree_attrs.push('⚠');
        }

        // Add user-defined status from git config to symbols (Position 4)
        symbols.user_status = wt_repo.user_status(wt.branch.as_deref());

        // Create display fields with rendered status
        let status_display = if !symbols.is_empty() {
            Some(symbols.render())
        } else {
            None
        };

        let display = DisplayFields {
            status_display,
            ..Default::default()
        };

        Ok(WorktreeInfo {
            // Flatten worktree fields
            path: wt.path.clone(),
            head: wt.head.clone(),
            branch: wt.branch.clone(),
            bare: wt.bare,
            detached: wt.detached,
            locked: wt.locked.clone(),
            prunable: wt.prunable.clone(),
            // Remaining fields
            commit,
            counts,
            branch_diff,
            working_tree_diff,
            working_tree_diff_with_main,
            worktree_state,
            has_conflicts,
            is_primary,
            upstream,
            pr_status,
            status_symbols: symbols,
            display,
            working_diff_display: None,
        })
    }
}
