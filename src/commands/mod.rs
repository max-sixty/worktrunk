pub(crate) mod branch_deletion;
pub(crate) mod command_approval;
pub(crate) mod command_executor;
pub(crate) mod commit;
pub(crate) mod config;
pub(crate) mod configure_shell;
pub(crate) mod context;
mod for_each;
mod handle_switch;
mod hook_commands;
mod hook_filter;
pub(crate) mod hooks;
pub(crate) mod init;
pub(crate) mod list;
pub(crate) mod merge;
pub(crate) mod process;
pub(crate) mod project_config;
mod relocate;
pub(crate) mod repository_ext;
#[cfg(unix)]
pub(crate) mod select;
pub(crate) mod statusline;
pub(crate) mod step_commands;
pub(crate) mod worktree;

pub(crate) use config::{
    handle_config_create, handle_config_show, handle_hints_clear, handle_hints_get,
    handle_logs_get, handle_state_clear, handle_state_clear_all, handle_state_get,
    handle_state_set, handle_state_show,
};
pub(crate) use configure_shell::{
    handle_configure_shell, handle_show_theme, handle_unconfigure_shell,
};
pub(crate) use for_each::step_for_each;
pub(crate) use handle_switch::{SwitchOptions, handle_switch};
pub(crate) use hook_commands::{add_approvals, clear_approvals, handle_hook_show, run_hook};
pub(crate) use init::{handle_completions, handle_init};
pub(crate) use list::handle_list;
pub(crate) use merge::{MergeOptions, handle_merge};
#[cfg(unix)]
pub(crate) use select::handle_select;
pub(crate) use step_commands::{
    RebaseResult, SquashResult, handle_rebase, handle_squash, step_commit, step_copy_ignored,
    step_relocate, step_show_squash_prompt,
};
pub(crate) use worktree::{
    OperationMode, handle_remove, handle_remove_current, is_worktree_at_expected_path,
    resolve_worktree_arg, worktree_display_name,
};

// Re-export Shell from the canonical location
pub(crate) use worktrunk::shell::Shell;

use color_print::cformat;
use worktrunk::styling::{eprintln, format_with_gutter};

/// Format command execution label with optional command name.
///
/// Examples:
/// - `format_command_label("post-create", Some("install"))` → `"Running post-create install"` (with bold)
/// - `format_command_label("post-create", None)` → `"Running post-create"`
pub(crate) fn format_command_label(command_type: &str, name: Option<&str>) -> String {
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
pub(crate) fn show_diffstat(repo: &worktrunk::git::Repository, range: &str) -> anyhow::Result<()> {
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
        eprintln!("{}", format_with_gutter(&diff_stat, None));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_command_label_with_name() {
        let result = format_command_label("post-create", Some("install"));
        assert!(result.contains("Running"));
        assert!(result.contains("post-create"));
        assert!(result.contains("install"));
    }

    #[test]
    fn test_format_command_label_without_name() {
        let result = format_command_label("pre-merge", None);
        assert_eq!(result, "Running pre-merge");
    }

    #[test]
    fn test_format_command_label_various_types() {
        let result = format_command_label("post-start", Some("build"));
        assert!(result.contains("post-start"));
        assert!(result.contains("build"));

        let result = format_command_label("pre-commit", None);
        assert!(result.contains("pre-commit"));
        assert!(!result.contains("None"));
    }
}
