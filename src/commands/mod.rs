pub mod command_approval;
pub mod command_executor;
pub mod commit;
pub mod config;
pub mod configure_shell;
pub mod context;
mod hooks;
pub mod init;
pub mod list;
pub mod merge;
pub mod process;
pub mod project_config;
pub mod repository_ext;
#[cfg(unix)]
pub mod select;
pub mod standalone;
pub mod statusline;
pub mod worktree;

pub use config::{
    handle_cache_clear, handle_cache_refresh, handle_cache_show, handle_config_create,
    handle_config_show, handle_var_clear, handle_var_get, handle_var_set,
};
pub use configure_shell::{ConfigAction, handle_configure_shell, handle_unconfigure_shell};
pub use init::handle_init;
pub use list::handle_list;
pub use merge::{execute_pre_remove_commands, handle_merge};
#[cfg(unix)]
pub use select::handle_select;
pub use standalone::{
    RebaseResult, SquashResult, handle_rebase, handle_squash, handle_standalone_add_approvals,
    handle_standalone_clear_approvals, handle_standalone_commit, handle_standalone_run_hook,
};
pub use worktree::{
    handle_remove, handle_remove_by_path, handle_remove_current, handle_switch,
    resolve_worktree_path_first,
};

// Re-export Shell from the canonical location
pub use worktrunk::shell::Shell;

/// Format command execution label with optional command name.
///
/// Examples:
/// - `format_command_label("post-create", Some("install"))` → `"Running post-create install"` (with bold)
/// - `format_command_label("post-create", None)` → `"Running post-create"`
pub fn format_command_label(command_type: &str, name: Option<&str>) -> String {
    use color_print::cformat;

    match name {
        Some(name) => cformat!("Running {command_type} <bold>{name}</>"),
        None => format!("Running {command_type}"),
    }
}

/// Show detailed diffstat for a given commit range.
///
/// Displays the diff statistics (file changes, insertions, deletions) in a gutter format.
/// Used after commit/squash to show what was included in the commit.
///
/// # Arguments
/// * `repo` - The repository to query
/// * `range` - The commit range to diff (e.g., "HEAD~1..HEAD" or "main..HEAD")
pub fn show_diffstat(repo: &worktrunk::git::Repository, range: &str) -> anyhow::Result<()> {
    use worktrunk::styling::format_with_gutter;

    let term_width = crate::display::get_terminal_width();
    let stat_width = term_width.saturating_sub(worktrunk::styling::GUTTER_OVERHEAD);
    let diff_stat = repo
        .run_command(&[
            "diff",
            "--color=always",
            "--stat",
            &format!("--stat-width={}", stat_width),
            range,
        ])?
        .trim_end()
        .to_string();

    if !diff_stat.is_empty() {
        crate::output::gutter(format_with_gutter(&diff_stat, "", None))?;
    }

    Ok(())
}
