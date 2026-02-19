#![cfg(all(unix, feature = "shell-integration-tests"))]
//! PTY-based tests for `wt config update` interactive prompt.
//!
//! Tests the accept/decline flow in a real TTY environment.

use crate::common::pty::{build_pty_command, exec_cmd_in_pty_prompted};
use crate::common::{TestRepo, add_pty_filters, repo, setup_snapshot_settings, wt_bin};
use insta::assert_snapshot;
use rstest::rstest;
use std::fs;

/// Execute `wt config update` in a PTY, waiting for the confirmation prompt.
fn exec_config_update_in_pty(
    repo: &TestRepo,
    env_vars: &[(String, String)],
    input: &str,
) -> (String, i32) {
    let cmd = build_pty_command(
        wt_bin().to_str().unwrap(),
        &["config", "update"],
        repo.root_path(),
        env_vars,
        None,
    );
    exec_cmd_in_pty_prompted(cmd, &[input], "[y/N")
}

fn config_update_pty_settings(repo: &TestRepo) -> insta::Settings {
    let mut settings = setup_snapshot_settings(repo);
    add_pty_filters(&mut settings);
    settings
}

#[rstest]
fn test_config_update_prompt_accept(repo: TestRepo) {
    let config_path = repo.test_config_path();
    fs::write(
        config_path,
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
post-create = "ln -sf {{ repo_root }}/node_modules"
"#,
    )
    .unwrap();

    let env_vars = repo.test_env_vars();
    let (output, exit_code) = exec_config_update_in_pty(&repo, &env_vars, "y\n");

    assert_eq!(exit_code, 0);
    config_update_pty_settings(&repo).bind(|| {
        assert_snapshot!("config_update_prompt_accept", &output);
    });

    // Verify config was actually updated
    let updated = fs::read_to_string(config_path).unwrap();
    assert!(updated.contains("{{ repo }}"));
    assert!(updated.contains("{{ repo_path }}"));
}

#[rstest]
fn test_config_update_prompt_decline(repo: TestRepo) {
    let config_path = repo.test_config_path();
    let original_content = r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
post-create = "ln -sf {{ repo_root }}/node_modules"
"#;
    fs::write(config_path, original_content).unwrap();

    let env_vars = repo.test_env_vars();
    let (output, exit_code) = exec_config_update_in_pty(&repo, &env_vars, "n\n");

    assert_eq!(exit_code, 0);
    config_update_pty_settings(&repo).bind(|| {
        assert_snapshot!("config_update_prompt_decline", &output);
    });

    // Verify config was NOT changed
    let content = fs::read_to_string(config_path).unwrap();
    assert_eq!(content, original_content, "Config should be unchanged");
}
