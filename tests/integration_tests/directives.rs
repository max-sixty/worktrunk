use crate::common::TestRepo;
use insta::Settings;
use insta_cmd::{assert_cmd_snapshot, get_cargo_bin};
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
        let mut cmd = Command::new(get_cargo_bin("wt"));
        repo.clean_cli_env(&mut cmd);
        cmd.arg("--internal")
            .arg("switch")
            .arg("my-feature")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!("switch_internal_directive", cmd);
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
        let mut cmd = Command::new(get_cargo_bin("wt"));
        repo.clean_cli_env(&mut cmd);
        cmd.arg("switch")
            .arg("my-feature")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!("switch_without_internal", cmd);
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
        let mut cmd = Command::new(get_cargo_bin("wt"));
        repo.clean_cli_env(&mut cmd);
        cmd.arg("--internal")
            .arg("remove")
            .current_dir(repo.root_path());

        assert_cmd_snapshot!("remove_internal_directive", cmd);
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
        let mut cmd = Command::new(get_cargo_bin("wt"));
        repo.clean_cli_env(&mut cmd);
        cmd.arg("remove").current_dir(repo.root_path());

        assert_cmd_snapshot!("remove_without_internal", cmd);
    });
}

/// Test merge command with internal flag and --keep
#[test]
fn test_merge_internal_keep() {
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
        .expect("Failed to add worktree");

    // Create a feature worktree and make a commit
    let feature_wt = repo.add_worktree("feature", "feature");
    std::fs::write(feature_wt.join("feature.txt"), "feature content")
        .expect("Failed to write file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "feature.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add feature file"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");
    // Normalize SHA in output
    settings.add_filter(r"@ [a-f0-9]{7}", "@ [SHA]");

    settings.bind(|| {
        let mut cmd = Command::new(get_cargo_bin("wt"));
        repo.clean_cli_env(&mut cmd);
        cmd.arg("--internal")
            .arg("merge")
            .arg("main")
            .arg("--keep")
            .current_dir(&feature_wt);

        assert_cmd_snapshot!("merge_internal_keep", cmd);
    });
}
