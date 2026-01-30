//! State enums for worktree and branch status.
//!
//! These represent various states a worktree or branch can be in relative to
//! the default branch, upstream remote, or git operations in progress.

use worktrunk::git::IntegrationReason;

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
/// Priority order for worktrees: BranchWorktreeMismatch > Prunable > Locked
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, strum::IntoStaticStr)]
pub enum WorktreeState {
    #[strum(serialize = "")]
    /// Normal worktree (path matches template, not locked or prunable)
    #[default]
    None,
    /// Branch-worktree mismatch: path doesn't match what the template would generate
    BranchWorktreeMismatch,
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
            Self::BranchWorktreeMismatch => write!(f, "⚑"),
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

/// Default branch relationship state
///
/// Represents the combined relationship to the default branch in a single position.
/// Uses horizontal arrows (vs vertical arrows for Remote column).
///
/// Priority order determines which symbol is shown:
/// 1. IsMain (^) - this IS the main worktree
/// 2. Orphan (∅) - no common ancestor with default branch
/// 3. WouldConflict (✗) - merge-tree simulation shows conflicts
/// 4. Empty (_) - same commit as default branch AND clean working tree (safe to delete)
/// 5. SameCommit (–) - same commit as default branch with uncommitted changes
/// 6. Integrated (⊂) - content is in default branch via different history
/// 7. Diverged (↕) - both ahead and behind default branch
/// 8. Ahead (↑) - has commits default branch doesn't have
/// 9. Behind (↓) - missing commits from default branch
///
/// The `Integrated` variant carries an [`IntegrationReason`] explaining how the
/// content was integrated (ancestor, trees match, no added changes, or merge adds nothing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum MainState {
    /// Normal working branch (up-to-date with default branch, no special state)
    #[default]
    #[strum(serialize = "")]
    None,
    /// This IS the main worktree
    IsMain,
    /// Merge-tree conflicts with default branch (simulated via git merge-tree)
    WouldConflict,
    /// Branch HEAD is same commit as default branch AND working tree is clean (safe to delete)
    Empty,
    /// Branch HEAD is same commit as default branch but has uncommitted changes
    SameCommit,
    /// Content is integrated into default branch via different history
    #[strum(serialize = "integrated")]
    Integrated(IntegrationReason),
    /// No common ancestor with default branch (orphan branch)
    Orphan,
    /// Both ahead and behind default branch
    Diverged,
    /// Has commits default branch doesn't have
    Ahead,
    /// Missing commits from default branch
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
            Self::Orphan => write!(f, "∅"), // U+2205 empty set
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
        let s: &'static str = self.into();
        if s.is_empty() { None } else { Some(s) }
    }

    /// Compute from divergence counts, integration state, and same-commit-dirty flag.
    ///
    /// Priority: IsMain > Orphan > WouldConflict > integration > SameCommit > Diverged > Ahead > Behind
    ///
    /// Orphan takes priority over WouldConflict because:
    /// - Orphan is a fundamental property (no common ancestor)
    /// - Merge conflicts for orphan branches are expected but not actionable normally
    /// - Users should understand "this is an orphan branch" rather than "this would conflict"
    pub fn from_integration_and_counts(
        is_main: bool,
        would_conflict: bool,
        integration: Option<MainState>,
        is_same_commit_dirty: bool,
        is_orphan: bool,
        ahead: usize,
        behind: usize,
    ) -> Self {
        if is_main {
            Self::IsMain
        } else if is_orphan {
            Self::Orphan
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum OperationState {
    /// No operation in progress
    #[default]
    #[strum(serialize = "")]
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
        let s: &'static str = self.into();
        if s.is_empty() { None } else { Some(s) }
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

/// Active git operation in a worktree
///
/// Represents raw data about whether a worktree is in the middle of a git operation.
/// This is distinct from [`OperationState`] which is the display enum (includes Conflicts,
/// has symbols/colors).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, strum::IntoStaticStr)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum ActiveGitOperation {
    #[strum(serialize = "")]
    #[serde(rename = "")]
    #[default]
    None,
    /// Rebase in progress (rebase-merge or rebase-apply directory exists)
    Rebase,
    /// Merge in progress (MERGE_HEAD exists)
    Merge,
}

impl ActiveGitOperation {
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(format!("{}", WorktreeState::BranchWorktreeMismatch), "⚑");
        assert_eq!(format!("{}", WorktreeState::Prunable), "⊟");
        assert_eq!(format!("{}", WorktreeState::Locked), "⊞");
        assert_eq!(format!("{}", WorktreeState::Branch), "/");
    }

    #[test]
    fn test_worktree_state_serialize() {
        // Serialize to JSON and check the string representation
        let json = serde_json::to_string(&WorktreeState::None).unwrap();
        assert_eq!(json, "\"\"");

        let json = serde_json::to_string(&WorktreeState::BranchWorktreeMismatch).unwrap();
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
        assert_eq!(format!("{}", MainState::Orphan), "∅"); // empty set
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

        // Orphan is dimmed (informational, not a warning)
        let styled = MainState::Orphan.styled().unwrap();
        assert!(styled.contains("∅"));
    }

    #[test]
    fn test_main_state_serialize() {
        let json = serde_json::to_string(&MainState::None).unwrap();
        assert_eq!(json, "\"\"");

        let json = serde_json::to_string(&MainState::IsMain).unwrap();
        assert_eq!(json, "\"^\"");

        let json = serde_json::to_string(&MainState::Diverged).unwrap();
        assert_eq!(json, "\"↕\"");

        let json = serde_json::to_string(&MainState::Orphan).unwrap();
        assert_eq!(json, "\"∅\"");
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
            MainState::from_integration_and_counts(true, false, None, false, false, 5, 3),
            MainState::IsMain
        ));

        // Orphan takes priority over WouldConflict (orphan is root cause)
        assert!(matches!(
            MainState::from_integration_and_counts(false, true, None, false, true, 0, 0),
            MainState::Orphan
        ));

        // WouldConflict when not orphan
        assert!(matches!(
            MainState::from_integration_and_counts(false, true, None, false, false, 5, 3),
            MainState::WouldConflict
        ));

        // Empty (passed as integration state - same commit with clean working tree)
        assert!(matches!(
            MainState::from_integration_and_counts(
                false,
                false,
                Some(MainState::Empty),
                false,
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
                false,
                0,
                5
            ),
            MainState::Integrated(IntegrationReason::Ancestor)
        ));

        // SameCommit (via is_same_commit_dirty flag, NOT integration)
        assert!(matches!(
            MainState::from_integration_and_counts(false, false, None, true, false, 0, 0),
            MainState::SameCommit
        ));

        // Orphan (no common ancestor with default branch)
        assert!(matches!(
            MainState::from_integration_and_counts(false, false, None, false, true, 0, 0),
            MainState::Orphan
        ));

        // Diverged (both ahead and behind)
        assert!(matches!(
            MainState::from_integration_and_counts(false, false, None, false, false, 3, 2),
            MainState::Diverged
        ));

        // Ahead only
        assert!(matches!(
            MainState::from_integration_and_counts(false, false, None, false, false, 3, 0),
            MainState::Ahead
        ));

        // Behind only
        assert!(matches!(
            MainState::from_integration_and_counts(false, false, None, false, false, 0, 2),
            MainState::Behind
        ));

        // None (in sync)
        assert!(matches!(
            MainState::from_integration_and_counts(false, false, None, false, false, 0, 0),
            MainState::None
        ));
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

    #[test]
    fn test_operation_state_as_json_str() {
        assert_eq!(OperationState::None.as_json_str(), None);
        assert_eq!(OperationState::Conflicts.as_json_str(), Some("conflicts"));
        assert_eq!(OperationState::Rebase.as_json_str(), Some("rebase"));
        assert_eq!(OperationState::Merge.as_json_str(), Some("merge"));
    }

    // ============================================================================
    // ActiveGitOperation Tests
    // ============================================================================

    #[test]
    fn test_git_operation_state_default() {
        let state = ActiveGitOperation::default();
        assert_eq!(state, ActiveGitOperation::None);
    }

    #[test]
    fn test_git_operation_state_is_none() {
        assert!(ActiveGitOperation::None.is_none());
        assert!(!ActiveGitOperation::Rebase.is_none());
        assert!(!ActiveGitOperation::Merge.is_none());
    }
}
