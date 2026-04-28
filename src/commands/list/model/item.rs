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

/// Serde helper: skip `git_operation` when it is absent (unloaded) or an
/// active `ActiveGitOperation::None` — preserving the original
/// `skip_serializing_if = "ActiveGitOperation::is_none"` behavior now that
/// the field is wrapped in `Option`.
// `clippy::ref_option` fires because we take `&Option<T>` instead of
// `Option<&T>`, but `skip_serializing_if` calls this with `&field`, so the
// signature is forced by serde.
#[allow(clippy::ref_option)]
fn git_operation_is_none_or_unloaded(op: &Option<ActiveGitOperation>) -> bool {
    match op {
        None => true,
        Some(inner) => inner.is_none(),
    }
}

/// Compute the `WorktreeState` from `WorktreeData` metadata alone.
///
/// Used by both `compute_status_symbols` (full computation) and the
/// metadata-only fallback path. The decision priority is:
/// `branch_worktree_mismatch` > `prunable` > `locked` > `None`.
fn metadata_worktree_state(data: &WorktreeData) -> WorktreeState {
    if data.branch_worktree_mismatch {
        WorktreeState::BranchWorktreeMismatch
    } else if data.is_prunable() {
        WorktreeState::Prunable
    } else if data.locked.is_some() {
        WorktreeState::Locked
    } else {
        WorktreeState::None
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
    /// Working-tree change flags (tracked/untracked/modified). `None` = not yet
    /// loaded; `Some` = loaded (possibly empty). Fed by the `WorkingTreeDiff` task.
    #[serde(skip)]
    pub working_tree_status: Option<WorkingTreeStatus>,
    /// Whether the working tree has merge conflicts in tracked files. `None` =
    /// not yet loaded; `Some` = loaded. Fed by the `WorkingTreeDiff` task.
    #[serde(skip)]
    pub has_conflicts: Option<bool>,
    /// Result of `WorkingTreeConflicts` task (`--full` mode only). Outer `None`
    /// = task hasn't run yet. Outer `Some(None)` = task ran but working tree
    /// was clean, so fall back to the committed-HEAD merge-tree check.
    /// Outer `Some(Some(b))` = dirty working tree, `b` is the conflict result.
    #[serde(skip)]
    pub has_working_tree_conflicts: Option<Option<bool>>,
    /// Git operation in progress (rebase/merge). `None` = not yet loaded;
    /// `Some(ActiveGitOperation::None)` = loaded, no operation in progress.
    /// Fed by the `GitOperation` task.
    #[serde(skip_serializing_if = "git_operation_is_none_or_unloaded")]
    pub git_operation: Option<ActiveGitOperation>,
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
#[derive(serde::Serialize, Clone)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ItemKind {
    Worktree(Box<WorktreeData>),
    Branch,
}

/// Unified item for displaying worktrees and branches in the same table.
///
/// Column-rendered fields are `Option<U>` where the outer `Option` encodes whether data
/// was collected (`None` = not loaded, render shows placeholder). The inner type `U` is
/// whatever the data naturally is — e.g., `AheadBehind` (always has a value, even if zero)
/// or `Option<PrStatus>` (CI may not exist).
#[derive(serde::Serialize, Clone)]
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
    /// Whether the branch's squashed patch-id matches a commit on the integration target.
    /// Detects squash merges when merge-tree conflicts (both sides modified the same files).
    #[serde(skip)]
    pub is_patch_id_match: Option<bool>,
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

    /// CI/PR status (inner Option: whether CI exists for this branch)
    pub pr_status: Option<Option<PrStatus>>,

    /// Dev server URL computed from project config template
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Whether the URL's port is actively listening
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url_active: Option<bool>,

    /// LLM-generated branch summary (inner Option: whether LLM produced a summary)
    #[serde(skip)]
    pub summary: Option<Option<String>>,

    /// Potential merge conflicts with the integration target, computed from
    /// the committed HEAD via `git merge-tree`. `None` = not yet loaded;
    /// `Some` = loaded. Fed by the `MergeTreeConflicts` task.
    #[serde(skip)]
    pub has_merge_tree_conflicts: Option<bool>,
    /// User-defined status marker from git config. Outer `None` = task
    /// hasn't run yet; `Some(None)` = task ran, no marker configured;
    /// `Some(Some(s))` = task ran, marker is `s`. Fed by the `UserMarker` task.
    #[serde(skip)]
    pub user_marker: Option<Option<String>>,

    /// Git status symbols — one `StatusSymbols` struct per item, always
    /// present after construction. Each *field inside* `StatusSymbols` is an
    /// `Option` that progresses from `None` (loading, renders `·`) to `Some`
    /// as task results arrive. See the `status_symbols` module docstring for
    /// the per-gate rendering rules.
    ///
    /// Not serialized directly — JSON output converts to `JsonItem` first.
    #[serde(skip)]
    pub status_symbols: StatusSymbols,

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
            is_patch_id_match: None,
            is_ancestor: None,
            is_orphan: None,
            upstream: None,
            pr_status: None,
            url: None,
            url_active: None,
            summary: None,
            has_merge_tree_conflicts: None,
            user_marker: None,
            status_symbols: StatusSymbols::default(),
            display: DisplayFields::default(),
            kind: ItemKind::Branch,
        }
    }

    pub fn branch_name(&self) -> &str {
        self.branch.as_deref().unwrap_or("(detached)")
    }

    /// Short display name for this item — the branch if present, otherwise
    /// a truncated HEAD SHA. Use when reporting which item is pending,
    /// stuck, or missing: `branch_name()`'s `"(detached)"` fallback collapses
    /// distinct detached items into one label.
    pub fn display_name(&self) -> &str {
        self.branch
            .as_deref()
            .unwrap_or_else(|| &self.head[..8.min(self.head.len())])
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
        // Gate 3 (`main_state`) is `None` until its inputs land. Until
        // then, we don't know whether the item is removable.
        let main_state = self.status_symbols.main_state?;
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
        let status = self.status_symbols.format_compact();
        if !status.is_empty() {
            segments.push(StatuslineSegment::from_column(status, ColumnKind::Status));
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

    /// Refresh status symbols for this item, populating any gates whose
    /// inputs have newly become available.
    ///
    /// Idempotent: safe to call repeatedly as task results arrive. Each
    /// gate is resolved independently; a gate once resolved is never
    /// unresolved. Gates whose inputs aren't ready yet are left at
    /// `None`, and the renderer emits the position-level `·` placeholder
    /// for them (step 5).
    ///
    /// See the `status_symbols` module docstring for the full per-gate
    /// spec including priority short-circuit rules.
    pub(crate) fn refresh_status_symbols(&mut self, default_branch: Option<&str>) {
        // Gate 2 (metadata family — position 3). Metadata-only, so it
        // resolves synchronously on the first refresh call. After this
        // line, `status_symbols.worktree_state` is always `Some`.
        // (Prunable worktrees are pre-seeded at spawn time and have
        // `worktree_state = Some(Prunable)` by the time this runs.)
        let metadata_state = match &self.kind {
            ItemKind::Worktree(data) => metadata_worktree_state(data),
            ItemKind::Branch => WorktreeState::Branch,
        };
        if self.status_symbols.worktree_state.is_none() {
            self.status_symbols.worktree_state = Some(metadata_state);
        }

        // Gate 1 (working tree flags — positions 0-2).
        if self.status_symbols.working_tree.is_none()
            && let Some(wt) = self.try_gate_working_tree()
        {
            self.status_symbols.working_tree = Some(wt);
        }

        // Gate 2 (operation family — position 3).
        if self.status_symbols.operation_state.is_none()
            && let Some(op) = self.try_gate_operation_state()
        {
            self.status_symbols.operation_state = Some(op);
        }

        // Gate 3 (main state — position 4).
        // Gate 3 is re-evaluable: unlike other gates, its answer can
        // become more specific as later signals arrive. Integration
        // signals are not hard-gated, so the first pass may see only
        // counts and produce `Ahead`; a later pass with `has_file_changes`
        // loaded can refine to `Integrated(NoAddedChanges)`. The
        // progression is strictly refinement (never wrong, just less
        // specific), so re-evaluation is safe.
        if let Some(ms) = self.try_gate_main_state(default_branch) {
            self.status_symbols.main_state = Some(ms);
        }

        // Gate 4 (upstream divergence — position 5).
        if self.status_symbols.upstream_divergence.is_none()
            && let Some(d) = self.try_gate_upstream_divergence()
        {
            self.status_symbols.upstream_divergence = Some(d);
        }

        // Gate 5 (user marker — position 6).
        if self.status_symbols.user_marker.is_none()
            && let Some(m) = self.try_gate_user_marker()
        {
            self.status_symbols.user_marker = Some(m);
        }
    }

    /// Gate 1: working tree flags. Resolves as soon as `working_tree_status`
    /// is loaded (for worktrees) or immediately (for branches, which have
    /// no working tree).
    fn try_gate_working_tree(&self) -> Option<WorkingTreeStatus> {
        match &self.kind {
            ItemKind::Worktree(data) => data.working_tree_status,
            // Branches have no working tree; treat as permanently clean.
            ItemKind::Branch => Some(WorkingTreeStatus::default()),
        }
    }

    /// Gate 2: operation state. Resolves once both `has_conflicts` and
    /// `git_operation` have reported. Priority within the gate:
    /// `has_conflicts` > rebase > merge > none.
    fn try_gate_operation_state(&self) -> Option<OperationState> {
        match &self.kind {
            ItemKind::Worktree(data) => {
                let has_conflicts = data.has_conflicts?;
                if has_conflicts {
                    return Some(OperationState::Conflicts);
                }
                let git_operation = data.git_operation.as_ref()?;
                match git_operation {
                    ActiveGitOperation::Rebase => Some(OperationState::Rebase),
                    ActiveGitOperation::Merge => Some(OperationState::Merge),
                    ActiveGitOperation::None => Some(OperationState::None),
                }
            }
            // Branches have no operation state; trivially resolved to None.
            ItemKind::Branch => Some(OperationState::None),
        }
    }

    /// Gate 3: main state. Walks the priority chain tier by tier, using
    /// the per-tier helpers from `state.rs`.
    fn try_gate_main_state(&self, default_branch: Option<&str>) -> Option<MainState> {
        use super::state::{
            Tier, tier_integration_or_counts, tier_is_main, tier_orphan, tier_would_conflict,
        };

        let is_main = matches!(&self.kind, ItemKind::Worktree(data) if data.is_main);

        // Tier 1: IsMain (immediate if `is_main`, otherwise rule out).
        match tier_is_main(is_main) {
            Tier::Fired(s) => return Some(s),
            Tier::RuledOut => {}
            Tier::Wait => return None, // unreachable: tier 1 never waits
        }

        // Tier 2: Orphan.
        match tier_orphan(self.is_orphan) {
            Tier::Fired(s) => return Some(s),
            Tier::RuledOut => {}
            Tier::Wait => return None,
        }

        // Tier 3: WouldConflict. For branches, there's no working-tree
        // conflict probe, so we substitute `Some(None)` (the "task ran
        // but working tree is clean / N/A" sentinel).
        let has_working_tree_conflicts = match &self.kind {
            ItemKind::Worktree(data) => data.has_working_tree_conflicts,
            ItemKind::Branch => Some(None),
        };
        match tier_would_conflict(self.has_merge_tree_conflicts, has_working_tree_conflicts) {
            Tier::Fired(s) => return Some(s),
            Tier::RuledOut => {}
            Tier::Wait => return None,
        }

        // Tiers 4-6: integration / same-commit-dirty / counts-based. Needs
        // `counts` and `is_clean`, plus the integration signals fed
        // through `check_integration_state` (which is short-circuiting and
        // treats missing integration signals as "no info, fall through").
        let is_clean = match &self.kind {
            ItemKind::Worktree(data) => {
                let diff = data.working_tree_diff.as_ref()?;
                let status = data.working_tree_status?;
                Some(diff.is_empty() && !status.untracked)
            }
            // Branches have no working tree; trivially clean.
            ItemKind::Branch => Some(true),
        };
        let integration = match &self.kind {
            ItemKind::Worktree(data) => {
                self.check_integration_state(data.is_main, default_branch, is_clean?)
            }
            ItemKind::Branch => self.check_integration_state(false, default_branch, true),
        };

        match tier_integration_or_counts(self.counts, is_clean, integration) {
            Tier::Fired(s) => Some(s),
            Tier::RuledOut | Tier::Wait => None,
        }
    }

    /// Gate 4: upstream divergence. Resolves once `upstream` is loaded.
    fn try_gate_upstream_divergence(&self) -> Option<Divergence> {
        let upstream = self.upstream.as_ref()?;
        Some(match upstream.active() {
            Some(active) => Divergence::from_counts_with_remote(active.ahead, active.behind),
            None => Divergence::None,
        })
    }

    /// Gate 5: user marker. Resolves once the `UserMarker` task reports,
    /// carrying either `Some(marker)` or `None` (no marker configured).
    fn try_gate_user_marker(&self) -> Option<Option<String>> {
        self.user_marker.clone()
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
            is_patch_id_match: self.is_patch_id_match,
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

    // ============================================================================
    // Per-gate refresh_status_symbols tests
    //
    // Each test exercises one gate's "waiting for inputs" / "short-circuit
    // resolved" behavior. These replace the old
    // `test_compute_status_symbols_waits_for_every_required_field` in
    // results.rs, which fused all five gates into one "this field gates
    // the whole function" assertion list.
    // ============================================================================

    /// Build a worktree `ListItem` pointing at a non-null HEAD, with
    /// `is_main=false` so tests can exercise the full gate chain without
    /// hitting the main-worktree short-circuit.
    fn make_worktree_item() -> ListItem {
        use crate::commands::list::collect::build_worktree_item;
        use worktrunk::git::WorktreeInfo;
        let wt = WorktreeInfo {
            path: std::path::PathBuf::from("/tmp/wt"),
            head: "abc123".into(),
            branch: Some("feat".into()),
            bare: false,
            detached: false,
            locked: None,
            prunable: None,
        };
        build_worktree_item(&wt, false, false, false)
    }

    // ---- Gate 1: working tree flags (positions 0-2) ----

    #[test]
    fn gate_working_tree_loading_vs_resolved() {
        use super::super::super::model::WorkingTreeStatus;

        // Branch items are treated as permanently clean — gate 1 always
        // resolves to `Some(default)` on first refresh.
        let mut item = ListItem::new_branch("abc".into(), "feat".into());
        item.refresh_status_symbols(None);
        assert_eq!(
            item.status_symbols.working_tree,
            Some(WorkingTreeStatus::default())
        );

        // Worktree items with no `working_tree_status` → gate 1 stays
        // None (Loading).
        let mut item = make_worktree_item();
        item.refresh_status_symbols(None);
        assert_eq!(item.status_symbols.working_tree, None);

        // Set working_tree_status → gate 1 resolves on next refresh.
        if let ItemKind::Worktree(ref mut data) = item.kind {
            data.working_tree_status =
                Some(WorkingTreeStatus::new(true, false, false, false, false));
        }
        item.refresh_status_symbols(None);
        assert!(item.status_symbols.working_tree.unwrap().staged);
    }

    // ---- Gate 2: operation state (position 3) ----

    #[test]
    fn gate_operation_state_short_circuits_on_conflicts() {
        // `has_conflicts = Some(true)` fires the gate immediately without
        // waiting for `git_operation`.
        let mut item = make_worktree_item();
        if let ItemKind::Worktree(ref mut data) = item.kind {
            data.has_conflicts = Some(true);
            // git_operation deliberately left None
        }
        item.refresh_status_symbols(None);
        assert_eq!(
            item.status_symbols.operation_state,
            Some(OperationState::Conflicts)
        );
    }

    #[test]
    fn gate_operation_state_waits_for_both_inputs() {
        // `has_conflicts = Some(false)` but `git_operation = None` →
        // gate stays Loading (could still become Rebase/Merge).
        let mut item = make_worktree_item();
        if let ItemKind::Worktree(ref mut data) = item.kind {
            data.has_conflicts = Some(false);
            data.git_operation = None;
        }
        item.refresh_status_symbols(None);
        assert_eq!(item.status_symbols.operation_state, None);

        // Set git_operation → gate resolves.
        if let ItemKind::Worktree(ref mut data) = item.kind {
            data.git_operation = Some(ActiveGitOperation::Rebase);
        }
        item.refresh_status_symbols(None);
        assert_eq!(
            item.status_symbols.operation_state,
            Some(OperationState::Rebase)
        );
    }

    // ---- Gate 3: main state (position 4) ----

    #[test]
    fn gate_main_state_is_main_short_circuit() {
        // Construct a main-worktree ListItem directly.
        use crate::commands::list::collect::build_worktree_item;
        use worktrunk::git::WorktreeInfo;
        let wt = WorktreeInfo {
            path: std::path::PathBuf::from("/tmp/main"),
            head: "abc123".into(),
            branch: Some("main".into()),
            bare: false,
            detached: false,
            locked: None,
            prunable: None,
        };
        let mut item = build_worktree_item(&wt, true, false, false);
        // No other inputs set — tier 1 fires on metadata alone.
        item.refresh_status_symbols(Some("main"));
        assert_eq!(item.status_symbols.main_state, Some(MainState::IsMain));
    }

    #[test]
    fn gate_main_state_orphan_blocks_lower_tiers() {
        let mut item = make_worktree_item();
        item.is_orphan = Some(true);
        // Other inputs deliberately left None — tier 2 fires without them.
        item.refresh_status_symbols(None);
        assert_eq!(item.status_symbols.main_state, Some(MainState::Orphan));
    }

    #[test]
    fn gate_main_state_would_conflict_requires_both_conflict_signals() {
        // `has_merge_tree_conflicts = None` → tier 3 waits even with
        // `has_working_tree_conflicts` saying "clean."
        let mut item = make_worktree_item();
        item.is_orphan = Some(false);
        item.has_merge_tree_conflicts = None;
        if let ItemKind::Worktree(ref mut data) = item.kind {
            data.has_working_tree_conflicts = Some(None); // clean working tree
        }
        item.refresh_status_symbols(None);
        assert_eq!(item.status_symbols.main_state, None);

        // Set the merge-tree probe to "no conflict" → tier 3 rules out,
        // fall through to lower tiers. But counts is still None, so
        // tier 4 waits and gate stays None.
        item.has_merge_tree_conflicts = Some(false);
        item.refresh_status_symbols(None);
        assert_eq!(item.status_symbols.main_state, None);
    }

    #[test]
    fn gate_main_state_tier4_waits_for_counts_and_clean() {
        use super::super::super::model::{AheadBehind, WorkingTreeStatus};
        use worktrunk::git::LineDiff;

        let mut item = make_worktree_item();
        item.is_orphan = Some(false);
        item.has_merge_tree_conflicts = Some(false);
        if let ItemKind::Worktree(ref mut data) = item.kind {
            data.has_working_tree_conflicts = Some(None);
        }
        // counts set but is_clean inputs missing → Wait.
        item.counts = Some(AheadBehind {
            ahead: 3,
            behind: 2,
        });
        item.refresh_status_symbols(None);
        assert_eq!(item.status_symbols.main_state, None);

        // Fill in the is_clean inputs → gate resolves.
        if let ItemKind::Worktree(ref mut data) = item.kind {
            data.working_tree_diff = Some(LineDiff::default());
            data.working_tree_status = Some(WorkingTreeStatus::default());
        }
        item.refresh_status_symbols(None);
        assert_eq!(item.status_symbols.main_state, Some(MainState::Diverged));
    }

    // ---- Gate 4: upstream divergence (position 5) ----

    #[test]
    fn gate_upstream_divergence() {
        use super::super::super::model::UpstreamStatus;

        let mut item = make_worktree_item();
        // upstream None → Loading.
        item.refresh_status_symbols(None);
        assert_eq!(item.status_symbols.upstream_divergence, None);

        // Default UpstreamStatus has remote=None, so active() returns
        // None → resolves to Divergence::None.
        item.upstream = Some(UpstreamStatus::default());
        item.refresh_status_symbols(None);
        assert_eq!(
            item.status_symbols.upstream_divergence,
            Some(Divergence::None)
        );
    }

    // ---- Gate 5: user marker (position 6) ----

    #[test]
    fn gate_user_marker() {
        // Loading until user_marker is Some(_).
        let mut item = make_worktree_item();
        item.refresh_status_symbols(None);
        assert_eq!(item.status_symbols.user_marker, None);

        // Task ran, no marker configured → resolves to Some(None).
        item.user_marker = Some(None);
        item.refresh_status_symbols(None);
        assert_eq!(item.status_symbols.user_marker, Some(None));

        // Task ran with a marker value → resolves to Some(Some(s)).
        let mut item = make_worktree_item();
        item.user_marker = Some(Some("🔥".to_string()));
        item.refresh_status_symbols(None);
        assert_eq!(
            item.status_symbols.user_marker,
            Some(Some("🔥".to_string()))
        );
    }
}
