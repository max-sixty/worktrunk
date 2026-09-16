#![cfg(all(unix, feature = "shell-integration-tests"))]
//! PTY-based tests for `wt config update` interactive prompt.
//!
//! Tests the accept/decline flow in a real TTY environment.

use crate::common::pty::{
    build_pty_command, exec_cmd_in_pty_prompted, exec_cmd_in_pty_prompted_with,
};
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
pre-start = "ln -sf {{ repo_root }}/node_modules"

[list]
json-schema = 1
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
pre-start = "ln -sf {{ repo_root }}/node_modules"

[list]
json-schema = 1
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

/// The preview authorizes the migration of the content it was computed from.
/// An edit that completes while the prompt is open must not be replaced by
/// that earlier snapshot.
#[rstest]
fn test_config_update_rejects_preview_stale_at_apply(repo: TestRepo) {
    let config_path = repo.test_config_path().to_path_buf();
    let original = r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
"#;
    fs::write(&config_path, original).unwrap();

    let edited = format!("{original}\n# preserve this edit\n");
    let callback_path = config_path.clone();
    let callback_content = edited.clone();
    let cmd = build_pty_command(
        wt_bin().to_str().unwrap(),
        &["config", "update"],
        repo.root_path(),
        &repo.test_env_vars(),
        None,
    );
    let (output, exit_code) = exec_cmd_in_pty_prompted_with(cmd, &["y\n"], "[y/N", move |_| {
        fs::write(&callback_path, &callback_content).unwrap();
    });

    assert_ne!(
        exit_code, 0,
        "applying a preview of superseded content should fail:\n{output}"
    );
    assert!(
        output.contains("since the preview"),
        "failure should name the superseded preview:\n{output}"
    );
    assert_eq!(
        fs::read_to_string(&config_path).unwrap(),
        edited,
        "the edit made while the prompt was open should survive"
    );
}

/// Removing the config while the prompt is open is the same staleness, at its
/// limit: the command fails rather than recreating the file from the preview.
#[rstest]
fn test_config_update_rejects_config_removed_at_apply(repo: TestRepo) {
    let config_path = repo.test_config_path().to_path_buf();
    fs::write(
        &config_path,
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
"#,
    )
    .unwrap();

    let callback_path = config_path.clone();
    let cmd = build_pty_command(
        wt_bin().to_str().unwrap(),
        &["config", "update"],
        repo.root_path(),
        &repo.test_env_vars(),
        None,
    );
    let (output, exit_code) = exec_cmd_in_pty_prompted_with(cmd, &["y\n"], "[y/N", move |_| {
        fs::remove_file(&callback_path).unwrap();
    });

    assert_ne!(
        exit_code, 0,
        "applying a preview of a removed config should fail:\n{output}"
    );
    assert!(
        output.contains("Failed to re-read user config"),
        "failure should name the unreadable config:\n{output}"
    );
    assert!(
        !config_path.exists(),
        "the removed config should not be recreated from the preview"
    );
}

/// Execute `wt config update --output=<destination>` in a PTY, answering the
/// overwrite prompt.
fn exec_config_update_output_in_pty(
    repo: &TestRepo,
    destination: &str,
    input: &str,
) -> (String, i32) {
    let output_arg = format!("--output={destination}");
    let cmd = build_pty_command(
        wt_bin().to_str().unwrap(),
        &["config", "update", &output_arg],
        repo.root_path(),
        &repo.test_env_vars(),
        None,
    );
    exec_cmd_in_pty_prompted(cmd, &[input], "[y/N")
}

/// An existing `--output` destination is overwritten only once the prompt is
/// accepted. The prompt starts flush when nothing precedes it; the approvals
/// warning is narration above it, so a blank line separates the two.
#[rstest]
fn test_config_update_output_overwrite_prompt(repo: TestRepo) {
    let migrated = "worktree-path = \"../{{ repo }}.{{ branch }}\"\n";
    let deprecated = "worktree-path = \"../{{ main_worktree }}.{{ branch }}\"\n";
    fs::write(
        repo.test_config_path(),
        format!(
            r#"{deprecated}
[projects."github.com/user/repo"]
approved-commands = ["npm test"]
"#
        ),
    )
    .unwrap();
    let destination = repo.root_path().join("migrated.toml");
    fs::write(&destination, "important user data\n").unwrap();

    let (declined, exit_code) = exec_config_update_output_in_pty(&repo, "migrated.toml", "n\n");
    assert_eq!(exit_code, 0);
    assert_eq!(
        fs::read_to_string(&destination).unwrap(),
        "important user data\n"
    );

    fs::write(repo.test_config_path(), deprecated).unwrap();
    let (accepted, exit_code) = exec_config_update_output_in_pty(&repo, "migrated.toml", "y\n");
    assert_eq!(exit_code, 0);
    assert_eq!(fs::read_to_string(&destination).unwrap(), migrated);

    config_update_pty_settings(&repo).bind(|| {
        assert_snapshot!("config_update_output_overwrite_declined", &declined);
        assert_snapshot!("config_update_output_overwrite_accepted", &accepted);
    });
}
