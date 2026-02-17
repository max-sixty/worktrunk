pub(crate) mod branch_deletion;
pub(crate) mod command_approval;
pub(crate) mod command_executor;
pub(crate) mod commit;
pub(crate) mod config;
pub(crate) mod configure_shell;
pub(crate) mod context;
mod for_each;
mod handle_merge_jj;
pub(crate) mod handle_remove_jj;
pub(crate) mod handle_step_jj;
mod handle_switch;
mod handle_switch_jj;
mod hook_commands;
mod hook_filter;
pub(crate) mod hooks;
pub(crate) mod init;
pub(crate) mod list;
pub(crate) mod merge;
pub(crate) mod process;
pub(crate) mod project_config;
mod relocate;
mod remove_command;
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
pub(crate) use remove_command::{RemoveOptions, handle_remove_command};
#[cfg(unix)]
pub(crate) use select::handle_select;
pub(crate) use step_commands::{
    RebaseResult, SquashResult, handle_rebase, handle_squash, step_commit, step_copy_ignored,
    step_push, step_relocate, step_show_squash_prompt,
};
pub(crate) use worktree::{is_worktree_at_expected_path, worktree_display_name};

// Re-export Shell from the canonical location
pub(crate) use worktrunk::shell::Shell;

use color_print::cformat;
use worktrunk::git::Repository;
use worktrunk::workspace::Workspace;

/// Downcast a workspace to `Repository`, or error for jj repositories.
///
/// Replaces the old `require_git()` + `Repository::current()` two-step pattern.
/// Returns a reference to the `Repository` if this is a git workspace,
/// or a clear error for jj users.
pub(crate) fn require_git_workspace<'a>(
    workspace: &'a dyn Workspace,
    command: &str,
) -> anyhow::Result<&'a Repository> {
    workspace
        .as_any()
        .downcast_ref::<Repository>()
        .ok_or_else(|| anyhow::anyhow!("`wt {command}` is not yet supported for jj repositories"))
}

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
