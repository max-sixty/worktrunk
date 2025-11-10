mod ci_status;
mod columns;
mod layout;
pub mod model;
mod render;

#[cfg(test)]
mod spacing_test;
#[cfg(test)]
mod status_column_tests;

use super::repository_ext::RepositoryCliExt;
use columns::ColumnKind;
use layout::{LayoutConfig, calculate_responsive_layout};
use model::{DisplayFields, ListData, ListItem};
use worktrunk::git::{GitError, Repository};
use worktrunk::styling::{INFO_EMOJI, println};

pub fn handle_list(
    format: crate::OutputFormat,
    show_branches: bool,
    show_full: bool,
) -> Result<(), GitError> {
    let repo = Repository::current();
    let Some(ListData {
        items,
        current_worktree_path,
    }) = repo.gather_list_data(show_branches, show_full, show_full)?
    else {
        return Ok(());
    };

    match format {
        crate::OutputFormat::Json => {
            let enriched_items: Vec<_> = items
                .into_iter()
                .map(ListItem::with_display_fields)
                .collect();

            let json = serde_json::to_string_pretty(&enriched_items).map_err(|e| {
                GitError::CommandFailed(format!("Failed to serialize to JSON: {}", e))
            })?;
            println!("{}", json);
        }
        crate::OutputFormat::Table => {
            let layout = calculate_responsive_layout(&items, show_full, show_full);
            layout.format_header_line();
            for item in &items {
                layout.format_list_item_line(item, current_worktree_path.as_ref());
            }
            layout.render_summary(&items, show_branches);
        }
    }

    Ok(())
}

#[derive(Default)]
struct SummaryMetrics {
    worktrees: usize,
    branches: usize,
    dirty_worktrees: usize,
    ahead_items: usize,
}

impl SummaryMetrics {
    fn from_items(items: &[ListItem]) -> Self {
        let mut metrics = Self::default();
        for item in items {
            metrics.update(item);
        }
        metrics
    }

    fn update(&mut self, item: &ListItem) {
        if let Some(info) = item.worktree_info() {
            self.worktrees += 1;
            if !info.working_tree_diff.is_empty() {
                self.dirty_worktrees += 1;
            }
        } else {
            self.branches += 1;
        }

        let counts = item.counts();
        if counts.ahead > 0 {
            self.ahead_items += 1;
        }
    }

    fn summary_parts(&self, include_branches: bool, hidden_columns: usize) -> Vec<String> {
        let mut parts = Vec::new();

        if include_branches {
            parts.push(format!("{} worktrees", self.worktrees));
            if self.branches > 0 {
                parts.push(format!("{} branches", self.branches));
            }
        } else {
            let plural = if self.worktrees == 1 { "" } else { "s" };
            parts.push(format!("{} worktree{}", self.worktrees, plural));
        }

        if self.dirty_worktrees > 0 {
            parts.push(format!("{} with changes", self.dirty_worktrees));
        }

        if self.ahead_items > 0 {
            parts.push(format!("{} ahead", self.ahead_items));
        }

        if hidden_columns > 0 {
            let plural = if hidden_columns == 1 {
                "column"
            } else {
                "columns"
            };
            parts.push(format!("{} {} hidden", hidden_columns, plural));
        }

        parts
    }
}

impl LayoutConfig {
    fn render_summary(&self, items: &[ListItem], include_branches: bool) {
        use anstyle::Style;

        if items.is_empty() {
            println!();
            use worktrunk::styling::{HINT, HINT_EMOJI};
            println!("{HINT_EMOJI} {HINT}No worktrees found{HINT:#}");
            println!("{HINT_EMOJI} {HINT}Create one with: wt switch --create <branch>{HINT:#}");
            return;
        }

        let metrics = SummaryMetrics::from_items(items);

        println!();
        let dim = Style::new().dimmed();

        let summary = metrics
            .summary_parts(include_branches, self.hidden_nonempty_count)
            .join(", ");
        println!("{INFO_EMOJI} {dim}Showing {summary}{dim:#}");
    }
}

impl ListItem {
    /// Enrich a ListItem with display fields for json-pretty format.
    fn with_display_fields(mut self) -> Self {
        match &mut self {
            ListItem::Worktree(info) => {
                let mut display = DisplayFields::from_common_fields(
                    &info.counts,
                    &info.branch_diff,
                    &info.upstream,
                    &info.pr_status,
                );
                // Preserve status_display that was set in constructor
                display.status_display = info.display.status_display.clone();
                info.display = display;

                // Working tree specific field
                info.working_diff_display = ColumnKind::WorkingDiff.format_diff_plain(
                    info.working_tree_diff.added,
                    info.working_tree_diff.deleted,
                );
            }
            ListItem::Branch(info) => {
                let mut display = DisplayFields::from_common_fields(
                    &info.counts,
                    &info.branch_diff,
                    &info.upstream,
                    &info.pr_status,
                );
                // Preserve status_display that was set in constructor
                display.status_display = info.display.status_display.clone();
                info.display = display;
            }
        }
        self
    }
}
