use crate::common::{TestRepo, setup_snapshot_settings, wt_command};
use insta::Settings;
use insta_cmd::assert_cmd_snapshot;
use std::process::Command;

/// Test the directive protocol for switch command
#[test]
fn test_switch_internal_directive() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    // Normalize the directive path output
    settings.add_filter(r"__WORKTRUNK_CD__[^\n]+", "__WORKTRUNK_CD__[PATH]");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("--internal")
            .arg("switch")
            .arg("my-feature")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @r"
        success: false
        exit_code: 1
        ----- stdout -----

        ----- stderr -----
        ❌ [31mFailed to create worktree for [1m[31mmy-feature[0m[0m
        [107m [0m  fatal: invalid reference: my-feature
        ");
    });
}

/// Test switch without internal flag (should show help message)
#[test]
fn test_switch_without_internal() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("switch")
            .arg("my-feature")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @r"
        success: false
        exit_code: 1
        ----- stdout -----
        ❌ [31mFailed to create worktree for [1m[31mmy-feature[0m[0m
        [107m [0m  fatal: invalid reference: my-feature


        ----- stderr -----
        ");
    });
}

/// Test remove command with internal flag
#[test]
fn test_remove_internal_directive() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    // Normalize the directive path output
    settings.add_filter(r"__WORKTRUNK_CD__[^\n]+", "__WORKTRUNK_CD__[PATH]");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("--internal")
            .arg("remove")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----
        __WORKTRUNK_CD__[PATH]
        ----- stderr -----
        🔄 [36mRemoving [1m[36mmain[0m[36m worktree & branch in background[0m
        ");
    });
}

/// Test remove without internal flag
#[test]
fn test_remove_without_internal() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("remove").current_dir(repo.root_path());

        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----
        🔄 [36mRemoving [1m[36mmain[0m[36m worktree & branch in background[0m

        ----- stderr -----
        ");
    });
}

/// Test merge command with internal flag and --no-remove
#[test]
fn test_merge_internal_no_remove() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create a worktree for main
    let main_wt = repo.root_path().parent().unwrap().join("test-repo.main-wt");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["worktree", "add", main_wt.to_str().unwrap(), "main"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    // Create a feature worktree and make a commit
    let feature_wt = repo.add_worktree("feature", "feature");
    std::fs::write(feature_wt.join("feature.txt"), "feature content").unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "feature.txt"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add feature file"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    let mut settings = setup_snapshot_settings(&repo);
    // Normalize SHA in output
    settings.add_filter(r"@ [a-f0-9]{7}", "@ [SHA]");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("--internal")
            .arg("merge")
            .arg("main")
            .arg("--no-remove")
            .current_dir(&feature_wt);

        // Note: Using file snapshot instead of inline because multiline inline snapshots
        // don't work well with NUL bytes (\0) in the output
        assert_cmd_snapshot!(cmd);
    });
}

/// Test merge command with internal flag (removes worktree, emits __WORKTRUNK_CD__)
/// This test verifies that the directive protocol is correctly formatted with NUL terminators
#[test]
fn test_merge_internal_remove() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create a worktree for main
    let main_wt = repo.root_path().parent().unwrap().join("test-repo.main-wt");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["worktree", "add", main_wt.to_str().unwrap(), "main"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    // Create a feature worktree and make a commit
    let feature_wt = repo.add_worktree("feature", "feature");
    std::fs::write(feature_wt.join("feature.txt"), "feature content").unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "feature.txt"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add feature file"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    let mut settings = setup_snapshot_settings(&repo);
    // Normalize SHA and path in output
    settings.add_filter(r"@ [a-f0-9]{7}", "@ [SHA]");
    settings.add_filter(r"__WORKTRUNK_CD__[^\x00]+", "__WORKTRUNK_CD__[PATH]");

    settings.bind(|| {
        let mut cmd = wt_command();
        repo.clean_cli_env(&mut cmd);
        cmd.arg("--internal")
            .arg("merge")
            .arg("main")
            .current_dir(&feature_wt);

        // Note: Using file snapshot instead of inline because multiline inline snapshots
        // don't work well with NUL bytes (\0) in the output
        assert_cmd_snapshot!(cmd);
    });
}
