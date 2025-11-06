pub mod beta;
pub mod command_approval;
mod command_executor;
pub mod completion;
pub mod config;
pub mod configure_shell;
pub mod init;
pub mod list;
pub mod merge;
pub mod process;
#[cfg(unix)]
pub mod select;
pub mod worktree;

pub use beta::{
    handle_beta_ask_approvals, handle_beta_commit, handle_beta_push, handle_beta_rebase,
    handle_beta_run_hook, handle_beta_squash,
};
pub use completion::{handle_complete, handle_completion};
pub use config::{
    handle_config_help, handle_config_init, handle_config_list, handle_config_refresh_cache,
};
pub use configure_shell::{ConfigAction, handle_configure_shell};
pub use init::handle_init;
pub use list::handle_list;
pub use merge::handle_merge;
#[cfg(unix)]
pub use select::handle_select;
pub use worktree::{handle_remove, handle_switch};

// Re-export Shell from the canonical location
pub use worktrunk::shell::Shell;

/// Format command execution label with optional command name.
///
/// Examples:
/// - `format_command_label("post-create", Some("install"))` → `"Running post-create: install"` (with bold)
/// - `format_command_label("post-create", None)` → `"Running post-create"`
pub fn format_command_label(command_type: &str, name: Option<&str>) -> String {
    use worktrunk::styling::AnstyleStyle;

    match name {
        Some(name) => {
            let bold = AnstyleStyle::new().bold();
            format!("Running {command_type}: {bold}{name}{bold:#}")
        }
        None => format!("Running {command_type}"),
    }
}
