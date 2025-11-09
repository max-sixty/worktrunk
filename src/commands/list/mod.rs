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
use layout::calculate_responsive_layout;
use model::{ListData, ListItem};
use worktrunk::git::{GitError, Repository};
use worktrunk::styling::{INFO_EMOJI, println};

/// Helper to enrich common display fields shared between worktrees and branches
fn enrich_common_fields(
    counts: &model::AheadBehind,
    branch_diff: &model::BranchDiffTotals,
    upstream: &model::UpstreamStatus,
    pr_status: &Option<ci_status::PrStatus>,
) -> model::DisplayFields {
    let commits_display = ColumnKind::AheadBehind.format_diff_plain(counts.ahead, counts.behind);

    let (added, deleted) = branch_diff.diff;
    let branch_diff_display = ColumnKind::BranchDiff.format_diff_plain(added, deleted);

    let upstream_display = upstream
        .active()
        .and_then(|(_, upstream_ahead, upstream_behind)| {
            ColumnKind::Upstream.format_diff_plain(upstream_ahead, upstream_behind)
        });

    let ci_status_display = pr_status.as_ref().map(ci_status::PrStatus::format_plain);

    model::DisplayFields {
        commits_display,
        branch_diff_display,
        upstream_display,
        ci_status_display,
        status_display: None, // Status display is populated in WorktreeInfo/BranchInfo constructors
    }
}

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
            display_summary(&items, show_branches, &layout);
        }
    }

    Ok(())
}

fn display_summary(items: &[ListItem], include_branches: bool, layout: &layout::LayoutConfig) {
    use anstyle::Style;

    if items.is_empty() {
        println!();
        use worktrunk::styling::{HINT, HINT_EMOJI};
        println!("{HINT_EMOJI} {HINT}No worktrees found{HINT:#}");
        println!("{HINT_EMOJI} {HINT}Create one with: wt switch --create <branch>{HINT:#}");
        return;
    }

    let mut metrics = SummaryMetrics::default();
    for item in items {
        metrics.update(item);
    }

    println!();
    let dim = Style::new().dimmed();

    // Build summary parts
    let mut parts = Vec::new();

    if include_branches {
        parts.push(format!("{} worktrees", metrics.worktrees));
        if metrics.branches > 0 {
            parts.push(format!("{} branches", metrics.branches));
        }
    } else {
        let plural = if metrics.worktrees == 1 { "" } else { "s" };
        parts.push(format!("{} worktree{}", metrics.worktrees, plural));
    }

    if metrics.dirty_worktrees > 0 {
        parts.push(format!("{} with changes", metrics.dirty_worktrees));
    }

    if metrics.ahead_items > 0 {
        parts.push(format!("{} ahead", metrics.ahead_items));
    }

    if layout.hidden_nonempty_count > 0 {
        let plural = if layout.hidden_nonempty_count == 1 {
            "column"
        } else {
            "columns"
        };
        parts.push(format!(
            "{} {} hidden",
            layout.hidden_nonempty_count, plural
        ));
    }

    let summary = parts.join(", ");
    println!("{INFO_EMOJI} {dim}Showing {summary}{dim:#}");
}

#[derive(Default)]
struct SummaryMetrics {
    worktrees: usize,
    branches: usize,
    dirty_worktrees: usize,
    ahead_items: usize,
}

impl SummaryMetrics {
    fn update(&mut self, item: &ListItem) {
        if let Some(info) = item.worktree_info() {
            self.worktrees += 1;
            let (added, deleted) = info.working_tree_diff;
            if added > 0 || deleted > 0 {
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
}

impl ListItem {
    /// Enrich a ListItem with display fields for json-pretty format.
    fn with_display_fields(mut self) -> Self {
        match &mut self {
            ListItem::Worktree(info) => {
                let mut display = enrich_common_fields(
                    &info.counts,
                    &info.branch_diff,
                    &info.upstream,
                    &info.pr_status,
                );
                // Preserve status_display that was set in constructor
                display.status_display = info.display.status_display.clone();
                info.display = display;

                // Working tree specific field
                let (added, deleted) = info.working_tree_diff;
                info.working_diff_display =
                    ColumnKind::WorkingDiff.format_diff_plain(added, deleted);
            }
            ListItem::Branch(info) => {
                let mut display = enrich_common_fields(
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
