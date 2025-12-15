use super::collect::TaskKind;

/// Logical identifier for each column rendered by `wt list`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColumnKind {
    Gutter, // Type indicator: `@` (current), `^` (main), `+` (worktree), space (branch-only)
    Branch,
    Status, // Includes both git status symbols and user-defined status
    WorkingDiff,
    AheadBehind,
    BranchDiff,
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
    /// Simple arrows (↑↓) for commits ahead/behind main
    Arrows,
    /// Double-struck arrows (⇡⇣) for commits ahead/behind remote
    UpstreamArrows,
}

/// Static metadata describing a column's behavior in both layout and rendering.
#[derive(Clone, Copy, Debug)]
pub struct ColumnSpec {
    pub kind: ColumnKind,
    pub header: &'static str,
    pub base_priority: u8,
    /// Task required for this column's data. If Some and task is skipped, column is hidden.
    pub requires_task: Option<TaskKind>,
    pub display_index: u8,
}

impl ColumnSpec {
    pub const fn new(
        kind: ColumnKind,
        header: &'static str,
        base_priority: u8,
        requires_task: Option<TaskKind>,
        display_index: u8,
    ) -> Self {
        Self {
            kind,
            header,
            base_priority,
            requires_task,
            display_index,
        }
    }
}

/// Static registry of all possible columns in display order.
pub const COLUMN_SPECS: &[ColumnSpec] = &[
    ColumnSpec::new(ColumnKind::Gutter, super::layout::HEADER_GUTTER, 0, None, 0),
    ColumnSpec::new(ColumnKind::Branch, super::layout::HEADER_BRANCH, 1, None, 1),
    ColumnSpec::new(ColumnKind::Status, super::layout::HEADER_STATUS, 2, None, 2),
    ColumnSpec::new(
        ColumnKind::WorkingDiff,
        super::layout::HEADER_WORKING_DIFF,
        3,
        None,
        3,
    ),
    ColumnSpec::new(
        ColumnKind::AheadBehind,
        super::layout::HEADER_AHEAD_BEHIND,
        4,
        None,
        4,
    ),
    ColumnSpec::new(
        ColumnKind::BranchDiff,
        super::layout::HEADER_BRANCH_DIFF,
        5,
        Some(TaskKind::BranchDiff),
        5,
    ),
    ColumnSpec::new(ColumnKind::Path, super::layout::HEADER_PATH, 6, None, 6),
    ColumnSpec::new(
        ColumnKind::Upstream,
        super::layout::HEADER_UPSTREAM,
        7,
        None,
        7,
    ),
    ColumnSpec::new(
        ColumnKind::CiStatus,
        super::layout::HEADER_CI,
        8,
        Some(TaskKind::CiStatus),
        8,
    ),
    ColumnSpec::new(ColumnKind::Commit, super::layout::HEADER_COMMIT, 9, None, 9),
    ColumnSpec::new(ColumnKind::Time, super::layout::HEADER_AGE, 10, None, 10),
    ColumnSpec::new(
        ColumnKind::Message,
        super::layout::HEADER_MESSAGE,
        11,
        None,
        11,
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn columns_are_ordered_and_unique() {
        let kinds: Vec<ColumnKind> = COLUMN_SPECS.iter().map(|c| c.kind).collect();
        let expected = vec![
            ColumnKind::Gutter,
            ColumnKind::Branch,
            ColumnKind::Status,
            ColumnKind::WorkingDiff,
            ColumnKind::AheadBehind,
            ColumnKind::BranchDiff,
            ColumnKind::Path,
            ColumnKind::Upstream,
            ColumnKind::CiStatus,
            ColumnKind::Commit,
            ColumnKind::Time,
            ColumnKind::Message,
        ];
        assert_eq!(kinds, expected, "column order should match display layout");

        // display_index should match position to keep layout lookups O(1)
        for (idx, spec) in COLUMN_SPECS.iter().enumerate() {
            assert_eq!(
                spec.display_index as usize, idx,
                "display_index must be contiguous"
            );
        }
    }

    #[test]
    fn columns_gate_on_required_tasks() {
        let branch_diff = COLUMN_SPECS
            .iter()
            .find(|c| c.kind == ColumnKind::BranchDiff)
            .unwrap();
        assert_eq!(branch_diff.requires_task, Some(TaskKind::BranchDiff));

        let ci_status = COLUMN_SPECS
            .iter()
            .find(|c| c.kind == ColumnKind::CiStatus)
            .unwrap();
        assert_eq!(ci_status.requires_task, Some(TaskKind::CiStatus));

        // All other columns should not require a background task to render
        for spec in COLUMN_SPECS {
            if spec.kind != ColumnKind::BranchDiff && spec.kind != ColumnKind::CiStatus {
                assert!(
                    spec.requires_task.is_none(),
                    "{:?} unexpectedly requires a task",
                    spec.kind
                );
            }
        }
    }

    #[test]
    fn test_column_kind_debug() {
        // Test Debug trait implementation
        assert_eq!(format!("{:?}", ColumnKind::Gutter), "Gutter");
        assert_eq!(format!("{:?}", ColumnKind::Branch), "Branch");
        assert_eq!(format!("{:?}", ColumnKind::Status), "Status");
        assert_eq!(format!("{:?}", ColumnKind::Message), "Message");
    }

    #[test]
    fn test_column_kind_copy() {
        let kind = ColumnKind::Branch;
        let copied1 = kind; // Copy trait
        let copied2 = kind;
        assert_eq!(kind, copied1);
        assert_eq!(kind, copied2);
    }

    #[test]
    fn test_column_kind_hash() {
        let mut set = HashSet::new();
        set.insert(ColumnKind::Branch);
        set.insert(ColumnKind::Path);
        assert!(set.contains(&ColumnKind::Branch));
        assert!(set.contains(&ColumnKind::Path));
        assert!(!set.contains(&ColumnKind::Message));
    }

    #[test]
    fn test_diff_variant_debug() {
        assert_eq!(format!("{:?}", DiffVariant::Signs), "Signs");
        assert_eq!(format!("{:?}", DiffVariant::Arrows), "Arrows");
        assert_eq!(
            format!("{:?}", DiffVariant::UpstreamArrows),
            "UpstreamArrows"
        );
    }

    #[test]
    fn test_diff_variant_copy() {
        let variant = DiffVariant::Signs;
        let copied1 = variant; // Copy trait
        let copied2 = variant;
        assert_eq!(variant, copied1);
        assert_eq!(variant, copied2);
    }

    #[test]
    fn test_column_spec_new() {
        let spec = ColumnSpec::new(ColumnKind::Branch, "Branch", 1, None, 1);
        assert_eq!(spec.kind, ColumnKind::Branch);
        assert_eq!(spec.header, "Branch");
        assert_eq!(spec.base_priority, 1);
        assert!(spec.requires_task.is_none());
        assert_eq!(spec.display_index, 1);
    }

    #[test]
    fn test_column_spec_with_required_task() {
        let spec = ColumnSpec::new(
            ColumnKind::BranchDiff,
            "main…±",
            5,
            Some(TaskKind::BranchDiff),
            5,
        );
        assert_eq!(spec.kind, ColumnKind::BranchDiff);
        assert_eq!(spec.requires_task, Some(TaskKind::BranchDiff));
    }

    #[test]
    fn test_column_spec_debug() {
        let spec = ColumnSpec::new(ColumnKind::Path, "Path", 6, None, 6);
        let debug_str = format!("{:?}", spec);
        assert!(debug_str.contains("ColumnSpec"));
        assert!(debug_str.contains("Path"));
    }

    #[test]
    fn test_column_spec_copy() {
        let spec = ColumnSpec::new(ColumnKind::Time, "Age", 10, None, 10);
        let copied = spec; // Copy trait - should work without clone
        assert_eq!(spec.kind, copied.kind);
        assert_eq!(spec.header, copied.header);
        assert_eq!(spec.base_priority, copied.base_priority);
    }

    #[test]
    fn test_column_specs_priorities_are_unique() {
        // Each column should have a unique base_priority
        let priorities: Vec<u8> = COLUMN_SPECS.iter().map(|c| c.base_priority).collect();
        let unique: HashSet<u8> = priorities.iter().cloned().collect();
        assert_eq!(
            priorities.len(),
            unique.len(),
            "base_priority values should be unique"
        );
    }

    #[test]
    fn test_column_specs_headers_are_non_empty() {
        // All columns except Gutter should have non-empty headers
        for spec in COLUMN_SPECS {
            if spec.kind != ColumnKind::Gutter {
                assert!(
                    !spec.header.is_empty(),
                    "{:?} should have a non-empty header",
                    spec.kind
                );
            }
        }
    }
}
