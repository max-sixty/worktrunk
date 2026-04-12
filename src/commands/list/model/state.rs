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
    ///
    /// This function takes every input up front — it's the "all data is
    /// available" path. For the per-gate resolver that walks tiers with
    /// partial data, see [`tier_is_main`], [`tier_orphan`],
    /// [`tier_would_conflict`], [`tier_integration_or_counts`], and the
    /// [`Tier`] helper below.
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

/// Result of attempting to resolve a single tier of the main_state priority
/// chain with partial data.
///
/// The main-state gate (`refresh_status_symbols` gate 3) walks tiers in
/// priority order. Each tier returns one of:
///
/// - [`Tier::Fired`] — this tier's signal is both known and positive; use
///   the carried value and ignore lower-priority tiers.
/// - [`Tier::RuledOut`] — this tier's signal is known to be negative; move
///   on to the next (lower-priority) tier.
/// - [`Tier::Wait`] — this tier's signal is not yet loaded; we cannot
///   safely fall through to a lower tier because a later drain tick might
///   still produce a higher-priority answer. Stop and return `None` from
///   the gate resolver so the cell renders `⋯`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier<T> {
    /// Tier signal is known and positive — short-circuit with this value.
    Fired(T),
    /// Tier signal is known and negative — fall through to the next tier.
    RuledOut,
    /// Tier signal not yet loaded — the gate must wait.
    Wait,
}

/// Tier 1: `IsMain`. Resolves immediately since `is_main` is metadata.
///
/// Returns `Fired(IsMain)` for main worktrees, `RuledOut` otherwise.
pub fn tier_is_main(is_main: bool) -> Tier<MainState> {
    if is_main {
        Tier::Fired(MainState::IsMain)
    } else {
        Tier::RuledOut
    }
}

/// Tier 2: `Orphan`. Requires `is_orphan` to be loaded.
///
/// Orphan branches have no common ancestor with the default branch, so
/// every lower-priority signal (ahead/behind, integration, conflict) is
/// meaningless for them. Once we know `is_orphan == Some(true)`, the gate
/// short-circuits without needing anything else.
pub fn tier_orphan(is_orphan: Option<bool>) -> Tier<MainState> {
    match is_orphan {
        Some(true) => Tier::Fired(MainState::Orphan),
        Some(false) => Tier::RuledOut,
        None => Tier::Wait,
    }
}

/// Tier 3: `WouldConflict`. Requires *both* conflict probes to be loaded.
///
/// `has_merge_tree_conflicts` is the committed-HEAD probe (from
/// `MergeTreeConflicts`). `has_working_tree_conflicts` is the dirty-tree
/// probe (from `WorkingTreeConflicts`), with a nested Option: outer `None`
/// = task not run, `Some(None)` = task ran but working tree is clean (no
/// dirty-tree result, fall back to the HEAD probe), `Some(Some(b))` =
/// dirty-tree result.
///
/// Behavior:
/// - If either probe reports `true`, fire `WouldConflict` — no need to
///   wait for the other.
/// - If the HEAD probe is `None`, wait (we can't rule out a conflict
///   without it).
/// - If the HEAD probe is `Some(false)` and the working-tree probe is
///   `None`, wait — the working-tree result could still flip us to
///   `WouldConflict`.
/// - If both report "no conflict" (HEAD probe `Some(false)` and
///   working-tree probe either `Some(None)` or `Some(Some(false))`), rule
///   out.
pub fn tier_would_conflict(
    has_merge_tree_conflicts: Option<bool>,
    has_working_tree_conflicts: Option<Option<bool>>,
) -> Tier<MainState> {
    // Working-tree probe short-circuit: if it reports a dirty conflict,
    // fire regardless of the HEAD probe.
    if let Some(Some(true)) = has_working_tree_conflicts {
        return Tier::Fired(MainState::WouldConflict);
    }
    // HEAD probe short-circuit.
    match has_merge_tree_conflicts {
        Some(true) => return Tier::Fired(MainState::WouldConflict),
        Some(false) => {}
        None => return Tier::Wait,
    }
    // HEAD probe says "no conflict" — still need the working-tree probe
    // to complete (unless it's already reported a non-dirty result).
    match has_working_tree_conflicts {
        Some(_) => Tier::RuledOut,
        None => Tier::Wait,
    }
}

/// Tiers 4–6: integration / same-commit-dirty / counts-based fallback.
///
/// Once tiers 1–3 have been ruled out, the gate needs `counts` and
/// `is_clean`, plus whatever integration signals the caller can provide.
/// This delegates to [`MainState::from_integration_and_counts`] with
/// `is_main = false`, `would_conflict = false`, `is_orphan = false`
/// (callers enforce those invariants via the earlier tiers).
///
/// Returns:
/// - `Fired(state)` if `counts` and `is_clean` are both known. (`state`
///   may be `MainState::None` when there's nothing to display, which
///   callers should still treat as a resolved gate.)
/// - `Wait` if either `counts` or `is_clean` is still loading.
pub fn tier_integration_or_counts(
    counts: Option<super::stats::AheadBehind>,
    is_clean: Option<bool>,
    integration: Option<MainState>,
) -> Tier<MainState> {
    let Some(counts) = counts else {
        return Tier::Wait;
    };
    let Some(is_clean) = is_clean else {
        return Tier::Wait;
    };
    // is_same_commit_dirty requires counts and !is_clean.
    let is_same_commit_dirty = !is_clean && counts.ahead == 0 && counts.behind == 0;
    Tier::Fired(MainState::from_integration_and_counts(
        false, // is_main — tier 1 ruled this out
        false, // would_conflict — tier 3 ruled this out
        integration,
        is_same_commit_dirty,
        false, // is_orphan — tier 2 ruled this out
        counts.ahead,
        counts.behind,
    ))
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
        use insta::assert_snapshot;
        assert!(Divergence::None.styled().is_none());
        assert_snapshot!(Divergence::InSync.styled().unwrap(), @"[2m|[22m");
        assert_snapshot!(Divergence::Ahead.styled().unwrap(), @"[2m⇡[22m");
        assert_snapshot!(Divergence::Behind.styled().unwrap(), @"[2m⇣[22m");
        assert_snapshot!(Divergence::Diverged.styled().unwrap(), @"[2m⇅[22m");
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
        use insta::assert_snapshot;
        assert!(MainState::None.styled().is_none());
        assert_snapshot!(MainState::WouldConflict.styled().unwrap(), @"[33m✗[39m");
        assert_snapshot!(MainState::IsMain.styled().unwrap(), @"[2m^[22m");
        assert_snapshot!(MainState::Ahead.styled().unwrap(), @"[2m↑[22m");
        assert_snapshot!(MainState::Orphan.styled().unwrap(), @"[2m∅[22m");
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
        let s: &'static str = IntegrationReason::PatchIdMatch.into();
        assert_eq!(s, "patch-id-match");
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
        assert_eq!(
            MainState::Integrated(IntegrationReason::PatchIdMatch).integration_reason(),
            Some(IntegrationReason::PatchIdMatch)
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
    // MainState Tier Tests (per-gate resolution with partial data)
    // ============================================================================

    #[test]
    fn test_tier_is_main() {
        assert_eq!(tier_is_main(true), Tier::Fired(MainState::IsMain));
        assert_eq!(tier_is_main(false), Tier::RuledOut);
    }

    #[test]
    fn test_tier_orphan() {
        assert_eq!(tier_orphan(Some(true)), Tier::Fired(MainState::Orphan));
        assert_eq!(tier_orphan(Some(false)), Tier::RuledOut);
        assert_eq!(tier_orphan(None), Tier::Wait);
    }

    #[test]
    fn test_tier_would_conflict() {
        // HEAD probe says conflict → fire, even if working-tree probe
        // hasn't reported.
        assert_eq!(
            tier_would_conflict(Some(true), None),
            Tier::Fired(MainState::WouldConflict)
        );
        // Working-tree probe says dirty conflict → fire, even if HEAD
        // probe hasn't reported.
        assert_eq!(
            tier_would_conflict(None, Some(Some(true))),
            Tier::Fired(MainState::WouldConflict)
        );
        // Both probes report "no conflict" (HEAD: Some(false), WT:
        // Some(None) meaning "working tree is clean, no dirty-tree
        // result") → rule out.
        assert_eq!(tier_would_conflict(Some(false), Some(None)), Tier::RuledOut);
        // HEAD probe says "no conflict" and WT probe reports a clean
        // dirty-tree result (Some(Some(false))) → rule out.
        assert_eq!(
            tier_would_conflict(Some(false), Some(Some(false))),
            Tier::RuledOut
        );
        // HEAD probe hasn't reported → wait (could flip us to conflict).
        assert_eq!(tier_would_conflict(None, None), Tier::Wait);
        assert_eq!(tier_would_conflict(None, Some(None)), Tier::Wait);
        // HEAD probe says "no conflict" but WT probe hasn't reported →
        // wait (WT could still flip us).
        assert_eq!(tier_would_conflict(Some(false), None), Tier::Wait);
    }

    #[test]
    fn test_tier_integration_or_counts() {
        use super::super::stats::AheadBehind;

        // counts missing → wait
        assert_eq!(
            tier_integration_or_counts(None, Some(true), None),
            Tier::Wait
        );

        // is_clean missing → wait
        assert_eq!(
            tier_integration_or_counts(
                Some(AheadBehind {
                    ahead: 0,
                    behind: 0
                }),
                None,
                None
            ),
            Tier::Wait
        );

        // in-sync clean → Fired(None)
        assert_eq!(
            tier_integration_or_counts(
                Some(AheadBehind {
                    ahead: 0,
                    behind: 0
                }),
                Some(true),
                None
            ),
            Tier::Fired(MainState::None)
        );

        // in-sync dirty → SameCommit
        assert_eq!(
            tier_integration_or_counts(
                Some(AheadBehind {
                    ahead: 0,
                    behind: 0
                }),
                Some(false),
                None
            ),
            Tier::Fired(MainState::SameCommit)
        );

        // Ahead only
        assert_eq!(
            tier_integration_or_counts(
                Some(AheadBehind {
                    ahead: 3,
                    behind: 0
                }),
                Some(true),
                None
            ),
            Tier::Fired(MainState::Ahead)
        );

        // Diverged
        assert_eq!(
            tier_integration_or_counts(
                Some(AheadBehind {
                    ahead: 3,
                    behind: 2
                }),
                Some(true),
                None
            ),
            Tier::Fired(MainState::Diverged)
        );

        // Integration state wins over counts
        assert_eq!(
            tier_integration_or_counts(
                Some(AheadBehind {
                    ahead: 5,
                    behind: 0
                }),
                Some(true),
                Some(MainState::Integrated(IntegrationReason::Ancestor)),
            ),
            Tier::Fired(MainState::Integrated(IntegrationReason::Ancestor))
        );
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
        use insta::assert_snapshot;
        assert!(OperationState::None.styled().is_none());
        assert_snapshot!(OperationState::Conflicts.styled().unwrap(), @"[31m✘[39m");
        assert_snapshot!(OperationState::Rebase.styled().unwrap(), @"[33m⤴[39m");
        assert_snapshot!(OperationState::Merge.styled().unwrap(), @"[33m⤵[39m");
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
    fn test_git_operation_state_is_none() {
        assert!(ActiveGitOperation::None.is_none());
        assert!(ActiveGitOperation::default().is_none());
        assert!(!ActiveGitOperation::Rebase.is_none());
        assert!(!ActiveGitOperation::Merge.is_none());
    }
}
