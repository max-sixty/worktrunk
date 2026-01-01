//! Global worktree discovery and collection for `wt list --global`.
//!
//! Scans a configured global worktree directory to discover worktrees from
//! multiple projects, groups them by parent repository, and collects metadata.

use anyhow::{Context, bail};
use rayon::prelude::*;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::commands::list::model::{ItemKind, ListItem};
use worktrunk::config::WorktrunkConfig;
use worktrunk::git::Repository;

/// Worktrees grouped by their parent project.
pub struct ProjectWorktrees {
    /// Display name derived from project_identifier()
    pub name: String,
    /// Path to the parent repository's main worktree
    pub path: PathBuf,
    /// Worktrees belonging to this project
    pub items: Vec<ListItem>,
}

/// All global worktree data grouped by project.
pub struct GlobalListData {
    pub projects: Vec<ProjectWorktrees>,
}

impl GlobalListData {
    /// Total number of worktrees across all projects
    pub fn worktree_count(&self) -> usize {
        self.projects.iter().map(|p| p.items.len()).sum()
    }

    /// Number of projects
    pub fn project_count(&self) -> usize {
        self.projects.len()
    }
}

/// Discover worktrees in the global directory by scanning for .git files.
///
/// Git creates a `.git` file (not directory) for linked worktrees containing
/// the path back to the parent repository's gitdir.
///
/// Returns tuples of (worktree_path, parent_gitdir).
fn discover_worktrees(global_dir: &Path) -> anyhow::Result<Vec<(PathBuf, PathBuf)>> {
    let mut worktrees = Vec::new();

    let entries = std::fs::read_dir(global_dir)
        .with_context(|| format!("Failed to read global worktree directory: {}", global_dir.display()))?;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let git_path = path.join(".git");

        if git_path.is_file() {
            if let Ok(Some(parent_gitdir)) = parse_git_file(&git_path) {
                worktrees.push((path, parent_gitdir));
            }
        } else if git_path.is_dir() {
            // Main worktree (not linked) - include it grouped by itself
            worktrees.push((path.clone(), git_path));
        }
    }

    Ok(worktrees)
}

/// Parse a .git file to extract the parent repository's gitdir path.
///
/// Format: "gitdir: /path/to/.git/worktrees/branch-name"
/// Returns the path to the parent .git directory (navigates up from worktrees/X).
fn parse_git_file(git_file: &Path) -> anyhow::Result<Option<PathBuf>> {
    let content = std::fs::read_to_string(git_file)
        .with_context(|| format!("Failed to read .git file: {}", git_file.display()))?;

    if let Some(gitdir) = content.strip_prefix("gitdir: ") {
        let gitdir = PathBuf::from(gitdir.trim());

        // Navigate up from .git/worktrees/X to .git
        // The gitdir points to .git/worktrees/<worktree-name>/
        if let Some(parent) = gitdir.parent().and_then(|p| p.parent()) {
            return Ok(Some(parent.to_path_buf()));
        }
    }

    Ok(None)
}

/// Group discovered worktrees by their parent repository's gitdir.
fn group_by_parent(worktrees: Vec<(PathBuf, PathBuf)>) -> HashMap<PathBuf, Vec<PathBuf>> {
    let mut groups: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();

    for (path, parent_gitdir) in worktrees {
        groups.entry(parent_gitdir).or_default().push(path);
    }

    groups
}

/// Collect global worktree data from all projects in the global directory.
pub fn collect_global(
    global_dir: &Path,
    _config: &WorktrunkConfig,
) -> anyhow::Result<GlobalListData> {
    // 1. Discover worktrees and group by parent gitdir
    let discovered = discover_worktrees(global_dir)?;

    if discovered.is_empty() {
        return Ok(GlobalListData { projects: vec![] });
    }

    let by_parent = group_by_parent(discovered);

    // 2. For each parent repo, create Repository and collect worktree data
    let projects: Vec<_> = by_parent
        .into_par_iter()
        .filter_map(|(gitdir, worktree_paths)| {
            match collect_project_worktrees(&gitdir, &worktree_paths) {
                Ok(project) => Some(project),
                Err(e) => {
                    log::warn!("Skipping project at {}: {}", gitdir.display(), e);
                    None
                }
            }
        })
        .collect();

    // Sort projects by name for consistent output
    let mut projects = projects;
    projects.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    Ok(GlobalListData { projects })
}

/// Collect worktree data for a single project.
fn collect_project_worktrees(
    gitdir: &Path,
    worktree_paths: &[PathBuf],
) -> anyhow::Result<ProjectWorktrees> {
    use crate::commands::list::collect::build_worktree_item;

    // Create repository from gitdir
    // For normal repos, gitdir ends with .git and we use the parent
    // For bare repos, the gitdir IS the repo path
    let worktree_path = if gitdir.file_name() == Some(OsStr::new(".git")) {
        gitdir.parent().unwrap_or(gitdir)
    } else {
        gitdir
    };
    let repo = Repository::at(worktree_path);

    // Get project identifier for display name
    let project_id = repo.project_identifier()?;
    // Use just the repo name for display
    let name = project_id
        .rsplit('/')
        .next()
        .unwrap_or(&project_id)
        .to_string();

    // Get all worktrees for this repository
    let all_worktrees = repo.list_worktrees()?;

    // Filter to only worktrees in global_dir paths
    let worktree_path_set: std::collections::HashSet<_> = worktree_paths
        .iter()
        .filter_map(|p| dunce::canonicalize(p).ok())
        .collect();

    let default_branch = repo.default_branch().unwrap_or_default();

    // Build items for each worktree in the global directory
    let items: Vec<_> = all_worktrees
        .into_iter()
        .filter(|wt| {
            dunce::canonicalize(&wt.path)
                .map(|p| worktree_path_set.contains(&p))
                .unwrap_or(false)
        })
        .map(|wt| {
            let is_main = wt.branch.as_ref() == Some(&default_branch);
            let is_current = false; // Not current in global listing context
            let is_previous = false;
            build_worktree_item(&wt, is_main, is_current, is_previous)
        })
        .collect();

    // Get the main worktree path for the project
    let path = repo.worktree_base().unwrap_or_else(|_| gitdir.to_path_buf());

    Ok(ProjectWorktrees { name, path, items })
}

/// Handle the `wt list --global` command.
pub fn handle_list_global(
    format: crate::OutputFormat,
    show_full: bool,
    config: &WorktrunkConfig,
) -> anyhow::Result<()> {
    // Check that global_worktree_dir is configured
    let global_dir = config.global_worktree_dir_path().ok_or_else(|| {
        anyhow::anyhow!(
            "global-worktree-dir is not configured. Add it to your config:\n\n  \
            [user config path]/config.toml:\n  \
            global-worktree-dir = \"~/worktrees\""
        )
    })?;

    // Check that global_worktree_dir exists
    if !global_dir.exists() {
        bail!(
            "global-worktree-dir does not exist: {}",
            global_dir.display()
        );
    }

    let _ = show_full; // TODO: use for CI status, etc.

    let data = collect_global(&global_dir, config)?;

    match format {
        crate::OutputFormat::Table => {
            render_global_table(&data)?;
        }
        crate::OutputFormat::Json => {
            render_global_json(&data)?;
        }
    }

    Ok(())
}

/// Render the global worktree listing as a table.
fn render_global_table(data: &GlobalListData) -> anyhow::Result<()> {
    use crate::commands::list::collect::TaskKind;
    use crate::commands::list::layout::calculate_layout_with_width;
    use color_print::cformat;
    use worktrunk::styling::{get_terminal_width, info_message};

    if data.projects.is_empty() {
        crate::output::print(info_message("No worktrees found in global directory"))?;
        return Ok(());
    }

    // Skip expensive tasks for global listing (no CI, no branch diff)
    let skip_tasks: std::collections::HashSet<TaskKind> = [
        TaskKind::BranchDiff,
        TaskKind::CiStatus,
        TaskKind::WorkingTreeConflicts,
    ]
    .into_iter()
    .collect();

    // Use the first project's path as main_worktree_path for relative path display
    // (paths will show as absolute since they're from different repos)
    let main_worktree_path = &data.projects[0].path;

    // Collect all items for layout calculation
    let all_items: Vec<ListItem> = data
        .projects
        .iter()
        .flat_map(|p| p.items.iter().cloned())
        .collect();

    // Calculate layout based on all items
    let layout = calculate_layout_with_width(
        &all_items,
        &skip_tasks,
        get_terminal_width(),
        main_worktree_path,
        None, // no URL template for global listing
    );

    // Print header once at the top
    crate::output::table(layout.format_header_line())?;

    for project in &data.projects {
        // Project header in cyan
        crate::output::table(cformat!("<cyan>{}</>", project.name.to_uppercase()))?;

        for item in &project.items {
            crate::output::table(layout.format_list_item_line(item, None))?;
        }
    }

    // Summary line
    crate::output::table("")?;
    let worktree_count = data.worktree_count();
    let project_count = data.project_count();
    let summary = format!(
        "{} {} across {} {}",
        worktree_count,
        if worktree_count == 1 { "worktree" } else { "worktrees" },
        project_count,
        if project_count == 1 { "project" } else { "projects" }
    );
    crate::output::print(info_message(summary))?;

    Ok(())
}

/// Render the global worktree listing as JSON.
fn render_global_json(data: &GlobalListData) -> anyhow::Result<()> {
    use serde::Serialize;

    #[derive(Serialize)]
    struct JsonOutput {
        worktrees: Vec<JsonWorktree>,
        summary: JsonSummary,
    }

    #[derive(Serialize)]
    struct JsonWorktree {
        branch: Option<String>,
        project: String,
        project_path: PathBuf,
        path: PathBuf,
        head_sha: String,
    }

    #[derive(Serialize)]
    struct JsonSummary {
        worktree_count: usize,
        project_count: usize,
    }

    let mut worktrees = Vec::new();
    for project in &data.projects {
        for item in &project.items {
            let path = match &item.kind {
                ItemKind::Worktree(wt_data) => wt_data.path.clone(),
                ItemKind::Branch => continue,
            };
            worktrees.push(JsonWorktree {
                branch: item.branch.clone(),
                project: project.name.clone(),
                project_path: project.path.clone(),
                path,
                head_sha: item.head.clone(),
            });
        }
    }

    let output = JsonOutput {
        worktrees,
        summary: JsonSummary {
            worktree_count: data.worktree_count(),
            project_count: data.project_count(),
        },
    };

    let json = serde_json::to_string_pretty(&output)
        .context("Failed to serialize global list to JSON")?;
    crate::output::data(json)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_parse_git_file() {
        let temp = TempDir::new().unwrap();
        let git_file = temp.path().join(".git");

        // Write a valid .git file
        fs::write(&git_file, "gitdir: /path/to/repo/.git/worktrees/my-branch\n").unwrap();

        let result = parse_git_file(&git_file).unwrap();
        assert_eq!(result, Some(PathBuf::from("/path/to/repo/.git")));
    }

    #[test]
    fn test_parse_git_file_invalid() {
        let temp = TempDir::new().unwrap();
        let git_file = temp.path().join(".git");

        // Write an invalid .git file
        fs::write(&git_file, "not a valid git file").unwrap();

        let result = parse_git_file(&git_file).unwrap();
        assert_eq!(result, None);
    }
}
