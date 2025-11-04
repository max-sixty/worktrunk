/// Logical identifier for each column rendered by `wt list`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColumnKind {
    Branch,
    Status,
    WorkingDiff,
    AheadBehind,
    BranchDiff,
    States,
    Path,
    Upstream,
    Time,
    CiStatus,
    Commit,
    Message,
}

/// Differentiates between diff-style columns with plus/minus symbols and those with arrows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffVariant {
    Signs,
    Arrows,
}

/// Static metadata describing a column's behavior in both layout and rendering.
#[derive(Clone, Copy, Debug)]
pub struct ColumnSpec {
    pub kind: ColumnKind,
    pub header: &'static str,
    pub base_priority: u8,
    pub requires_show_full: bool,
    pub requires_fetch_ci: bool,
    pub display_index: u8,
}

impl ColumnSpec {
    pub const fn new(
        kind: ColumnKind,
        header: &'static str,
        base_priority: u8,
        requires_show_full: bool,
        requires_fetch_ci: bool,
        display_index: u8,
    ) -> Self {
        Self {
            kind,
            header,
            base_priority,
            requires_show_full,
            requires_fetch_ci,
            display_index,
        }
    }
}

/// Static registry of all possible columns in display order.
pub const COLUMN_SPECS: &[ColumnSpec] = &[
    ColumnSpec::new(
        ColumnKind::Branch,
        super::layout::HEADER_BRANCH,
        1,
        false,
        false,
        0,
    ),
    ColumnSpec::new(
        ColumnKind::Status,
        super::layout::HEADER_STATUS,
        2,
        false,
        false,
        1,
    ),
    ColumnSpec::new(
        ColumnKind::WorkingDiff,
        super::layout::HEADER_WORKING_DIFF,
        3,
        false,
        false,
        2,
    ),
    ColumnSpec::new(
        ColumnKind::AheadBehind,
        super::layout::HEADER_AHEAD_BEHIND,
        4,
        false,
        false,
        3,
    ),
    ColumnSpec::new(
        ColumnKind::BranchDiff,
        super::layout::HEADER_BRANCH_DIFF,
        5,
        true,
        false,
        4,
    ),
    ColumnSpec::new(
        ColumnKind::States,
        super::layout::HEADER_STATE,
        6,
        false,
        false,
        5,
    ),
    ColumnSpec::new(
        ColumnKind::Path,
        super::layout::HEADER_PATH,
        7,
        false,
        false,
        6,
    ),
    ColumnSpec::new(
        ColumnKind::Upstream,
        super::layout::HEADER_UPSTREAM,
        8,
        false,
        false,
        7,
    ),
    ColumnSpec::new(
        ColumnKind::CiStatus,
        super::layout::HEADER_CI,
        9,
        false,
        true,
        8,
    ),
    ColumnSpec::new(
        ColumnKind::Commit,
        super::layout::HEADER_COMMIT,
        10,
        false,
        false,
        9,
    ),
    ColumnSpec::new(
        ColumnKind::Time,
        super::layout::HEADER_AGE,
        11,
        false,
        false,
        10,
    ),
    ColumnSpec::new(
        ColumnKind::Message,
        super::layout::HEADER_MESSAGE,
        12,
        false,
        false,
        11,
    ),
];
