//! Tests for --internal flag behavior
//!
//! Verifies that:
//! 1. Commands that emit directives work correctly with --internal (already tested in directives.rs)
//! 2. Commands that DON'T emit directives work correctly with --internal
//! 3. The --internal flag can be safely passed to all commands

use crate::common::{TestRepo, set_temp_home_env, wt_command};
use insta::Settings;
use insta_cmd::assert_cmd_snapshot;
use std::fs;
use tempfile::TempDir;

/// Test that `list` command works with --internal flag
///
/// The list command doesn't emit directives, so output remains newline-terminated.
/// The --internal flag can be safely passed without changing behavior.
#[test]
fn test_list_with_internal_flag() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    // Normalize temp directory paths
    settings.add_filter(r"/private/var/folders/[^\s]+/test-repo", "[REPO]");
    settings.add_filter(r"/tmp/\.tmp[^\s]+/test-repo", "[REPO]");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("--internal")
            .arg("list")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}

/// Test that `config list` command works with --internal flag
///
/// Config list doesn't emit directives, but should work fine with --internal.
#[test]
fn test_config_list_with_internal_flag() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");
    let temp_home = TempDir::new().unwrap();

    // Create fake global config at XDG path
    let global_config_dir = temp_home.path().join(".config").join("worktrunk");
    fs::create_dir_all(&global_config_dir).unwrap();
    fs::write(
        global_config_dir.join("config.toml"),
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
"#,
    )
    .unwrap();

    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    // Filter out the actual config paths to use placeholders
    settings.add_filter(repo.root_path().to_str().unwrap(), "[REPO]");
    settings.add_filter(&temp_home.path().to_string_lossy(), "[TEMP_HOME]");
    settings.add_filter(r"\\", "/");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        set_temp_home_env(&mut cmd, temp_home.path());
        cmd.arg("--internal")
            .arg("config")
            .arg("list")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd);
    });
}
