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
    /// User-defined column from `[list.custom-columns]`; the index points into the
    /// resolved column list for this invocation. Values are expanded before
    /// layout, so widths are measured from content and headers come from the
    /// resolved name (not [`ColumnKind::header`]).
    Custom(u8),
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
            // Header is the resolved column name; layout substitutes it when
            // building `ColumnLayout`.
            ColumnKind::Custom(_) => "",
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

    /// Canonical kebab identifier for the `[list] columns` selection list.
    ///
    /// `None` for [`ColumnKind::Gutter`] (the worktree-type indicator, always
    /// shown and not user-selectable) and [`ColumnKind::Custom`] (addressed by
    /// its `[list.custom-columns]` header, not a static name). The exhaustive
    /// match forces every new variant to declare a name or opt out, and
    /// `test_config_name_round_trips` guards that the names stay unique and
    /// parseable.
    pub fn config_name(self) -> Option<&'static str> {
        Some(match self {
            ColumnKind::Gutter => return None,
            ColumnKind::Branch => "branch",
            ColumnKind::Status => "status",
            ColumnKind::WorkingDiff => "working-diff",
            ColumnKind::AheadBehind => "ahead-behind",
            ColumnKind::BranchDiff => "branch-diff",
            ColumnKind::Summary => "summary",
            ColumnKind::Upstream => "upstream",
            ColumnKind::CiStatus => "ci",
            ColumnKind::Path => "path",
            ColumnKind::Url => "url",
            ColumnKind::Commit => "commit",
            ColumnKind::Time => "age",
            ColumnKind::Message => "message",
            ColumnKind::Custom(_) => return None,
        })
    }

    /// Resolve a kebab name from `[list] columns` to its built-in column.
    ///
    /// Only built-ins registered in [`COLUMN_SPECS`] are selectable; Gutter
    /// (no name) and custom columns are unreachable here.
    pub fn from_config_name(name: &str) -> Option<ColumnKind> {
        COLUMN_SPECS
            .iter()
            .map(|spec| spec.kind)
            .find(|kind| kind.config_name() == Some(name))
    }

    /// All selectable column names in display order, for error messages and docs.
    pub fn selectable_names() -> Vec<&'static str> {
        COLUMN_SPECS
            .iter()
            .filter_map(|spec| spec.kind.config_name())
            .collect()
    }
}

/// Parse the `[list] columns` selection into an ordered list of built-in columns.
///
/// An empty input yields an empty selection (the caller treats that as "use the
/// default column set"). Unknown names and duplicates are hard errors so a typo
/// can't silently render a different table; the error lists every valid name.
/// Like `[list.custom-columns]`, this is validated at the `wt list` edge rather
/// than at config load — `ColumnKind` lives in the command layer, out of reach
/// of the config crate.
pub fn parse_selected_columns(names: &[String]) -> anyhow::Result<Vec<ColumnKind>> {
    let mut selected = Vec::with_capacity(names.len());
    for name in names {
        let kind = ColumnKind::from_config_name(name).ok_or_else(|| {
            anyhow::anyhow!(
                "Unknown column {name:?} in [list] columns. Valid columns: {}",
                ColumnKind::selectable_names().join(", ")
            )
        })?;
        if selected.contains(&kind) {
            anyhow::bail!("Duplicate column {name:?} in [list] columns");
        }
        selected.push(kind);
    }
    Ok(selected)
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
    /// If true, the column can shrink below its ideal width (down to header width)
    /// instead of being dropped entirely when space is tight.
    pub shrinkable: bool,
}

impl ColumnSpec {
    pub const fn new(kind: ColumnKind, base_priority: u8, requires_task: Option<TaskKind>) -> Self {
        Self {
            kind,
            base_priority,
            requires_task,
            shrinkable: false,
        }
    }

    pub const fn shrinkable(mut self) -> Self {
        self.shrinkable = true;
        self
    }
}

/// Static registry of all possible columns in display order.
///
/// Note: base_priority determines truncation order (lower = kept longer),
/// which is independent of display order (position in array).
pub const COLUMN_SPECS: &[ColumnSpec] = &[
    ColumnSpec::new(ColumnKind::Gutter, 0, None),
    ColumnSpec::new(ColumnKind::Branch, 1, None).shrinkable(),
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

/// Sort key for display order: (slot in `COLUMN_SPECS`, sub-order).
///
/// Custom columns share the Url slot with a non-zero sub-order, so they
/// render after Url and before Commit, in their resolution order.
pub fn column_display_index(kind: ColumnKind) -> (usize, usize) {
    if let ColumnKind::Custom(i) = kind {
        let url_slot = COLUMN_SPECS
            .iter()
            .position(|spec| spec.kind == ColumnKind::Url)
            .unwrap_or(usize::MAX);
        return (url_slot, i as usize + 1);
    }
    let slot = COLUMN_SPECS
        .iter()
        .position(|spec| spec.kind == kind)
        .unwrap_or(usize::MAX);
    (slot, 0)
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

    #[test]
    fn test_custom_columns_display_between_url_and_commit() {
        let url = column_display_index(ColumnKind::Url);
        let commit = column_display_index(ColumnKind::Commit);
        let first = column_display_index(ColumnKind::Custom(0));
        let second = column_display_index(ColumnKind::Custom(1));
        assert!(url < first, "custom columns render after Url");
        assert!(first < second, "custom columns keep resolution order");
        assert!(second < commit, "custom columns render before Commit");
    }

    #[test]
    fn test_config_name_round_trips() {
        // Drives off COLUMN_SPECS (single source of truth): every registered
        // built-in either has a unique, parseable name or is Gutter. A new
        // variant added to COLUMN_SPECS without a config_name arm fails the
        // exhaustive match in config_name() at compile time; one set to None
        // (other than Gutter) or colliding with another name fails here.
        for spec in COLUMN_SPECS {
            let kind = spec.kind;
            match kind.config_name() {
                Some(name) => assert_eq!(
                    ColumnKind::from_config_name(name),
                    Some(kind),
                    "{kind:?} name {name:?} must round-trip (unique + parseable)"
                ),
                None => assert_eq!(
                    kind,
                    ColumnKind::Gutter,
                    "only Gutter may lack a config_name; {kind:?} is missing one"
                ),
            }
        }
        // Custom columns are addressed by header, never by a static name.
        assert_eq!(ColumnKind::Custom(0).config_name(), None);
        assert_eq!(ColumnKind::from_config_name("gutter"), None);
        assert_eq!(ColumnKind::from_config_name("nonsense"), None);
    }

    #[test]
    fn test_parse_selected_columns() {
        let selected =
            parse_selected_columns(&["ci".into(), "branch".into(), "path".into()]).unwrap();
        assert_eq!(
            selected,
            vec![ColumnKind::CiStatus, ColumnKind::Branch, ColumnKind::Path],
            "selection preserves the configured order"
        );

        assert!(parse_selected_columns(&[]).unwrap().is_empty());

        let unknown = parse_selected_columns(&["branch".into(), "bogus".into()]).unwrap_err();
        assert!(unknown.to_string().contains("Unknown column"), "{unknown}");
        assert!(unknown.to_string().contains("bogus"), "{unknown}");
        // The error lists valid names so a typo is self-correcting.
        assert!(unknown.to_string().contains("ci"), "{unknown}");

        let dup = parse_selected_columns(&["branch".into(), "branch".into()]).unwrap_err();
        assert!(dup.to_string().contains("Duplicate column"), "{dup}");

        // Gutter is structural and not user-selectable.
        let gutter = parse_selected_columns(&["gutter".into()]).unwrap_err();
        assert!(gutter.to_string().contains("Unknown column"), "{gutter}");

        // Matching is exact: the rendered header "Branch" is not the kebab name.
        let cased = parse_selected_columns(&["Branch".into()]).unwrap_err();
        assert!(cased.to_string().contains("Unknown column"), "{cased}");
    }
}
