//! Jujutsu workspace data collection for `wt list`.
//!
//! Simpler than the git collection path: sequential execution, no progressive
//! rendering. jj repos typically have few workspaces (1-5), so parallelism
//! isn't needed for acceptable latency.

use worktrunk::workspace::{JjWorkspace, Workspace, WorkspaceItem};

use super::model::item::{DisplayFields, ItemKind, ListData, ListItem, WorktreeData};
use super::model::stats::{AheadBehind, CommitDetails};

/// Collect workspace data from a jj repository.
///
/// Returns `ListData` compatible with the existing rendering pipeline.
/// Fields not applicable to jj (upstream, CI, branch diff) are left as None.
pub fn collect_jj(workspace: &JjWorkspace) -> anyhow::Result<ListData> {
    let workspaces = workspace.list_workspaces()?;

    // Determine current workspace by matching cwd
    let cwd = dunce::canonicalize(std::env::current_dir()?)?;

    let mut items: Vec<ListItem> = Vec::with_capacity(workspaces.len());

    for ws in &workspaces {
        let is_current = dunce::canonicalize(&ws.path)
            .map(|p| cwd.starts_with(&p))
            .unwrap_or(false);

        let mut item = build_jj_item(ws, is_current);

        // Collect status data (sequential — few workspaces expected)
        populate_jj_item(workspace, ws, &mut item);

        // Compute status symbols with available data
        item.compute_status_symbols(None, false, None, None, false);
        item.finalize_display();

        items.push(item);
    }

    // Sort: current first, then default, then by name
    items.sort_by(|a, b| {
        let a_current = a.worktree_data().is_some_and(|d| d.is_current);
        let b_current = b.worktree_data().is_some_and(|d| d.is_current);
        let a_main = a.worktree_data().is_some_and(|d| d.is_main);
        let b_main = b.worktree_data().is_some_and(|d| d.is_main);

        b_current
            .cmp(&a_current)
            .then(b_main.cmp(&a_main))
            .then(a.branch_name().cmp(b.branch_name()))
    });

    let main_path = workspace
        .default_workspace_path()?
        .unwrap_or_else(|| workspace.root().to_path_buf());

    Ok(ListData {
        items,
        main_worktree_path: main_path,
    })
}

/// Build a minimal ListItem for a jj workspace.
fn build_jj_item(ws: &WorkspaceItem, is_current: bool) -> ListItem {
    ListItem {
        head: ws.head.clone(),
        // Use workspace name as the "branch" for display purposes
        branch: Some(ws.name.clone()),
        commit: None,
        counts: None,
        branch_diff: None,
        committed_trees_match: None,
        has_file_changes: None,
        would_merge_add: None,
        is_ancestor: None,
        is_orphan: None,
        upstream: None,
        pr_status: None,
        url: None,
        url_active: None,
        status_symbols: None,
        display: DisplayFields::default(),
        kind: ItemKind::Worktree(Box::new(WorktreeData {
            path: ws.path.clone(),
            detached: false,
            locked: ws.locked.clone(),
            prunable: ws.prunable.clone(),
            is_main: ws.is_default,
            is_current,
            is_previous: false,
            ..Default::default()
        })),
    }
}

/// Populate computed fields for a jj workspace item.
///
/// Collects working tree diff, ahead/behind counts, and integration status.
/// Errors are logged but don't fail the overall collection.
fn populate_jj_item(workspace: &JjWorkspace, ws: &WorkspaceItem, item: &mut ListItem) {
    // Skip status collection for the default workspace if it's the trunk target
    let is_main = ws.is_default;

    // Working tree diff
    match workspace.working_diff(&ws.path) {
        Ok(diff) => {
            if let ItemKind::Worktree(ref mut data) = item.kind {
                data.working_tree_diff = Some(diff);
            }
        }
        Err(e) => log::debug!("Failed to get working diff for {}: {}", ws.name, e),
    }

    // Ahead/behind vs trunk (skip for default workspace)
    if !is_main {
        // Use trunk() revset as base, workspace change ID as head
        match workspace.ahead_behind("trunk()", &ws.head) {
            Ok((ahead, behind)) => {
                item.counts = Some(AheadBehind { ahead, behind });
            }
            Err(e) => log::debug!("Failed to get ahead/behind for {}: {}", ws.name, e),
        }

        // Integration check
        match workspace.is_integrated(&ws.head, "trunk()") {
            Ok(reason) => {
                if reason.is_some() {
                    item.is_ancestor = Some(true);
                }
            }
            Err(e) => log::debug!("Failed integration check for {}: {}", ws.name, e),
        }
    }

    // Commit details (timestamp + description)
    match workspace.commit_details(&ws.path) {
        Ok((timestamp, commit_message)) => {
            item.commit = Some(CommitDetails {
                timestamp,
                commit_message,
            });
        }
        Err(e) => log::debug!("Failed to get commit details for {}: {}", ws.name, e),
    }
}
