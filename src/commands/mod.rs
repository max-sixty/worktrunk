pub mod command_approval;
pub mod command_executor;
pub mod commit;
pub mod config;
pub mod configure_shell;
pub mod context;
mod for_each;
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

pub use command_approval::approve_hooks;
pub use config::{
    handle_config_create, handle_config_show, handle_state_clear, handle_state_clear_all,
    handle_state_get, handle_state_set, handle_state_show,
};
pub use configure_shell::{
    ConfigAction, handle_configure_shell, handle_show_theme, handle_unconfigure_shell,
};
pub use for_each::step_for_each;
pub use init::handle_init;
pub use list::handle_list;
pub use merge::{MergeOptions, execute_pre_remove_commands, handle_merge};
#[cfg(unix)]
pub use select::handle_select;
pub use standalone::{
    RebaseResult, SquashResult, add_approvals, clear_approvals, handle_hook_show, handle_rebase,
    handle_squash, run_hook, step_commit, step_show_squash_prompt,
};
pub use worktree::{
    compute_worktree_path, handle_remove, handle_remove_by_path, handle_remove_current,
    handle_switch, is_worktree_at_expected_path, resolve_worktree_arg, worktree_display_name,
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
