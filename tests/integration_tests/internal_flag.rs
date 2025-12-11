//! Tests for --internal flag behavior
//!
//! Verifies that:
//! 1. Commands that emit directives work correctly with --internal (already tested in directives.rs)
//! 2. Commands that DON'T emit directives work correctly with --internal
//! 3. The --internal flag can be safely passed to all commands

use crate::common::{
    TestRepo, set_temp_home_env, setup_snapshot_settings, setup_snapshot_settings_with_home,
    wt_command,
};
use insta_cmd::assert_cmd_snapshot;
use std::fs;
use tempfile::TempDir;

/// Test that `list` command works with --internal flag
///
/// The list command outputs tables through stderr in directive mode for progressive
/// streaming. This bypasses the shell wrapper's NUL-delimited parsing on stdout,
/// allowing real-time display without buffering.
///
/// Expected behavior:
/// - stdout: empty (no directives emitted by list command)
/// - stderr: complete table output with ANSI formatting
#[test]
fn test_list_with_internal_flag() {
    let repo = TestRepo::new();

    let settings = setup_snapshot_settings(&repo);

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("--internal")
            .arg("list")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test that `config show` command works with --internal flag
///
/// Config show doesn't emit directives, but should work fine with --internal.
#[test]
#[cfg_attr(windows, ignore = "mock gh/glab batch files not found on Windows")]
fn test_config_show_with_internal_flag() {
    let mut repo = TestRepo::new();
    let temp_home = TempDir::new().unwrap();

    // Setup mock gh/glab for deterministic BINARIES output
    repo.setup_mock_ci_tools_unauthenticated();

    // Create fake global config at XDG path
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
"#,
    )
    .unwrap();

    let settings = setup_snapshot_settings_with_home(&repo, &temp_home);
    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        repo.configure_mock_commands(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.arg("--internal")
            .arg("config")
            .arg("show")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}
