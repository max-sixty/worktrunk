use crate::common::{TestRepo, make_snapshot_cmd_with_global_flags, setup_snapshot_settings};
use insta_cmd::assert_cmd_snapshot;
use std::process::Command;

/// Helper to create snapshot with normalized paths
fn snapshot_remove(test_name: &str, repo: &TestRepo, args: &[&str], cwd: Option<&std::path::Path>) {
    snapshot_remove_with_global_flags(test_name, repo, args, cwd, &[]);
}

/// Helper to create snapshot with global flags (e.g., --internal)
fn snapshot_remove_with_global_flags(
    test_name: &str,
    repo: &TestRepo,
    args: &[&str],
    cwd: Option<&std::path::Path>,
    global_flags: &[&str],
) {
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd_with_global_flags(repo, "remove", args, cwd, global_flags);
        assert_cmd_snapshot!(test_name, cmd);
    });
}

#[test]
fn test_remove_already_on_default() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Already on main branch
    snapshot_remove("remove_already_on_default", &repo, &[], None);
}

#[test]
fn test_remove_switch_to_default() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create and switch to a feature branch in the main repo
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["switch", "-c", "feature"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to create branch");

    snapshot_remove("remove_switch_to_default", &repo, &[], None);
}

#[test]
fn test_remove_from_worktree() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    let worktree_path = repo.add_worktree("feature-wt", "feature-wt");

    // Run remove from within the worktree
    snapshot_remove("remove_from_worktree", &repo, &[], Some(&worktree_path));
}

#[test]
fn test_remove_internal_mode() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    let worktree_path = repo.add_worktree("feature-internal", "feature-internal");

    snapshot_remove_with_global_flags(
        "remove_internal_mode",
        &repo,
        &[],
        Some(&worktree_path),
        &["--internal"],
    );
}

#[test]
fn test_remove_dirty_working_tree() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create a dirty file
    std::fs::write(repo.root_path().join("dirty.txt"), "uncommitted changes")
        .expect("Failed to create file");

    snapshot_remove("remove_dirty_working_tree", &repo, &[], None);
}

#[test]
fn test_remove_by_name_from_main() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create a worktree
    let _worktree_path = repo.add_worktree("feature-a", "feature-a");

    // Remove it by name from main repo
    snapshot_remove("remove_by_name_from_main", &repo, &["feature-a"], None);
}

#[test]
fn test_remove_by_name_from_other_worktree() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create two worktrees
    let worktree_a = repo.add_worktree("feature-a", "feature-a");
    let _worktree_b = repo.add_worktree("feature-b", "feature-b");

    // From worktree A, remove worktree B by name
    snapshot_remove(
        "remove_by_name_from_other_worktree",
        &repo,
        &["feature-b"],
        Some(&worktree_a),
    );
}

#[test]
fn test_remove_current_by_name() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    let worktree_path = repo.add_worktree("feature-current", "feature-current");

    // Remove current worktree by specifying its name
    snapshot_remove(
        "remove_current_by_name",
        &repo,
        &["feature-current"],
        Some(&worktree_path),
    );
}

#[test]
fn test_remove_nonexistent_worktree() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Try to remove a worktree that doesn't exist
    snapshot_remove("remove_nonexistent_worktree", &repo, &["nonexistent"], None);
}

#[test]
fn test_remove_by_name_dirty_target() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    let worktree_path = repo.add_worktree("feature-dirty", "feature-dirty");

    // Create a dirty file in the target worktree
    std::fs::write(worktree_path.join("dirty.txt"), "uncommitted changes")
        .expect("Failed to create file");

    // Try to remove it by name from main repo
    snapshot_remove(
        "remove_by_name_dirty_target",
        &repo,
        &["feature-dirty"],
        None,
    );
}
