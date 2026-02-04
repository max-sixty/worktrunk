//! List command for jj workspaces.
//!
//! Displays all workspaces in the repository with their status.

use anyhow::Context;
use color_print::cformat;
use worktrunk::jj::Repository;

/// Handle the list command for jj workspaces.
pub fn handle_list_jj(format: crate::OutputFormat) -> anyhow::Result<()> {
    let repo = Repository::current()?;
    let workspaces = repo.list_workspaces()?;

    match format {
        crate::OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&workspaces)
                .context("Failed to serialize to JSON")?;
            println!("{}", json);
        }
        crate::OutputFormat::Table | crate::OutputFormat::ClaudeCode => {
            print_workspace_table(&repo, &workspaces)?;
        }
    }

    Ok(())
}

fn print_workspace_table(
    repo: &Repository,
    workspaces: &[worktrunk::jj::WorkspaceInfo],
) -> anyhow::Result<()> {
    use std::io::Write;

    // Calculate column widths
    let max_name_len = workspaces
        .iter()
        .map(|ws| ws.name.len())
        .max()
        .unwrap_or(7)
        .max(7); // "Workspace" header

    let max_bookmark_len = workspaces
        .iter()
        .filter_map(|ws| ws.bookmark.as_ref())
        .map(|b| b.len())
        .max()
        .unwrap_or(8)
        .max(8); // "Bookmark" header

    let max_path_len = workspaces
        .iter()
        .map(|ws| ws.dir_name().len())
        .max()
        .unwrap_or(4)
        .max(4); // "Path" header

    // Print header
    let header_style = anstyle::Style::new().bold();
    let dim_style = anstyle::Style::new().dimmed();

    print!(
        "{header_style}{:max_name_len$}  {:max_bookmark_len$}  {:max_path_len$}  Status{header_style:#}\n",
        "Workspace", "Bookmark", "Path"
    );

    // Print separator
    println!(
        "{dim_style}{}{dim_style:#}",
        "-".repeat(max_name_len + max_bookmark_len + max_path_len + 12)
    );

    // Print each workspace
    for ws in workspaces {
        let name_style = if ws.is_current {
            anstyle::Style::new()
                .bold()
                .fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::Cyan)))
        } else {
            anstyle::Style::new()
        };

        let bookmark = ws.bookmark.as_deref().unwrap_or("-");
        let path = ws.dir_name();

        // Get workspace status
        let status = get_workspace_status(repo, ws);
        let status_style = if status.contains("dirty") {
            anstyle::Style::new().fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::Yellow)))
        } else if status.contains("conflict") {
            anstyle::Style::new().fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::Red)))
        } else {
            dim_style
        };

        // Current workspace indicator
        let indicator = if ws.is_current { "*" } else { " " };

        println!(
            "{indicator}{name_style}{:max_name_len$}{name_style:#}  {dim_style}{:max_bookmark_len$}{dim_style:#}  {:max_path_len$}  {status_style}{}{status_style:#}",
            ws.name, bookmark, path, status
        );
    }

    // Print summary
    let dim = anstyle::Style::new().dimmed();
    let workspace_count = workspaces.len();
    let plural = if workspace_count == 1 { "" } else { "s" };
    println!(
        "\n{dim}Showing {} workspace{}{dim:#}",
        workspace_count, plural
    );

    std::io::stdout().flush()?;
    Ok(())
}

fn get_workspace_status(repo: &Repository, ws: &worktrunk::jj::WorkspaceInfo) -> String {
    let working_copy = repo.workspace_at(&ws.path);

    // Check for conflicts
    if let Ok(true) = working_copy.has_conflicts() {
        return "conflicts".to_string();
    }

    // Check for uncommitted changes
    if let Ok(true) = working_copy.is_dirty() {
        return "dirty".to_string();
    }

    "clean".to_string()
}
