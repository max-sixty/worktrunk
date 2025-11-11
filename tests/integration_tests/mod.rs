// Integration tests are Unix-only as they test bash/fish/zsh shell integration
// and rely on Unix-specific TTY/stdin/stdout behavior
#![cfg(unix)]

// column_alignment merged into spacing_edge_cases
pub mod approval_save;
pub mod approval_ui;
pub mod bare_repository;
pub mod column_alignment_verification;
pub mod completion;
pub mod completion_validation;
pub mod config_init;
pub mod config_list;
pub mod configure_shell;
pub mod default_branch;
pub mod directives;
pub mod e2e_shell;
pub mod e2e_shell_post_start;
pub mod init;
pub mod internal_flag;
pub mod list;
pub mod merge;
pub mod post_start_commands;
pub mod push;
pub mod remove;
pub mod security;
pub mod shell_wrapper;
pub mod spacing_edge_cases;
pub mod switch;
