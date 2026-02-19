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
    Summary,
    Upstream,
    CiStatus,
    Path,
    Url, // Dev server URL from project config template
    Commit,
    Time,
    Message,
}

impl ColumnKind {
    pub const fn header(self) -> &'static str {
        match self {
            ColumnKind::Gutter => "",
            ColumnKind::Branch => "Branch",
            ColumnKind::Status => "Status",
            ColumnKind::WorkingDiff => "HEAD±",
            ColumnKind::AheadBehind => "main↕",
            ColumnKind::BranchDiff => "main…±",
            ColumnKind::Path => "Path",
            ColumnKind::Upstream => "Remote⇅",
            ColumnKind::Url => "URL",
            ColumnKind::Time => "Age",
            ColumnKind::CiStatus => "CI",
            ColumnKind::Commit => "Commit",
            ColumnKind::Summary => "Summary",
            ColumnKind::Message => "Message",
        }
    }

    /// Get the base priority for this column (lower = more important).
    ///
    /// Used by both `wt list` layout and statusline truncation to ensure
    /// consistent priority ordering across commands.
    pub fn priority(self) -> u8 {
        COLUMN_SPECS
            .iter()
            .find(|spec| spec.kind == self)
            .map(|spec| spec.base_priority)
            .unwrap_or(u8::MAX)
    }
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
    pub base_priority: u8,
    /// Task required for this column's data. If Some and task is skipped, column is hidden.
    pub requires_task: Option<TaskKind>,
}

impl ColumnSpec {
    pub const fn new(kind: ColumnKind, base_priority: u8, requires_task: Option<TaskKind>) -> Self {
        Self {
            kind,
            base_priority,
            requires_task,
        }
    }
}

/// Static registry of all possible columns in display order.
///
/// Note: base_priority determines truncation order (lower = kept longer),
/// which is independent of display order (position in array).
pub const COLUMN_SPECS: &[ColumnSpec] = &[
    ColumnSpec::new(ColumnKind::Gutter, 0, None),
    ColumnSpec::new(ColumnKind::Branch, 1, None),
    ColumnSpec::new(ColumnKind::Status, 2, None),
    ColumnSpec::new(ColumnKind::WorkingDiff, 3, None),
    ColumnSpec::new(ColumnKind::AheadBehind, 4, None),
    ColumnSpec::new(ColumnKind::BranchDiff, 6, Some(TaskKind::BranchDiff)),
    ColumnSpec::new(ColumnKind::Summary, 10, Some(TaskKind::SummaryGenerate)),
    ColumnSpec::new(ColumnKind::Upstream, 8, None),
    ColumnSpec::new(ColumnKind::CiStatus, 5, Some(TaskKind::CiStatus)),
    ColumnSpec::new(ColumnKind::Path, 7, None),
    ColumnSpec::new(ColumnKind::Url, 9, Some(TaskKind::UrlStatus)),
    ColumnSpec::new(ColumnKind::Commit, 11, None),
    ColumnSpec::new(ColumnKind::Time, 12, None),
    ColumnSpec::new(ColumnKind::Message, 13, None),
];

pub fn column_display_index(kind: ColumnKind) -> usize {
    COLUMN_SPECS
        .iter()
        .position(|spec| spec.kind == kind)
        .unwrap_or(usize::MAX)
}

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
            ColumnKind::Summary,
            ColumnKind::Upstream,
            ColumnKind::CiStatus,
            ColumnKind::Path,
            ColumnKind::Url,
            ColumnKind::Commit,
            ColumnKind::Time,
            ColumnKind::Message,
        ];
        assert_eq!(kinds, expected, "column order should match display layout");
    }

    #[test]
    fn columns_gate_on_required_tasks() {
        let branch_diff = COLUMN_SPECS
            .iter()
            .find(|c| c.kind == ColumnKind::BranchDiff)
            .unwrap();
        assert_eq!(branch_diff.requires_task, Some(TaskKind::BranchDiff));

        let url = COLUMN_SPECS
            .iter()
            .find(|c| c.kind == ColumnKind::Url)
            .unwrap();
        assert_eq!(url.requires_task, Some(TaskKind::UrlStatus));

        let ci_status = COLUMN_SPECS
            .iter()
            .find(|c| c.kind == ColumnKind::CiStatus)
            .unwrap();
        assert_eq!(ci_status.requires_task, Some(TaskKind::CiStatus));

        let summary = COLUMN_SPECS
            .iter()
            .find(|c| c.kind == ColumnKind::Summary)
            .unwrap();
        assert_eq!(summary.requires_task, Some(TaskKind::SummaryGenerate));

        // All other columns should not require a background task to render
        for spec in COLUMN_SPECS {
            if spec.kind != ColumnKind::BranchDiff
                && spec.kind != ColumnKind::Url
                && spec.kind != ColumnKind::CiStatus
                && spec.kind != ColumnKind::Summary
            {
                assert!(
                    spec.requires_task.is_none(),
                    "{:?} unexpectedly requires a task",
                    spec.kind
                );
            }
        }
    }

    #[test]
    fn test_column_spec_new() {
        let spec = ColumnSpec::new(ColumnKind::Branch, 1, None);
        assert_eq!(spec.kind, ColumnKind::Branch);
        assert_eq!(spec.base_priority, 1);
        assert!(spec.requires_task.is_none());
    }

    #[test]
    fn test_column_spec_with_required_task() {
        let spec = ColumnSpec::new(ColumnKind::BranchDiff, 5, Some(TaskKind::BranchDiff));
        assert_eq!(spec.kind, ColumnKind::BranchDiff);
        assert_eq!(spec.requires_task, Some(TaskKind::BranchDiff));
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
        for kind in COLUMN_SPECS.iter().map(|spec| spec.kind) {
            if kind != ColumnKind::Gutter {
                assert!(
                    !kind.header().is_empty(),
                    "{:?} should have a non-empty header",
                    kind
                );
            }
        }
    }

    #[test]
    fn test_all_column_kinds_have_priority() {
        // Every ColumnKind variant must be in COLUMN_SPECS so priority() works correctly.
        // If this fails, a new variant was added but not registered in COLUMN_SPECS.
        let all_kinds = [
            ColumnKind::Gutter,
            ColumnKind::Branch,
            ColumnKind::Status,
            ColumnKind::WorkingDiff,
            ColumnKind::AheadBehind,
            ColumnKind::BranchDiff,
            ColumnKind::Path,
            ColumnKind::Upstream,
            ColumnKind::Url,
            ColumnKind::CiStatus,
            ColumnKind::Commit,
            ColumnKind::Time,
            ColumnKind::Summary,
            ColumnKind::Message,
        ];

        for kind in all_kinds {
            let priority = kind.priority();
            assert!(
                priority != u8::MAX,
                "{:?} not found in COLUMN_SPECS (priority returned u8::MAX)",
                kind
            );
        }
    }
}
