// Note: Some tests require Unix-specific features (PTY, shell integration).
// Those are gated at the individual test or file level with #[cfg(unix)],
// #[cfg_attr(windows, ignore)], or #[cfg(all(unix, feature = "shell-integration-tests"))].
//
// Windows path differences are handled by snapshot filters in setup_snapshot_settings().

// column_alignment merged into spacing_edge_cases
pub mod approval_pty;

pub mod approval_save;
pub mod approval_ui;
pub mod approvals;
pub mod bare_repository;
pub mod ci_status;
pub mod column_alignment_verification;
pub mod completion;
pub mod completion_validation;
pub mod config_init;
pub mod config_show;
pub mod config_show_theme;
pub mod config_state;
pub mod configure_shell;
pub mod default_branch;
pub mod directives;
pub mod e2e_shell;
pub mod e2e_shell_post_start;
pub mod for_each;
pub mod git_error_display;
pub mod help;
pub mod hook_show;
pub mod init;
pub mod internal_flag;
pub mod list;
pub mod list_column_alignment;
pub mod list_config;
pub mod list_progressive;
pub mod merge;
pub mod output_system_guard;
pub mod post_start_commands;
pub mod push;
pub mod readme_sync;
pub mod remove;
pub mod repository;
pub mod security;
pub mod select;
pub mod shell_wrapper;
pub mod spacing_edge_cases;
pub mod statusline;
pub mod switch;
pub mod user_hooks;
