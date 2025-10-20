use crate::common::TestRepo;
use insta::Settings;
use insta_cmd::{assert_cmd_snapshot, get_cargo_bin};
use std::process::Command;

/// Helper to create snapshot with normalized paths
fn snapshot_push(test_name: &str, repo: &TestRepo, args: &[&str], cwd: Option<&std::path::Path>) {
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    // Normalize paths
    settings.add_filter(repo.root_path().to_str().unwrap(), "[REPO]");
    for (name, path) in &repo.worktrees {
        settings.add_filter(
            path.to_str().unwrap(),
            format!("[WORKTREE_{}]", name.to_uppercase().replace('-', "_")),
        );
    }

    // Normalize git SHAs
    settings.add_filter(r"\b[0-9a-f]{7,40}\b", "[SHA]");
    settings.add_filter(r"\\", "/");

    settings.bind(|| {
        let mut cmd = Command::new(get_cargo_bin("wt"));
        repo.clean_cli_env(&mut cmd);
        cmd.arg("push")
            .args(args)
            .current_dir(cwd.unwrap_or(repo.root_path()));

        assert_cmd_snapshot!(test_name, cmd);
    });
}

#[test]
fn test_push_fast_forward() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create a worktree for main (checking out existing branch)
    let main_wt = repo.root_path().parent().unwrap().join("test-repo.main-wt");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["worktree", "add", main_wt.to_str().unwrap(), "main"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to add worktree");

    // Make a commit in a feature worktree
    let feature_wt = repo.add_worktree("feature", "feature");
    std::fs::write(feature_wt.join("test.txt"), "test content").expect("Failed to write file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "test.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add test file"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Push from feature to main
    snapshot_push("push_fast_forward", &repo, &["main"], Some(&feature_wt));
}

#[test]
fn test_push_not_fast_forward() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create commits in both worktrees
    // The repo root is already the main worktree
    std::fs::write(repo.root_path().join("main.txt"), "main content")
        .expect("Failed to write file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "main.txt"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to add file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add main file"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to commit");

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

    // Try to push from feature to main (should fail - not fast-forward)
    snapshot_push("push_not_fast_forward", &repo, &["main"], Some(&feature_wt));
}

#[test]
fn test_push_to_default_branch() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    let _main_wt = repo.root_path().parent().unwrap().join("test-repo.main-wt");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["worktree", "add", _main_wt.to_str().unwrap(), "main"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to add worktree");

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

    // Push without specifying target (should use default branch)
    snapshot_push("push_to_default", &repo, &[], Some(&feature_wt));
}

#[test]
fn test_push_with_dirty_target() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Make main worktree (repo root) dirty with a conflicting file
    std::fs::write(repo.root_path().join("conflict.txt"), "old content")
        .expect("Failed to write file");

    let feature_wt = repo.add_worktree("feature", "feature");
    std::fs::write(feature_wt.join("conflict.txt"), "new content").expect("Failed to write file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "conflict.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add conflict file"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Try to push (should fail due to conflicting changes)
    snapshot_push("push_dirty_target", &repo, &["main"], Some(&feature_wt));
}
