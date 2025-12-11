// Note: Some tests require Unix-specific features (PTY, shell integration).
// Those are gated at the individual file level with #[cfg(unix)] or
// #[cfg(all(unix, feature = "shell-integration-tests"))].

// TODO: Re-enable Windows tests once snapshot path normalization is implemented.
// Issue: Windows paths use backslashes which differ from Unix snapshots.
// Fix: Either normalize paths in test output or create Windows-specific snapshots.
// Modules below marked with #[cfg(not(windows))] fail due to path differences.

// column_alignment merged into spacing_edge_cases
pub mod approval_pty;

pub mod approval_save;
pub mod approval_ui;
pub mod approvals;
pub mod bare_repository;
pub mod column_alignment_verification;
pub mod completion;
pub mod completion_validation;
pub mod config_cache;
pub mod config_init;
pub mod config_show;
pub mod config_show_theme;
pub mod config_var;
pub mod configure_shell;
pub mod default_branch;
#[cfg(not(windows))]
pub mod directives;
pub mod e2e_shell;
pub mod e2e_shell_post_start;
pub mod for_each;
pub mod git_error_display;
pub mod help;
pub mod hook_show;
pub mod init;
#[cfg(not(windows))]
pub mod internal_flag;
#[cfg(not(windows))]
pub mod list;
#[cfg(not(windows))]
pub mod list_column_alignment;
#[cfg(not(windows))]
pub mod list_config;
pub mod list_progressive;
#[cfg(not(windows))]
pub mod merge;
pub mod output_system_guard;
pub mod post_start_commands;
#[cfg(not(windows))]
pub mod push;
pub mod readme_sync;
pub mod remove;
#[cfg(not(windows))]
pub mod security;
pub mod select;
pub mod shell_wrapper;
#[cfg(not(windows))]
pub mod spacing_edge_cases;
#[cfg(not(windows))]
pub mod statusline;
pub mod switch;
#[cfg(not(windows))]
pub mod user_hooks;
