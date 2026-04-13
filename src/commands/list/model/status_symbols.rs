//! Status symbol types and per-symbol atomic rendering rules.
//!
//! # Goal
//!
//! Each symbol in the Status column is computed and rendered independently.
//! A symbol only appears once we have the data needed to compute **that
//! specific symbol**. Symbols whose inputs have not arrived render as a
//! position-level loading / timeout placeholder, and stay that way if the
//! drain deadline fires before the data lands.
//!
//! The core rule is: **each position is atomic in its own inputs**. The cell
//! as a whole is never atomic — parts that are ready render; parts that are
//! not render `·`.
//!
//! # Positions
//!
//! The Status column has seven rendered positions (see [`PositionMask`]).
//! They render left-to-right and are independently gated:
//!
//! | # | Name                | Symbols              | Answers                               |
//! |---|---------------------|----------------------|---------------------------------------|
//! | 0 | `STAGED`            | `+`                  | Are there staged changes?             |
//! | 1 | `MODIFIED`          | `!`                  | Are there unstaged modifications?     |
//! | 2 | `UNTRACKED`         | `?`                  | Are there untracked files?            |
//! | 3 | `WORKTREE_STATE`    | `✘ ⤴ ⤵ ⚑ ⊟ ⊞ /`      | Operation / worktree attribute        |
//! | 4 | `MAIN_STATE`        | `^ ✗ _ – ⊂ ↕ ↑ ↓`    | Relationship to the default branch    |
//! | 5 | `UPSTREAM_DIVERGENCE` | \| ⇅ ⇡ ⇣           | Relationship to the tracked remote    |
//! | 6 | `USER_MARKER`       | emoji / text         | User-defined annotation               |
//!
//! Positions 0–2 are three visual slots but **one logical decision** — they
//! all come from `data.working_tree_status` and resolve together. Each of
//! positions 3, 4, 5, 6 is an independent logical decision with its own
//! input set. There are effectively **five independent gates** on the Status
//! cell.
//!
//! # Gate 1: Working tree flags (positions 0–2)
//!
//! **Renders:** any combination of `+`, `!`, `?` (plus `»` / `✘` for renamed
//! / deleted when used).
//!
//! **Inputs:** `data.working_tree_status` — produced by the `WorkingTreeDiff`
//! task, or seeded at spawn time for items whose tasks will not run.
//!
//! **Rule:**
//! - `Some(_)` → render the flags it contains. A clean tree leaves all three
//!   positions blank.
//! - `None` → all three positions render the position-level `·` placeholder.
//!
//! Branches and prunable worktrees seed `working_tree_status` to a "no
//! working tree" sentinel at spawn time, so their positions render blank,
//! not `·`.
//!
//! # Gate 2: Worktree state (position 3)
//!
//! **Renders:** at most one of `✘ ⤴ ⤵ ⚑ ⊟ ⊞ /`, priority
//! `✘ > ⤴ > ⤵ > ⚑ > ⊟ > ⊞ > /`. The operation family (`✘⤴⤵`) comes from live
//! task data; the attribute family (`⚑⊟⊞/`) is metadata, always known.
//!
//! **Inputs:** `data.has_conflicts`, `data.git_operation`, plus metadata
//! (`locked`, `prunable`, `branch_worktree_mismatch`, `ItemKind::Branch`).
//!
//! **Rule — short-circuit on priority:** a higher-priority signal, once known
//! to be positive, resolves the gate immediately without waiting for
//! lower-priority signals. Formally, render as soon as we can identify which
//! row of the priority table is the answer:
//!
//! 1. `has_conflicts == Some(true)` → `✘`.
//! 2. `has_conflicts == Some(false)` and `git_operation == Some(Rebase)` → `⤴`.
//! 3. `has_conflicts == Some(false)` and `git_operation == Some(Merge)` → `⤵`.
//! 4. `has_conflicts == Some(false)` and `git_operation == Some(None)` and
//!    metadata says mismatched → `⚑`.
//! 5. …continuing down through `⊟`, `⊞`, `/`, nothing.
//!
//! Until both `has_conflicts` and `git_operation` are known, we cannot rule
//! out `✘/⤴/⤵`, so the position renders `·` even if metadata would otherwise
//! produce `⊟` or `⊞`.
//!
//! **Exception — items with no working tree:** branches and prunable
//! worktrees have seeded sentinels (`has_conflicts = Some(false)`,
//! `git_operation = Some(None)`) at spawn time, so the operation family is
//! ruled out immediately and metadata wins.
//!
//! # Gate 3: Main state (position 4)
//!
//! **Renders:** at most one of `^ ✗ _ – ⊂ ↕ ↑ ↓`. Priority:
//! `IsMain > Orphan > WouldConflict > Empty(_) > SameCommitDirty(–) >
//! Integrated(⊂) > Diverged(↕) > Ahead(↑) > Behind(↓) > None`.
//!
//! **Inputs:**
//! - `is_main` (metadata)
//! - `is_orphan` (from `AheadBehind`)
//! - `has_merge_tree_conflicts` (from `MergeTreeConflicts`)
//! - `has_working_tree_conflicts` (from `WorkingTreeConflicts`)
//! - Integration signals: `is_ancestor`, `committed_trees_match`,
//!   `has_file_changes`, `would_merge_add`, `is_patch_id_match`
//! - `counts` (from `AheadBehind`)
//! - `working_tree_diff` + `working_tree_status` (used for `is_clean`, which
//!   distinguishes `Empty(_)` from `SameCommitDirty(–)`)
//!
//! **Rule — short-circuit on priority (same pattern as gate 2):** render as
//! soon as the priority-winning signal can be identified.
//!
//! 1. `is_main == true` (metadata) → `^`. Always immediate for the main
//!    worktree, regardless of any other field.
//! 2. `is_orphan == Some(true)` → orphan display.
//! 3. `has_merge_tree_conflicts == Some(true)` or
//!    `has_working_tree_conflicts == Some(Some(true))` → `✗`.
//! 4. Distinguish Empty / SameCommitDirty / Integrated — requires `counts`,
//!    `is_clean` (from `working_tree_diff` + `working_tree_status` for
//!    worktrees; trivially clean for branches), and the integration signals.
//!    Render as soon as all signals needed to pick a row have landed.
//! 5. Otherwise fall through to `↕/↑/↓/None` from `counts`.
//!
//! **Exception — unborn items (`HEAD == NULL_OID`):** all commit-dependent
//! tasks are skipped at spawn time. Their fields are seeded with "no commits"
//! sentinels (`counts = (0, 0)`, `is_orphan = false`, integration signals
//! conservatively false, `has_merge_tree_conflicts = false`). With those
//! seeded, gate 3 resolves to `None` for unborn non-main items and the row
//! still renders.
//!
//! **Exception — stale branches and `--skip-tasks`:** fields for
//! deliberately-skipped tasks are seeded at spawn time with conservative
//! defaults. Stale branches can render a less-specific main-state symbol
//! than fresh ones (e.g., `↕` instead of `⊂`).
//!
//! TODO: review whether the conservative-seed strategy for stale branches is
//! the right trade-off. An alternative is to leave skipped-task fields as
//! `None` so stale-branch rows show `·` in the affected positions — more
//! honest, but noisier in the picker.
//!
//! # Gate 4: Upstream divergence (position 5)
//!
//! **Renders:** at most one of `| ⇅ ⇡ ⇣`.
//!
//! **Inputs:** `upstream` (from `Upstream`).
//!
//! **Rule:**
//! - `Some(UpstreamStatus::None)` → nothing (no upstream configured).
//! - `Some(active)` → render the divergence glyph from `active.ahead/behind`.
//! - `None` → `·`.
//!
//! **Exception — unborn:** `Upstream` is a `COMMIT_TASK` and is skipped for
//! unborn items. `upstream` is seeded to "no upstream" at spawn time so
//! gate 4 renders blank for unborn rows, not `·`.
//!
//! # Gate 5: User marker (position 6)
//!
//! **Renders:** whatever the user's marker lookup produced.
//!
//! **Inputs:** `user_marker` (from `UserMarker`).
//!
//! **Rule:**
//! - `Some(Some(s))` → render `s`.
//! - `Some(None)` → nothing (no marker for this item).
//! - `None` → `·`.
//!
//! # Rendering `·` at the position level
//!
//! `·` is a position-level placeholder emitted by the render function for
//! each gate that hasn't resolved yet. It takes the full allocated width of
//! its position so table alignment is preserved across rows.
//!
//! An in-progress cell might look like:
//!
//! ```text
//! +!  · ↕ | ·     ← staged + modified known; worktree state + user marker still loading
//! ··· · ^ | ·    ← main worktree known from metadata; other gates still loading
//!     ^ | 💬      ← everything resolved: main worktree, in sync, user marker
//! ```
//!
//! TODO: loading and post-deadline timeout both render as the same dim `·`
//! today. The original design distinguished them (`⋯` vs `·`) but `⋯` was
//! too loud for a tight Status column where most cells are in one state or
//! the other at any given render. Pick a subtle second glyph — e.g., a
//! single-dot braille or an en-dash — once the two can be evaluated
//! side-by-side in real tables. See `PLACEHOLDER` in `render.rs`.
//!
//! # Timeout behavior
//!
//! When the drain deadline fires (`wt switch` picker budget,
//! `[list].timeout-ms`):
//!
//! 1. Each position's last-known state is displayed: resolved positions show
//!    their symbol; unresolved positions show `·`.
//! 2. The diagnostic footer already lists which tasks did not finish per
//!    item — this continues unchanged and gives the user the mapping from
//!    "`·` in position X" back to "`TaskKind::Foo` timed out."
//! 3. JSON output omits fields that correspond to unresolved gates
//!    (`working_tree`, `main_state`, `operation_state`, `upstream_divergence`,
//!    etc.) so machine consumers can distinguish "loading / timeout" from
//!    "loaded with no symbol."
//!
//! # Error handling
//!
//! A task that errors counts as "result received, carrying no information."
//! The affected field remains `None` and its gate renders `·`. There is **no
//! conservative-defaults fallback** for errored status-feeder tasks — a
//! failed `WorkingTreeDiff` shows `·` in positions 0–2 and 3, not a
//! fabricated clean state. The error itself is still captured and shown in
//! the post-render diagnostic section.
//!
//! # Data model
//!
//! Each gate's output is an `Option` inside [`StatusSymbols`]. `None` means
//! loading / unresolved; `Some(default)` means resolved with nothing to
//! display; `Some(value)` means resolved with a symbol.
//!
//! ```text
//! pub struct StatusSymbols {
//!     working_tree: Option<WorkingTreeStatus>,          // positions 0–2 (one decision)
//!     worktree_state: Option<ResolvedWorktreeState>,    // position 3
//!     main_state: Option<MainState>,                    // position 4
//!     upstream_divergence: Option<Divergence>,          // position 5
//!     user_marker: Option<Option<String>>,              // position 6
//! }
//! ```
//!
//! `item.status_symbols` is always `Some(StatusSymbols::default())` after the
//! skeleton render — every gate starts as `None` → `·`. Per-gate resolution
//! populates individual fields as task results arrive.
//!
//! `refresh_status_symbols` (replacing the current "all or nothing"
//! `compute_status_symbols`) is called after every drain tick: it tries each
//! gate independently and sets any fields whose inputs are now ready. It is
//! idempotent — a gate, once resolved, is never un-resolved.
//!
//! TODO: replace `Option<T>` on status-feeding fields with a `TaskInput<T>`
//! enum that distinguishes `Pending` / `Skipped` / `Ready(T)`. Currently the
//! "task won't run" state is encoded by seeding a conservative `Some(value)`
//! via `seed_skipped_task_defaults`, which conflates "measured and got this
//! answer" with "fabricated a safe default." A typed enum would:
//!
//! - Eliminate the fabricated-value class of bugs (e.g., seeding a clean
//!   `WorkingTreeDiff` for a dirty worktree).
//! - Make JSON output trivially correct (`Ready(v)` emits, others don't).
//! - Let each gate explicitly decide what `Skipped` means for its tier
//!   logic, instead of relying on a convention about which sentinel value
//!   is "safe."
//! - Collapse the `Option<Option<T>>` hacks on `has_working_tree_conflicts`
//!   and `user_marker` into `TaskInput<Option<T>>`.
//! - Subsume the `SkipReason` concept — the field itself carries the state.
//!
//! Natural inflection point: when caching lands, cached values become a
//! fourth source (`Cached(T)`) that needs the same "real vs fabricated"
//! distinction. The enum refactor would pay for itself at that point.
//! Estimated scope: ~15 fields × all read sites, ~300–500 lines mechanical.
//!
//! # Non-goals
//!
//! - **Animated `·`.** Out of scope.
//! - **Partial rendering within a gate.** Gate 3 does not render `⊟` just
//!   because prunable metadata says so — it waits until the operation family
//!   can be ruled out. Atomicity *within* a gate is preserved; only *between*
//!   gates are decisions independent.
//! - **Retrying timed-out or errored tasks.** Separate concern.
//! - **Tuning picker budgets or task skip lists.** This spec only defines
//!   what happens when data does or does not arrive in time.

use super::state::{Divergence, MainState, OperationState, WorktreeState};

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
    pub(crate) const STAGED: usize = 0; // + (staged changes)
    pub(crate) const MODIFIED: usize = 1; // ! (modified files)
    pub(crate) const UNTRACKED: usize = 2; // ? (untracked files)
    pub(crate) const WORKTREE_STATE: usize = 3; // Worktree: ✘⤴⤵/⚑⊟⊞
    pub(crate) const MAIN_STATE: usize = 4; // Main relationship: ^✗_⊂↕↑↓
    pub(crate) const UPSTREAM_DIVERGENCE: usize = 5; // Remote: |⇅⇡⇣
    pub(crate) const USER_MARKER: usize = 6;

    /// Full mask with all positions enabled (for JSON output and progressive rendering)
    /// Allocates realistic widths based on common symbol sizes to ensure proper grid alignment
    pub const FULL: Self = Self {
        widths: [
            1, // STAGED: + (1 char)
            1, // MODIFIED: ! (1 char)
            1, // UNTRACKED: ? (1 char)
            1, // WORKTREE_STATE: ✘⤴⤵/⚑⊟⊞ (1 char, priority: conflicts > rebase > merge > branch_worktree_mismatch > prunable > locked > branch)
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

/// Working tree changes as structured booleans
///
/// This is the canonical internal representation. Display strings are derived from this.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
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

/// Structured status symbols for aligned rendering
///
/// Symbols are categorized to enable vertical alignment in table output.
/// Display order (left to right):
/// - Working tree: +, !, ? (staged, modified, untracked - NOT mutually exclusive)
/// - Worktree state: ✘, ⤴, ⤵, /, ⚑, ⊟, ⊞ (operations + location)
/// - Main state: ^, ✗, _, ⊂, ↕, ↑, ↓ (relationship to default branch - single-stroke vertical arrows)
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
/// - ⚑: Branch-worktree mismatch
/// - ⊟: Prunable (directory missing)
/// - ⊞: Locked worktree
/// - /: Branch without worktree
///
/// **Main state (single position with priority):**
/// Priority: ^ > ✗ > _ > – > ⊂ > ↕ > ↑ > ↓
/// - ^: This IS the main worktree
/// - ✗: Would conflict if merged to default branch
/// - _: Same commit as default branch, clean working tree (removable)
/// - –: Same commit as default branch, uncommitted changes (NOT removable)
/// - ⊂: Content integrated (removable)
/// - ↕: Diverged from default branch
/// - ↑: Ahead of default branch
/// - ↓: Behind default branch
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
    /// Gate 3 output (position 4). `None` = loading; `Some(MainState::None)`
    /// = resolved to nothing. Priority: IsMain (^) > Orphan > WouldConflict
    /// (✗) > Empty (_) > SameCommit (–) > Integrated (⊂) > Diverged (↕) >
    /// Ahead (↑) > Behind (↓).
    pub(crate) main_state: Option<MainState>,

    /// Gate 2 output — operation family (position 3). `None` = loading (we
    /// can't rule out `✘⤴⤵`); `Some(OperationState::None)` = resolved to
    /// nothing (fall through to `worktree_state`).
    pub(crate) operation_state: Option<OperationState>,

    /// Gate 2 output — metadata family (position 3). `None` = not yet
    /// inspected; `Some(WorktreeState::None)` = normal worktree, no
    /// location/attribute to display. Populated synchronously from
    /// `WorktreeData` metadata at refresh entry, so in practice this is
    /// `Some` whenever `refresh_status_symbols` has been called at least
    /// once.
    pub(crate) worktree_state: Option<WorktreeState>,

    /// Gate 4 output (position 5). `None` = loading; `Some(Divergence::None)`
    /// = resolved to nothing (in sync or no upstream).
    pub(crate) upstream_divergence: Option<Divergence>,

    /// Gate 1 output (positions 0-2). `None` = loading; `Some(default)` =
    /// resolved clean; `Some(dirty)` = one or more flags set.
    pub(crate) working_tree: Option<WorkingTreeStatus>,

    /// Gate 5 output (position 6). Outer `None` = loading; `Some(None)` =
    /// resolved, no marker configured; `Some(Some(s))` = marker `s`.
    pub(crate) user_marker: Option<Option<String>>,
}

impl StatusSymbols {
    /// Render symbols with selective alignment based on position mask.
    ///
    /// Each position renders according to its [`SlotState`]:
    /// - `Loading` → `placeholder` padded to the slot width (dimmed).
    ///   Both normal and stale rows pass `·` today — see `PLACEHOLDER`.
    /// - `Empty` → whitespace padded to the slot width.
    /// - `Visible(s)` → styled content padded to the slot width.
    ///
    /// CRITICAL: Always use [`PositionMask::FULL`] for consistent spacing
    /// between progressive and final rendering. The mask provides the
    /// maximum width needed for each position across all rows.
    pub fn render_with_mask(&self, mask: &PositionMask, placeholder: &str) -> String {
        use anstyle::Style;
        use worktrunk::styling::StyledLine;

        let mut result = String::with_capacity(64);

        for (pos, slot) in self.styled_symbols() {
            let allocated_width = mask.width(pos);

            match slot {
                SlotState::Visible(content) => {
                    let mut segment = StyledLine::new();
                    segment.push_raw(content);
                    segment.pad_to(allocated_width);
                    result.push_str(&segment.render());
                }
                SlotState::Loading => {
                    // Emit the placeholder glyph (dimmed) padded to the
                    // slot's allocated width. Unlike `Empty`, this
                    // represents "gate has not resolved" — the user
                    // should see a visible marker, not blank space.
                    let mut segment = StyledLine::new();
                    segment.push_styled(placeholder.to_string(), Style::new().dimmed());
                    segment.pad_to(allocated_width);
                    result.push_str(&segment.render());
                }
                SlotState::Empty => {
                    // Fill with spaces for alignment.
                    for _ in 0..allocated_width {
                        result.push(' ');
                    }
                }
            }
        }

        result
    }

    /// Render status symbols in compact form for statusline (no grid alignment).
    ///
    /// Uses the same styled symbols as `render_with_mask()`, just without padding.
    pub fn format_compact(&self) -> String {
        self.styled_symbols()
            .into_iter()
            .filter_map(|(_, slot)| match slot {
                SlotState::Visible(s) => Some(s),
                // Loading and Empty contribute nothing to the compact form —
                // the statusline has no column alignment, so unresolved gates
                // are just omitted.
                SlotState::Loading | SlotState::Empty => None,
            })
            .collect()
    }

    /// Build styled symbols array with position indices.
    ///
    /// Returns one [`SlotState`] per position. The renderer uses this to
    /// emit three kinds of cell content: `Loading` slots are rendered as the
    /// position-level placeholder (`·`), `Empty` slots as allocated
    /// whitespace, and `Visible` slots as styled symbols.
    ///
    /// Order: working_tree (0-2) → worktree state (3) → main state (4) →
    /// upstream divergence (5) → user marker (6).
    ///
    /// Styling follows semantic meaning:
    /// - Cyan: Working tree changes (activity indicator)
    /// - Red: Conflicts (blocking problems)
    /// - Yellow: Git operations, would_conflict, locked/prunable (states needing attention)
    /// - Dimmed: Main state symbols, divergence arrows, branch indicator (informational)
    pub(crate) fn styled_symbols(&self) -> [(usize, SlotState); 7] {
        use color_print::cformat;

        // Gate 1 — working tree flags (positions 0-2). Loading together.
        let (staged, modified, untracked) = match self.working_tree {
            Some(wt) => {
                let flag = |has: bool, sym: char| -> SlotState {
                    if has {
                        SlotState::Visible(cformat!("<cyan>{sym}</>"))
                    } else {
                        SlotState::Empty
                    }
                };
                (
                    flag(wt.staged, '+'),
                    flag(wt.modified, '!'),
                    flag(wt.untracked, '?'),
                )
            }
            None => (SlotState::Loading, SlotState::Loading, SlotState::Loading),
        };

        // Gate 3 — main state (position 4).
        let main_state_slot = match self.main_state {
            Some(ms) => match ms.styled() {
                Some(s) => SlotState::Visible(s),
                None => SlotState::Empty,
            },
            None => SlotState::Loading,
        };

        // Gate 4 — upstream divergence (position 5).
        let upstream_slot = match self.upstream_divergence {
            Some(d) => match d.styled() {
                Some(s) => SlotState::Visible(s),
                None => SlotState::Empty,
            },
            None => SlotState::Loading,
        };

        // Gate 2 — worktree state (position 3). Operation family (`✘⤴⤵`)
        // takes priority over metadata family (`⚑⊟⊞/`). The gate is
        // `Loading` iff `operation_state` is still `None` — even when
        // `worktree_state` metadata would yield `⊟`, we cannot safely show
        // it without ruling out a pending operation signal. Once
        // `operation_state == Some(None)`, fall through to `worktree_state`
        // metadata (which `refresh_status_symbols` fills synchronously, so
        // it's always `Some` by the time `operation_state` resolves).
        let worktree_slot = match self.operation_state {
            None => SlotState::Loading,
            Some(op) if op != OperationState::None => {
                SlotState::Visible(op.styled().unwrap_or_default())
            }
            Some(_) => match self.worktree_state {
                None | Some(WorktreeState::None) => SlotState::Empty,
                Some(WorktreeState::Branch) => {
                    SlotState::Visible(cformat!("<dim>{}</>", WorktreeState::Branch))
                }
                Some(WorktreeState::BranchWorktreeMismatch) => SlotState::Visible(cformat!(
                    "<red>{}</>",
                    WorktreeState::BranchWorktreeMismatch
                )),
                Some(other) => SlotState::Visible(cformat!("<yellow>{}</>", other)),
            },
        };

        // Gate 5 — user marker (position 6).
        let user_marker_slot = match &self.user_marker {
            None => SlotState::Loading,
            Some(None) => SlotState::Empty,
            Some(Some(s)) => SlotState::Visible(s.clone()),
        };

        // CRITICAL: Display order must match position indices for correct rendering.
        // Order: Working tree (0-2) → Worktree (3) → Main (4) → Remote (5) → User (6)
        [
            (PositionMask::STAGED, staged),
            (PositionMask::MODIFIED, modified),
            (PositionMask::UNTRACKED, untracked),
            (PositionMask::WORKTREE_STATE, worktree_slot),
            (PositionMask::MAIN_STATE, main_state_slot),
            (PositionMask::UPSTREAM_DIVERGENCE, upstream_slot),
            (PositionMask::USER_MARKER, user_marker_slot),
        ]
    }
}

/// State of a single Status-column slot when rendering.
///
/// Returned by [`StatusSymbols::styled_symbols`] so the renderer can decide,
/// per position, whether to emit a position-level placeholder (`Loading`),
/// blank whitespace (`Empty`), or the styled content (`Visible`).
#[derive(Debug, Clone)]
pub(crate) enum SlotState {
    /// Gate has not resolved — emit the placeholder glyph padded to the
    /// slot's allocated width.
    Loading,
    /// Gate resolved to "nothing to display" — emit whitespace padded to
    /// the slot's allocated width.
    Empty,
    /// Gate resolved with content — emit the styled string (padded by the
    /// caller to the slot's allocated width).
    Visible(String),
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;

    use super::*;

    /// True iff every gate is either unresolved (`None`) or resolved to a
    /// "nothing to display" variant. Sanity check for `Default` /
    /// `None`-variant semantics of gate outputs.
    fn is_empty(s: &StatusSymbols) -> bool {
        let main_empty = s.main_state.is_none_or(|s| s == MainState::None);
        let op_empty = s.operation_state.is_none_or(|s| s == OperationState::None);
        let wt_state_empty = s.worktree_state.is_none_or(|s| s == WorktreeState::None);
        let upstream_empty = s.upstream_divergence.is_none_or(|s| s == Divergence::None);
        let working_tree_empty = s.working_tree.is_none_or(|wt| !wt.is_dirty());
        let user_marker_empty = s.user_marker.as_ref().is_none_or(|m| m.is_none());
        main_empty
            && op_empty
            && wt_state_empty
            && upstream_empty
            && working_tree_empty
            && user_marker_empty
    }

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
    fn test_status_symbols_is_empty() {
        let symbols = StatusSymbols::default();
        assert!(is_empty(&symbols));

        let symbols = StatusSymbols {
            main_state: Some(MainState::Ahead),
            ..Default::default()
        };
        assert!(!is_empty(&symbols));

        let symbols = StatusSymbols {
            operation_state: Some(OperationState::Rebase),
            ..Default::default()
        };
        assert!(!is_empty(&symbols));

        let symbols = StatusSymbols {
            worktree_state: Some(WorktreeState::Locked),
            ..Default::default()
        };
        assert!(!is_empty(&symbols));

        let symbols = StatusSymbols {
            upstream_divergence: Some(Divergence::Ahead),
            ..Default::default()
        };
        assert!(!is_empty(&symbols));

        let symbols = StatusSymbols {
            working_tree: Some(WorkingTreeStatus::new(true, false, false, false, false)),
            ..Default::default()
        };
        assert!(!is_empty(&symbols));

        let symbols = StatusSymbols {
            user_marker: Some(Some("🔥".to_string())),
            ..Default::default()
        };
        assert!(!is_empty(&symbols));

        // Gates resolved to the "None" variant are still is_empty == true
        // (resolved, but nothing to show). This matches the pre-step-2
        // behavior for a cleanly-computed row that has no visible symbols.
        let symbols = StatusSymbols {
            main_state: Some(MainState::None),
            operation_state: Some(OperationState::None),
            worktree_state: Some(WorktreeState::None),
            upstream_divergence: Some(Divergence::None),
            working_tree: Some(WorkingTreeStatus::default()),
            user_marker: Some(None),
        };
        assert!(is_empty(&symbols));
    }

    #[test]
    fn test_status_symbols_format_compact() {
        // Empty symbols
        let symbols = StatusSymbols::default();
        assert_eq!(symbols.format_compact(), "");

        // Single symbol
        let symbols = StatusSymbols {
            main_state: Some(MainState::Ahead),
            ..Default::default()
        };
        assert_snapshot!(symbols.format_compact(), @"[2m↑[22m");

        // Multiple symbols
        let symbols = StatusSymbols {
            working_tree: Some(WorkingTreeStatus::new(true, true, false, false, false)),
            main_state: Some(MainState::Ahead),
            ..Default::default()
        };
        assert_snapshot!(symbols.format_compact(), @"[36m+[39m[36m![39m[2m↑[22m");
    }

    #[test]
    fn test_status_symbols_render_with_mask() {
        let symbols = StatusSymbols {
            main_state: Some(MainState::Ahead),
            ..Default::default()
        };
        let rendered = symbols.render_with_mask(&PositionMask::FULL, "·");
        assert_snapshot!(rendered, @"[2m·[0m[2m·[0m[2m·[0m[2m·[0m[2m↑[22m[2m·[0m[2m·[0m");
    }

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
}
