mod ci_status;
mod columns;
mod layout;
mod model;
mod render;

#[cfg(test)]
mod spacing_test;

use layout::calculate_responsive_layout;
use model::{ListData, ListItem, gather_list_data};
use render::{
    format_ahead_behind_plain, format_ci_status_plain, format_diff_plain, format_header_line,
    format_list_item_line,
};
use worktrunk::git::{GitError, Repository};
use worktrunk::styling::{ADDITION, DELETION, INFO_EMOJI, println};

/// Helper to enrich common display fields shared between worktrees and branches
fn enrich_common_fields(
    counts: &model::AheadBehind,
    branch_diff: &model::BranchDiffTotals,
    upstream: &model::UpstreamStatus,
    pr_status: &Option<ci_status::PrStatus>,
) -> model::DisplayFields {
    let commits_display = format_ahead_behind_plain(counts.ahead, counts.behind);

    let (added, deleted) = branch_diff.diff;
    let branch_diff_display = format_diff_plain(added, deleted);

    let upstream_display = upstream
        .active()
        .map(|(_, upstream_ahead, upstream_behind)| {
            // Always show arrows for upstream (even ↑0 ↓0) to distinguish from no remote
            format_ahead_behind_plain(upstream_ahead, upstream_behind).unwrap_or_else(|| {
                let dim_deletion = DELETION.dimmed();
                format!(
                    "{}↑{}{} {}↓{}{}",
                    ADDITION,
                    0,
                    ADDITION.render_reset(),
                    dim_deletion,
                    0,
                    dim_deletion.render_reset()
                )
            })
        });

    let ci_status_display = pr_status.as_ref().map(format_ci_status_plain);

    model::DisplayFields {
        commits_display,
        branch_diff_display,
        upstream_display,
        ci_status_display,
    }
}

/// Enrich a ListItem with display fields for json-pretty format
fn enrich_with_display_fields(mut item: ListItem) -> ListItem {
    match &mut item {
        ListItem::Worktree(info) => {
            info.display = enrich_common_fields(
                &info.counts,
                &info.branch_diff,
                &info.upstream,
                &info.pr_status,
            );

            // Working tree specific field
            let (added, deleted) = info.working_tree_diff;
            info.working_diff_display = format_diff_plain(added, deleted);
        }
        ListItem::Branch(info) => {
            info.display = enrich_common_fields(
                &info.counts,
                &info.branch_diff,
                &info.upstream,
                &info.pr_status,
            );
        }
    }
    item
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
    }) = gather_list_data(&repo, show_branches, show_full, show_full)?
    else {
        return Ok(());
    };

    match format {
        crate::OutputFormat::Json => {
            let enriched_items: Vec<_> =
                items.into_iter().map(enrich_with_display_fields).collect();

            let json = serde_json::to_string_pretty(&enriched_items).map_err(|e| {
                GitError::CommandFailed(format!("Failed to serialize to JSON: {}", e))
            })?;
            println!("{}", json);
        }
        crate::OutputFormat::Table => {
            let layout = calculate_responsive_layout(&items, show_full, show_full);
            format_header_line(&layout);
            for item in &items {
                format_list_item_line(item, &layout, current_worktree_path.as_ref());
            }
            display_summary(&items, show_branches);
        }
    }

    Ok(())
}

fn display_summary(items: &[ListItem], include_branches: bool) {
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

    if metrics.behind_items > 0 {
        parts.push(format!("{} behind", metrics.behind_items));
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
    behind_items: usize,
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
        if counts.behind > 0 {
            self.behind_items += 1;
        }
    }
}
