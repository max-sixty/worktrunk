use crate::common::{TestRepo, make_snapshot_cmd, setup_snapshot_settings};
use insta_cmd::assert_cmd_snapshot;
use std::fs;
use std::path::Path;
use std::process::Command;

/// Helper to create snapshot with normalized paths
fn snapshot_merge(test_name: &str, repo: &TestRepo, args: &[&str], cwd: Option<&Path>) {
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(repo, "merge", args, cwd);
        assert_cmd_snapshot!(test_name, cmd);
    });
}

/// Helper to snapshot switch command
fn snapshot_switch(test_name: &str, repo: &TestRepo, args: &[&str], cwd: Option<&Path>) {
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(repo, "switch", args, cwd);
        assert_cmd_snapshot!(test_name, cmd);
    });
}

/// Helper to create snapshot with custom environment (for LLM testing)
fn snapshot_merge_with_env(
    test_name: &str,
    repo: &TestRepo,
    args: &[&str],
    cwd: Option<&std::path::Path>,
    env_vars: &[(&str, &str)],
) {
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(repo, "merge", args, cwd);
        for (key, value) in env_vars {
            cmd.env(key, value);
        }
        assert_cmd_snapshot!(test_name, cmd);
    });
}

/// Standard merge test setup: creates repo with remote, main worktree, and feature worktree with one commit.
/// Returns (repo, feature_wt_path).
fn setup_merge_scenario() -> (TestRepo, std::path::PathBuf) {
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

    (repo, feature_wt)
}

/// Merge test setup with multiple commits on feature branch.
/// Returns (repo, feature_wt_path).
fn setup_merge_scenario_multi_commit() -> (TestRepo, std::path::PathBuf) {
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

    // Create a feature worktree and make multiple commits
    let feature_wt = repo.add_worktree("feature", "feature");

    std::fs::write(feature_wt.join("file1.txt"), "content 1").expect("Failed to write file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "file1.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "feat: add file 1"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    std::fs::write(feature_wt.join("file2.txt"), "content 2").expect("Failed to write file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "file2.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "feat: add file 2"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    (repo, feature_wt)
}

#[test]
fn test_merge_fast_forward() {
    let (repo, feature_wt) = setup_merge_scenario();

    // Merge feature into main
    snapshot_merge("merge_fast_forward", &repo, &["main"], Some(&feature_wt));
}

#[test]
fn test_merge_when_primary_not_on_default_but_default_has_worktree() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Move the primary worktree off main
    repo.switch_primary_to("develop");

    // Ensure main still has a worktree (secondary)
    let _main_wt = repo.add_main_worktree();

    // Create a feature worktree and commit
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

    snapshot_merge(
        "merge_when_primary_not_on_default_but_default_has_worktree",
        &repo,
        &["main"],
        Some(&feature_wt),
    );
}

#[test]
fn test_merge_with_no_remove_flag() {
    let (repo, feature_wt) = setup_merge_scenario();

    // Merge with --no-remove flag (should not finish worktree)
    snapshot_merge(
        "merge_with_no_remove",
        &repo,
        &["main", "--no-remove"],
        Some(&feature_wt),
    );
}

#[test]
fn test_merge_already_on_target() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Already on main branch (repo root)
    snapshot_merge("merge_already_on_target", &repo, &[], None);
}

#[test]
fn test_merge_dirty_working_tree() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create a feature worktree with uncommitted changes
    let feature_wt = repo.add_worktree("feature", "feature");
    std::fs::write(feature_wt.join("dirty.txt"), "uncommitted content")
        .expect("Failed to write file");

    // Try to merge (should fail due to dirty working tree)
    snapshot_merge(
        "merge_dirty_working_tree",
        &repo,
        &["main"],
        Some(&feature_wt),
    );
}

#[test]
fn test_merge_not_fast_forward() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create commits in both branches
    // Add commit to main (repo root)
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

    // Create a feature worktree branching from before the main commit
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

    // Try to merge (should fail or require actual merge)
    snapshot_merge(
        "merge_not_fast_forward",
        &repo,
        &["main"],
        Some(&feature_wt),
    );
}

#[test]
fn test_merge_rebase_conflict() {
    let mut repo = TestRepo::new();

    // Create a shared file
    std::fs::write(repo.root_path().join("shared.txt"), "initial content\n")
        .expect("Failed to write file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "shared.txt"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to add file");

    repo.commit("Add shared file");
    repo.setup_remote("main");

    // Create a worktree for main
    let main_wt = repo.root_path().parent().unwrap().join("test-repo.main-wt");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["worktree", "add", main_wt.to_str().unwrap(), "main"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to add worktree");

    // Modify shared.txt in main branch (from the base commit)
    std::fs::write(repo.root_path().join("shared.txt"), "main version\n")
        .expect("Failed to write file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "shared.txt"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to add file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Update shared.txt in main"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to commit");

    // Create a feature worktree branching from before the main commit
    // We need to create it from the original commit, not current main
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    let output = cmd
        .args(["rev-parse", "HEAD~1"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to get previous commit");
    let base_commit = String::from_utf8_lossy(&output.stdout).trim().to_string();

    let feature_wt = repo.root_path().parent().unwrap().join("test-repo.feature");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args([
        "worktree",
        "add",
        feature_wt.to_str().unwrap(),
        "-b",
        "feature",
        &base_commit,
    ])
    .current_dir(repo.root_path())
    .output()
    .expect("Failed to add feature worktree");

    // Modify the same file with conflicting content
    std::fs::write(feature_wt.join("shared.txt"), "feature version\n")
        .expect("Failed to write file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "shared.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Update shared.txt in feature"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Try to merge - should fail with rebase conflict
    snapshot_merge("merge_rebase_conflict", &repo, &["main"], Some(&feature_wt));
}

#[test]
fn test_merge_to_default_branch() {
    let (repo, feature_wt) = setup_merge_scenario();

    // Merge without specifying target (should use default branch)
    snapshot_merge("merge_to_default", &repo, &[], Some(&feature_wt));
}

#[test]
fn test_merge_error_detached_head() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Detach HEAD in the repo
    repo.detach_head();

    // Try to merge (should fail - detached HEAD)
    snapshot_merge(
        "merge_error_detached_head",
        &repo,
        &["main"],
        Some(repo.root_path()),
    );
}

#[test]
fn test_merge_squash_deterministic() {
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

    // Create a feature worktree and make multiple commits
    let feature_wt = repo.add_worktree("feature", "feature");

    std::fs::write(feature_wt.join("file1.txt"), "content 1").expect("Failed to write file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "file1.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "feat: add file 1"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    std::fs::write(feature_wt.join("file2.txt"), "content 2").expect("Failed to write file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "file2.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "fix: update logic"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    std::fs::write(feature_wt.join("file3.txt"), "content 3").expect("Failed to write file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "file3.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "docs: update readme"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Merge (squashing is now the default - no LLM configured, should use deterministic message)
    snapshot_merge(
        "merge_squash_deterministic",
        &repo,
        &["main"],
        Some(&feature_wt),
    );
}

#[test]
fn test_merge_squash_with_llm() {
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

    // Create a feature worktree and make multiple commits
    let feature_wt = repo.add_worktree("feature", "feature");

    std::fs::write(feature_wt.join("auth.txt"), "auth module").expect("Failed to write file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "auth.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "feat: add authentication"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    std::fs::write(feature_wt.join("auth.txt"), "auth module updated")
        .expect("Failed to write file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "auth.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "fix: handle edge case"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Configure mock LLM command via config file
    // Use sh to consume stdin and return a fixed message
    let worktrunk_config = r#"
[commit-generation]
command = "sh"
args = ["-c", "cat >/dev/null && echo 'feat: implement user authentication system'"]
"#;
    fs::write(repo.test_config_path(), worktrunk_config).expect("Failed to write worktrunk config");

    // (squashing is now the default, no need for --squash flag)
    snapshot_merge("merge_squash_with_llm", &repo, &["main"], Some(&feature_wt));
}

#[test]
fn test_merge_squash_llm_fallback() {
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

    // Create a feature worktree and make multiple commits
    let feature_wt = repo.add_worktree("feature", "feature");

    std::fs::write(feature_wt.join("file1.txt"), "content 1").expect("Failed to write file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "file1.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "feat: new feature"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    std::fs::write(feature_wt.join("file2.txt"), "content 2").expect("Failed to write file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "file2.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "fix: bug fix"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Configure LLM command that will fail (non-existent command)
    // Should now error instead of falling back
    // (squashing is now the default, no need for --squash flag)
    snapshot_merge_with_env(
        "merge_squash_llm_fallback",
        &repo,
        &["main"],
        Some(&feature_wt),
        &[(
            "WORKTRUNK_COMMIT_GENERATION__COMMAND",
            "nonexistent-llm-command",
        )],
    );
}

#[test]
fn test_merge_squash_single_commit() {
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

    // Create a feature worktree with only one commit
    let feature_wt = repo.add_worktree("feature", "feature");

    std::fs::write(feature_wt.join("file1.txt"), "content").expect("Failed to write file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "file1.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "feat: single commit"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Merge (squashing is default) - should skip squashing since there's only one commit
    snapshot_merge(
        "merge_squash_single_commit",
        &repo,
        &["main"],
        Some(&feature_wt),
    );
}

#[test]
fn test_merge_no_squash() {
    let (repo, feature_wt) = setup_merge_scenario_multi_commit();

    // Merge with --no-squash - should NOT squash the commits
    snapshot_merge(
        "merge_no_squash",
        &repo,
        &["main", "--no-squash"],
        Some(&feature_wt),
    );
}

#[test]
fn test_merge_squash_empty_changes() {
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

    // Create a feature worktree with commits that result in no net changes
    let feature_wt = repo.add_worktree("feature", "feature");

    // Get the initial content of file.txt (created by the initial commit)
    let file_path = feature_wt.join("file.txt");
    let initial_content = std::fs::read_to_string(&file_path).expect("Failed to read file.txt");

    // Commit 1: Modify file.txt
    std::fs::write(&file_path, "change1").expect("Failed to write file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "file.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Change 1"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Commit 2: Modify file.txt again
    std::fs::write(&file_path, "change2").expect("Failed to write file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "file.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Change 2"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Commit 3: Revert to original content
    std::fs::write(&file_path, initial_content).expect("Failed to write file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "file.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Revert to initial"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Merge (squashing is default) - should succeed even when commits result in no net changes
    snapshot_merge(
        "merge_squash_empty_changes",
        &repo,
        &["main"],
        Some(&feature_wt),
    );
}

#[test]
fn test_merge_auto_commit_deterministic() {
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

    // Create a feature worktree with a commit
    let feature_wt = repo.add_worktree("feature", "feature");
    std::fs::write(feature_wt.join("feature.txt"), "initial content")
        .expect("Failed to write file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "feature.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "feat: initial feature"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Now add uncommitted tracked changes
    std::fs::write(feature_wt.join("feature.txt"), "modified content")
        .expect("Failed to write file");

    // Merge - should auto-commit with deterministic message (no LLM configured)
    snapshot_merge(
        "merge_auto_commit_deterministic",
        &repo,
        &["main"],
        Some(&feature_wt),
    );
}

#[test]
fn test_merge_auto_commit_with_llm() {
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

    // Create a feature worktree with a commit
    let feature_wt = repo.add_worktree("feature", "feature");
    std::fs::write(feature_wt.join("auth.txt"), "initial auth").expect("Failed to write file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "auth.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "feat: add authentication"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Now add uncommitted tracked changes
    std::fs::write(feature_wt.join("auth.txt"), "improved auth with validation")
        .expect("Failed to write file");

    // Configure mock LLM command via config file
    // Use /bin/echo to avoid PATH resolution issues in CI environments
    let worktrunk_config = r#"
[commit-generation]
command = "/bin/echo"
args = ["fix: improve auth validation logic"]
"#;
    fs::write(repo.test_config_path(), worktrunk_config).expect("Failed to write worktrunk config");

    // Merge with LLM configured - should auto-commit with LLM message
    snapshot_merge(
        "merge_auto_commit_with_llm",
        &repo,
        &["main"],
        Some(&feature_wt),
    );
}

#[test]
fn test_merge_auto_commit_and_squash() {
    let (repo, feature_wt) = setup_merge_scenario_multi_commit();

    // Add uncommitted tracked changes
    std::fs::write(feature_wt.join("file1.txt"), "updated content 1")
        .expect("Failed to write file");

    // Configure mock LLM command via config file
    // Use /bin/echo to avoid PATH resolution issues in CI environments
    let worktrunk_config = r#"
[commit-generation]
command = "/bin/echo"
args = ["fix: update file 1 content"]
"#;
    fs::write(repo.test_config_path(), worktrunk_config).expect("Failed to write worktrunk config");

    // Merge (squashing is default) - should stage uncommitted changes, then squash all commits including the staged changes
    snapshot_merge(
        "merge_auto_commit_and_squash",
        &repo,
        &["main"],
        Some(&feature_wt),
    );
}

#[test]
fn test_merge_with_untracked_files() {
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

    // Create a feature worktree with one commit
    let feature_wt = repo.add_worktree("feature", "feature");
    std::fs::write(feature_wt.join("file1.txt"), "content 1").expect("Failed to write file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "file1.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "feat: add file 1"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Add untracked files
    std::fs::write(feature_wt.join("untracked1.txt"), "untracked content 1")
        .expect("Failed to write file");
    std::fs::write(feature_wt.join("untracked2.txt"), "untracked content 2")
        .expect("Failed to write file");

    // Merge - should show warning about untracked files
    snapshot_merge_with_env(
        "merge_with_untracked_files",
        &repo,
        &["main"],
        Some(&feature_wt),
        &[
            ("WORKTRUNK_COMMIT_GENERATION__COMMAND", "echo"),
            ("WORKTRUNK_COMMIT_GENERATION__ARGS", "fix: commit changes"),
        ],
    );
}

#[test]
fn test_merge_pre_merge_command_success() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create project config with pre-merge command
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create config dir");
    fs::write(
        config_dir.join("wt.toml"),
        r#"pre-merge-command = "exit 0""#,
    )
    .expect("Failed to write config");

    repo.commit("Add config");

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
    fs::write(feature_wt.join("feature.txt"), "feature content").expect("Failed to write file");

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

    // Merge with --force to skip approval prompts
    snapshot_merge(
        "merge_pre_merge_command_success",
        &repo,
        &["main", "--force"],
        Some(&feature_wt),
    );
}

#[test]
fn test_merge_pre_merge_command_failure() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create project config with failing pre-merge command
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create config dir");
    fs::write(
        config_dir.join("wt.toml"),
        r#"pre-merge-command = "exit 1""#,
    )
    .expect("Failed to write config");

    repo.commit("Add config");

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
    fs::write(feature_wt.join("feature.txt"), "feature content").expect("Failed to write file");

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

    // Merge with --force - pre-merge command should fail and block merge
    snapshot_merge(
        "merge_pre_merge_command_failure",
        &repo,
        &["main", "--force"],
        Some(&feature_wt),
    );
}

#[test]
fn test_merge_pre_merge_command_no_hooks() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create project config with failing pre-merge command
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create config dir");
    fs::write(
        config_dir.join("wt.toml"),
        r#"pre-merge-command = "exit 1""#,
    )
    .expect("Failed to write config");

    repo.commit("Add config");

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
    fs::write(feature_wt.join("feature.txt"), "feature content").expect("Failed to write file");

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

    // Merge with --no-hooks - should skip pre-merge commands and succeed
    snapshot_merge(
        "merge_pre_merge_command_no_hooks",
        &repo,
        &["main", "--no-hooks"],
        Some(&feature_wt),
    );
}

#[test]
fn test_merge_pre_merge_command_named() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create project config with named pre-merge commands
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create config dir");
    fs::write(
        config_dir.join("wt.toml"),
        r#"
[pre-merge-command]
format = "exit 0"
lint = "exit 0"
test = "exit 0"
"#,
    )
    .expect("Failed to write config");

    repo.commit("Add config");

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
    fs::write(feature_wt.join("feature.txt"), "feature content").expect("Failed to write file");

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

    // Merge with --force - all pre-merge commands should pass
    snapshot_merge(
        "merge_pre_merge_command_named",
        &repo,
        &["main", "--force"],
        Some(&feature_wt),
    );
}

#[test]
fn test_merge_post_merge_command_success() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create project config with post-merge command that writes a marker file
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create config dir");
    fs::write(
        config_dir.join("wt.toml"),
        r#"post-merge-command = "echo 'merged {{ branch }} to {{ target }}' > post-merge-ran.txt""#,
    )
    .expect("Failed to write config");

    repo.commit("Add config");

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
    fs::write(feature_wt.join("feature.txt"), "feature content").expect("Failed to write file");

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

    // Merge with --force
    snapshot_merge(
        "merge_post_merge_command_success",
        &repo,
        &["main", "--force"],
        Some(&feature_wt),
    );

    // Verify the command ran in the main worktree (not the feature worktree)
    let marker_file = repo.root_path().join("post-merge-ran.txt");
    assert!(
        marker_file.exists(),
        "Post-merge command should have created marker file in main worktree"
    );
    let content = fs::read_to_string(&marker_file).expect("Failed to read marker file");
    assert!(
        content.contains("merged feature to main"),
        "Marker file should contain correct branch and target: {}",
        content
    );
}

#[test]
fn test_merge_post_merge_command_skipped_with_no_verify() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create project config with post-merge command that writes a marker file
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create config dir");
    fs::write(
        config_dir.join("wt.toml"),
        r#"post-merge-command = "echo 'merged {{ branch }} to {{ target }}' > post-merge-ran.txt""#,
    )
    .expect("Failed to write config");

    repo.commit("Add config");

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
    fs::write(feature_wt.join("feature.txt"), "feature content").expect("Failed to write file");

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

    // Merge with --no-verify - hook should be skipped entirely
    snapshot_merge(
        "merge_post_merge_command_no_verify",
        &repo,
        &["main", "--force", "--no-verify"],
        Some(&feature_wt),
    );

    // Verify the command did not run in the main worktree
    let marker_file = repo.root_path().join("post-merge-ran.txt");
    assert!(
        !marker_file.exists(),
        "Post-merge command should not run when --no-verify is set"
    );
}

#[test]
fn test_merge_post_merge_command_failure() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create project config with failing post-merge command
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create config dir");
    fs::write(
        config_dir.join("wt.toml"),
        r#"post-merge-command = "exit 1""#,
    )
    .expect("Failed to write config");

    repo.commit("Add config");

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
    fs::write(feature_wt.join("feature.txt"), "feature content").expect("Failed to write file");

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

    // Merge with --force - post-merge command should fail but merge should complete
    snapshot_merge(
        "merge_post_merge_command_failure",
        &repo,
        &["main", "--force"],
        Some(&feature_wt),
    );
}

#[test]
fn test_merge_post_merge_command_named() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create project config with named post-merge commands
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create config dir");
    fs::write(
        config_dir.join("wt.toml"),
        r#"
[post-merge-command]
notify = "echo 'Merge to {{ target }} complete' > notify.txt"
deploy = "echo 'Deploying branch {{ branch }}' > deploy.txt"
"#,
    )
    .expect("Failed to write config");

    repo.commit("Add config");

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
    fs::write(feature_wt.join("feature.txt"), "feature content").expect("Failed to write file");

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

    // Merge with --force
    snapshot_merge(
        "merge_post_merge_command_named",
        &repo,
        &["main", "--force"],
        Some(&feature_wt),
    );

    // Verify both commands ran
    let notify_file = repo.root_path().join("notify.txt");
    let deploy_file = repo.root_path().join("deploy.txt");
    assert!(
        notify_file.exists(),
        "Notify command should have created marker file"
    );
    assert!(
        deploy_file.exists(),
        "Deploy command should have created marker file"
    );
}

#[test]
fn test_merge_pre_commit_command_success() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create project config with pre-commit command
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create config dir");
    fs::write(
        config_dir.join("wt.toml"),
        r#"pre-commit-command = "echo 'Pre-commit check passed'""#,
    )
    .expect("Failed to write config");

    repo.commit("Add config");

    // Create a worktree for main
    let main_wt = repo.root_path().parent().unwrap().join("test-repo.main-wt");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["worktree", "add", main_wt.to_str().unwrap(), "main"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to add worktree");

    // Create a feature worktree and make a change
    let feature_wt = repo.add_worktree("feature", "feature");
    fs::write(feature_wt.join("feature.txt"), "feature content").expect("Failed to write file");

    // Merge with --force (changes uncommitted, should trigger pre-commit hook)
    snapshot_merge(
        "merge_pre_commit_command_success",
        &repo,
        &["main", "--force"],
        Some(&feature_wt),
    );
}

#[test]
fn test_merge_pre_commit_command_failure() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create project config with failing pre-commit command
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create config dir");
    fs::write(
        config_dir.join("wt.toml"),
        r#"pre-commit-command = "exit 1""#,
    )
    .expect("Failed to write config");

    repo.commit("Add config");

    // Create a worktree for main
    let main_wt = repo.root_path().parent().unwrap().join("test-repo.main-wt");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["worktree", "add", main_wt.to_str().unwrap(), "main"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to add worktree");

    // Create a feature worktree and make a change
    let feature_wt = repo.add_worktree("feature", "feature");
    fs::write(feature_wt.join("feature.txt"), "feature content").expect("Failed to write file");

    // Merge with --force - pre-commit command should fail and block merge
    snapshot_merge(
        "merge_pre_commit_command_failure",
        &repo,
        &["main", "--force"],
        Some(&feature_wt),
    );
}

#[test]
fn test_merge_pre_squash_command_success() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create project config with pre-commit command (used for both squash and no-squash)
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create config dir");
    fs::write(
        config_dir.join("wt.toml"),
        "pre-commit-command = \"echo 'Pre-commit check passed'\"",
    )
    .expect("Failed to write config");

    repo.commit("Add config");

    // Create a worktree for main
    let main_wt = repo.root_path().parent().unwrap().join("test-repo.main-wt");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["worktree", "add", main_wt.to_str().unwrap(), "main"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to add worktree");

    // Create a feature worktree and make commits
    let feature_wt = repo.add_worktree("feature", "feature");
    fs::write(feature_wt.join("feature.txt"), "feature content").expect("Failed to write file");

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

    // Merge with --force (squashing is now the default)
    snapshot_merge(
        "merge_pre_squash_command_success",
        &repo,
        &["main", "--force"],
        Some(&feature_wt),
    );
}

#[test]
fn test_merge_pre_squash_command_failure() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create project config with failing pre-commit command (used for both squash and no-squash)
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create config dir");
    fs::write(
        config_dir.join("wt.toml"),
        r#"pre-commit-command = "exit 1""#,
    )
    .expect("Failed to write config");

    repo.commit("Add config");

    // Create a worktree for main
    let main_wt = repo.root_path().parent().unwrap().join("test-repo.main-wt");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["worktree", "add", main_wt.to_str().unwrap(), "main"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to add worktree");

    // Create a feature worktree and make commits
    let feature_wt = repo.add_worktree("feature", "feature");
    fs::write(feature_wt.join("feature.txt"), "feature content").expect("Failed to write file");

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

    // Merge with --force (squashing is default) - pre-commit command should fail and block merge
    snapshot_merge(
        "merge_pre_squash_command_failure",
        &repo,
        &["main", "--force"],
        Some(&feature_wt),
    );
}

#[test]
fn test_merge_no_remote() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    // Deliberately NOT calling setup_remote to test the error case

    // Create a feature worktree and make a commit
    let feature_wt = repo.add_worktree("feature", "feature");
    fs::write(feature_wt.join("feature.txt"), "feature content").expect("Failed to write file");

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

    // Try to merge without specifying target (should fail - no remote to get default branch)
    snapshot_merge("merge_no_remote", &repo, &[], Some(&feature_wt));
}

// README EXAMPLE GENERATION TESTS
// These tests are specifically designed to generate realistic output examples for the README.
// The snapshots from these tests are manually copied into README.md to show users what
// worktrunk output looks like in practice.

/// Generate README example: Simple merge workflow with a single commit
/// This demonstrates the basic "What It Does" flow - create worktree, make changes, merge back.
///
/// Output is used in README.md "What It Does" section.
/// Merge output: tests/snapshots/integration__integration_tests__merge__readme_example_simple.snap
/// Switch output: tests/snapshots/integration__integration_tests__merge__readme_example_simple_switch.snap
#[test]
fn test_readme_example_simple() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Snapshot the switch --create command (runs from bare repo)
    snapshot_switch(
        "readme_example_simple_switch",
        &repo,
        &["--create", "fix-auth"],
        None,
    );

    // Get the created worktree path and make a commit
    let feature_wt = repo
        .root_path()
        .parent()
        .unwrap()
        .join("test-repo.fix-auth");
    let auth_rs = r#"// JWT validation utilities
pub struct JwtClaims {
    pub sub: String,
    pub scope: String,
}

pub fn validate(token: &str) -> bool {
    token.starts_with("Bearer ") && token.split('.').count() == 3
}

pub fn refresh(refresh_token: &str) -> String {
    format!("{}::refreshed", refresh_token)
}
"#;
    std::fs::write(feature_wt.join("auth.rs"), auth_rs).expect("Failed to write file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "auth.rs"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Implement JWT validation"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Snapshot the merge command
    snapshot_merge("readme_example_simple", &repo, &["main"], Some(&feature_wt));
}

/// Generate README example: Complex merge with multiple hooks
/// This demonstrates advanced features - pre-merge hooks (tests, lints), post-merge hooks.
/// Shows the full power of worktrunk's automation capabilities.
///
/// Output is used in README.md "Advanced Features" or "Project Automation" section.
/// Source: tests/snapshots/integration__integration_tests__merge__readme_example_complex.snap
#[test]
fn test_readme_example_complex() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create project config with multiple hooks
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create .config dir");

    // Create mock commands for realistic output
    let bin_dir = repo.root_path().join(".bin");
    fs::create_dir_all(&bin_dir).expect("Failed to create bin dir");

    // Mock cargo that handles both test and clippy subcommands
    let cargo_script = r#"#!/bin/sh
if [ "$1" = "test" ]; then
    echo "    Finished test [unoptimized + debuginfo] target(s) in 0.12s"
    echo "     Running unittests src/lib.rs (target/debug/deps/worktrunk-abc123)"
    echo ""
    echo "running 18 tests"
    echo "test auth::tests::test_jwt_decode ... ok"
    echo "test auth::tests::test_jwt_encode ... ok"
    echo "test auth::tests::test_token_refresh ... ok"
    echo "test auth::tests::test_token_validation ... ok"
    echo ""
    echo "test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.08s"
    exit 0
elif [ "$1" = "clippy" ]; then
    echo "    Checking worktrunk v0.1.0"
    echo "    Finished dev [unoptimized + debuginfo] target(s) in 1.23s"
    exit 0
elif [ "$1" = "install" ]; then
    echo "  Installing worktrunk v0.1.0"
    echo "   Compiling worktrunk v0.1.0"
    echo "    Finished release [optimized] target(s) in 2.34s"
    echo "  Installing ~/.cargo/bin/wt"
    echo "   Installed package \`worktrunk v0.1.0\` (executable \`wt\`)"
    exit 0
else
    echo "cargo: unknown subcommand '$1'"
    exit 1
fi
"#;
    fs::write(bin_dir.join("cargo"), cargo_script).expect("Failed to write cargo script");

    // Mock llm command that generates a high-quality commit message
    let llm_script = r#"#!/bin/sh
# Read stdin (the prompt) but ignore it for deterministic output
cat > /dev/null

# Return a realistic, high-quality squash commit message
cat << 'EOF'
feat(auth): Implement JWT authentication system

Add comprehensive JWT token handling including validation, refresh logic,
and authentication tests. This establishes the foundation for secure
API authentication.

- Implement token refresh mechanism with expiry handling
- Add JWT encoding/decoding with signature verification
- Create test suite covering all authentication flows
EOF
"#;
    fs::write(bin_dir.join("llm"), llm_script).expect("Failed to write llm script");

    // Make scripts executable (Unix only - Windows doesn't use executable bits)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(bin_dir.join("cargo")).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(bin_dir.join("cargo"), perms).unwrap();

        let mut perms = fs::metadata(bin_dir.join("llm")).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(bin_dir.join("llm"), perms).unwrap();
    }

    let config_content = r#"
[pre-merge-command]
"test" = "cargo test"
"lint" = "cargo clippy"

[post-merge-command]
"install" = "cargo install --path ."
"#;

    fs::write(config_dir.join("wt.toml"), config_content).expect("Failed to write project config");

    // Commit the config and mock cargo
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", ".config/wt.toml", ".bin"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to add config");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add project automation config"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to commit config");

    // Create a worktree for main
    let main_wt = repo.root_path().parent().unwrap().join("test-repo.main-wt");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["worktree", "add", main_wt.to_str().unwrap(), "main"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to add worktree");

    // Create a feature worktree and make multiple commits
    let feature_wt = repo.add_worktree("feature-auth", "feature-auth");

    // First commit: token refresh
    let commit_one = r#"// Token refresh logic
pub fn refresh(secret: &str, expires_in: u32) -> String {
    format!("{}::{}", secret, expires_in)
}

pub fn needs_rotation(issued_at: u64, ttl: u64, now: u64) -> bool {
    now.saturating_sub(issued_at) > ttl
}
"#;
    std::fs::write(feature_wt.join("auth.rs"), commit_one).expect("Failed to write file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "auth.rs"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add token refresh logic"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Second commit: JWT validation
    let commit_two = r#"// JWT validation
pub fn validate_signature(payload: &str, signature: &str) -> bool {
    !payload.is_empty() && signature.len() > 12
}

pub fn decode_claims(token: &str) -> Option<&str> {
    token.split('.').nth(1)
}
"#;
    std::fs::write(feature_wt.join("jwt.rs"), commit_two).expect("Failed to write file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "jwt.rs"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Implement JWT validation"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Third commit: tests
    let commit_three = r#"// Tests
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_rotates_secret() {
        let token = refresh("token", 30);
        assert!(token.contains("token::30"));
    }

    #[test]
    fn decode_claims_returns_payload() {
        let token = "header.payload.signature";
        assert_eq!(decode_claims(token), Some("payload"));
    }
}
"#;
    std::fs::write(feature_wt.join("auth_test.rs"), commit_three).expect("Failed to write file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "auth_test.rs"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add authentication tests"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Configure LLM in worktrunk config for deterministic, high-quality commit messages
    let llm_path = bin_dir.join("llm");
    let worktrunk_config = format!(
        r#"
[commit-generation]
command = "{}"
"#,
        llm_path.display()
    );
    fs::write(repo.test_config_path(), worktrunk_config).expect("Failed to write worktrunk config");

    // Merge with --force to skip approval prompts for commands
    // This test explicitly sets PATH (which will be captured in snapshot) because it needs
    // to find mock commands in .bin directory. We use a clean, minimal PATH to avoid leaking
    // user-specific paths like ~/.cargo/bin into the snapshot.
    //
    // TODO: This hardcoded PATH works on macOS and Linux CI, but may not work on all
    // environments (e.g., Windows, other package managers like nixpkgs). We should
    // reassess whether there's a better approach that doesn't require hardcoding
    // system paths. Ideally we'd avoid setting PATH entirely, but this test needs it
    // for mock commands.
    let path_with_bin = format!(
        "{}:/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        bin_dir.display()
    );
    snapshot_merge_with_env(
        "readme_example_complex",
        &repo,
        &["main", "--force"],
        Some(&feature_wt),
        &[("PATH", &path_with_bin)],
    );
}

/// Generate README example: Creating worktree with post-create and post-start hooks
/// This demonstrates the hooks feature with realistic tool output (uv sync, dev server).
///
/// Output is used in README.md "Project Hooks" section.
/// Source: tests/snapshots/integration__integration_tests__merge__readme_example_hooks_post_create.snap
#[test]
fn test_readme_example_hooks_post_create() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create project config with post-create and post-start hooks
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create .config dir");

    // Create mock commands for realistic output
    let bin_dir = repo.root_path().join(".bin");
    fs::create_dir_all(&bin_dir).expect("Failed to create bin dir");

    // Mock uv command that simulates dependency installation
    let uv_script = r#"#!/bin/sh
if [ "$1" = "sync" ]; then
    echo ""
    echo "  Resolved 24 packages in 145ms"
    echo "  Installed 24 packages in 1.2s"
    exit 0
elif [ "$1" = "run" ] && [ "$2" = "dev" ]; then
    echo ""
    echo "  Starting dev server on http://localhost:3000..."
    exit 0
else
    echo "uv: unknown command '$1 $2'"
    exit 1
fi
"#;
    fs::write(bin_dir.join("uv"), uv_script).expect("Failed to write uv script");

    // Make scripts executable (Unix only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(bin_dir.join("uv")).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(bin_dir.join("uv"), perms).unwrap();
    }

    let config_content = r#"
[post-create-command]
"install" = "uv sync"

[post-start-command]
"dev" = "uv run dev"
"#;

    fs::write(config_dir.join("wt.toml"), config_content).expect("Failed to write project config");

    // Commit the config
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", ".config/wt.toml", ".bin"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to add config");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add project hooks"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to commit config");

    // Set PATH to include mock commands and run switch --create with --force
    let path_with_bin = format!(
        "{}:/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        bin_dir.display()
    );

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd =
            make_snapshot_cmd(&repo, "switch", &["--create", "feature-x", "--force"], None);
        cmd.env("PATH", &path_with_bin);
        assert_cmd_snapshot!("readme_example_hooks_post_create", cmd);
    });
}

/// Generate README example: Merging with pre-merge hooks (test and lint)
/// This demonstrates the pre-merge hooks feature with realistic pytest and ruff output.
///
/// Output is used in README.md "Project Hooks" section.
/// Source: tests/snapshots/integration__integration_tests__merge__readme_example_hooks_pre_merge.snap
#[test]
fn test_readme_example_hooks_pre_merge() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create project config with pre-merge hooks
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create .config dir");

    // Create mock commands for realistic output
    let bin_dir = repo.root_path().join(".bin");
    fs::create_dir_all(&bin_dir).expect("Failed to create bin dir");

    // Mock pytest command
    let pytest_script = r#"#!/bin/sh
cat << 'EOF'

============================= test session starts ==============================
collected 3 items

tests/test_auth.py::test_login_success PASSED                            [ 33%]
tests/test_auth.py::test_login_invalid_password PASSED                   [ 66%]
tests/test_auth.py::test_token_validation PASSED                         [100%]

============================== 3 passed in 0.8s ===============================

EOF
exit 0
"#;
    fs::write(bin_dir.join("pytest"), pytest_script).expect("Failed to write pytest script");

    // Mock ruff command
    let ruff_script = r#"#!/bin/sh
if [ "$1" = "check" ]; then
    echo ""
    echo "All checks passed!"
    echo ""
    exit 0
else
    echo "ruff: unknown command '$1'"
    exit 1
fi
"#;
    fs::write(bin_dir.join("ruff"), ruff_script).expect("Failed to write ruff script");

    // Mock llm command for commit message
    let llm_script = r#"#!/bin/sh
cat > /dev/null
cat << 'EOF'
feat(api): Add user authentication endpoints

Implement login and token refresh endpoints with JWT validation.
Includes comprehensive test coverage and input validation.
EOF
"#;
    fs::write(bin_dir.join("llm"), llm_script).expect("Failed to write llm script");

    // Mock uv command for running pytest and ruff
    let uv_script = r#"#!/bin/sh
if [ "$1" = "run" ] && [ "$2" = "pytest" ]; then
    exec pytest
elif [ "$1" = "run" ] && [ "$2" = "ruff" ]; then
    shift 2
    exec ruff "$@"
else
    echo "uv: unknown command '$1 $2'"
    exit 1
fi
"#;
    fs::write(bin_dir.join("uv"), uv_script).expect("Failed to write uv script");

    // Make scripts executable (Unix only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for script in &["pytest", "ruff", "llm", "uv"] {
            let mut perms = fs::metadata(bin_dir.join(script)).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(bin_dir.join(script), perms).unwrap();
        }
    }

    let config_content = r#"
[pre-merge-command]
"test" = "uv run pytest"
"lint" = "uv run ruff check"
"#;

    fs::write(config_dir.join("wt.toml"), config_content).expect("Failed to write project config");

    // Commit the config
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", ".config/wt.toml", ".bin"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to add config");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add pre-merge hooks"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to commit config");

    // Create a worktree for main
    let main_wt = repo.root_path().parent().unwrap().join("test-repo.main-wt");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["worktree", "add", main_wt.to_str().unwrap(), "main"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to add worktree");

    // Create a feature worktree and make multiple commits
    let feature_wt = repo.add_worktree("feature-auth", "feature-auth");

    // First commit - create initial auth.py with login endpoint
    fs::create_dir_all(feature_wt.join("api")).expect("Failed to create api dir");
    let auth_py_v1 = r#"# Authentication API endpoints
from typing import Dict, Optional
import jwt
from datetime import datetime, timedelta, timezone

def login(username: str, password: str) -> Optional[Dict]:
    """Authenticate user and return JWT token."""
    # Validate credentials (stub)
    if not username or not password:
        return None

    # Generate JWT token
    payload = {
        'sub': username,
        'exp': datetime.now(timezone.utc) + timedelta(hours=1)
    }
    token = jwt.encode(payload, 'secret', algorithm='HS256')
    return {'token': token, 'expires_in': 3600}
"#;
    std::fs::write(feature_wt.join("api/auth.py"), auth_py_v1).expect("Failed to write file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "api/auth.py"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add login endpoint"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Second commit - add tests
    fs::create_dir_all(feature_wt.join("tests")).expect("Failed to create tests dir");
    let test_auth_py = r#"# Authentication endpoint tests
import pytest
from api.auth import login

def test_login_success():
    result = login('user', 'pass')
    assert result and 'token' in result

def test_login_invalid_password():
    result = login('user', '')
    assert result is None

def test_token_validation():
    assert login('valid_user', 'valid_pass')['expires_in'] == 3600
"#;
    std::fs::write(feature_wt.join("tests/test_auth.py"), test_auth_py)
        .expect("Failed to write file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "tests/test_auth.py"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add authentication tests"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Third commit - add refresh endpoint
    let auth_py_v2 = r#"# Authentication API endpoints
from typing import Dict, Optional
import jwt
from datetime import datetime, timedelta, timezone

def login(username: str, password: str) -> Optional[Dict]:
    """Authenticate user and return JWT token."""
    # Validate credentials (stub)
    if not username or not password:
        return None

    # Generate JWT token
    payload = {
        'sub': username,
        'exp': datetime.now(timezone.utc) + timedelta(hours=1)
    }
    token = jwt.encode(payload, 'secret', algorithm='HS256')
    return {'token': token, 'expires_in': 3600}

def refresh_token(token: str) -> Optional[Dict]:
    """Refresh an existing JWT token."""
    try:
        payload = jwt.decode(token, 'secret', algorithms=['HS256'])
        new_payload = {
            'sub': payload['sub'],
            'exp': datetime.now(timezone.utc) + timedelta(hours=1)
        }
        new_token = jwt.encode(new_payload, 'secret', algorithm='HS256')
        return {'token': new_token, 'expires_in': 3600}
    except jwt.InvalidTokenError:
        return None
"#;
    std::fs::write(feature_wt.join("api/auth.py"), auth_py_v2).expect("Failed to write file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "api/auth.py"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add validation"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Configure LLM in worktrunk config
    let llm_path = bin_dir.join("llm");
    let worktrunk_config = format!(
        r#"
[commit-generation]
command = "{}"
"#,
        llm_path.display()
    );
    fs::write(repo.test_config_path(), worktrunk_config).expect("Failed to write worktrunk config");

    // Set PATH and merge with --force
    let path_with_bin = format!(
        "{}:/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        bin_dir.display()
    );
    snapshot_merge_with_env(
        "readme_example_hooks_pre_merge",
        &repo,
        &["main", "--force"],
        Some(&feature_wt),
        &[("PATH", &path_with_bin)],
    );
}

#[test]
fn test_merge_no_commit_with_clean_tree() {
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

    // Create a feature worktree with commits (clean tree)
    let feature_wt = repo.add_worktree("feature", "feature");
    fs::write(feature_wt.join("feature.txt"), "feature content").expect("Failed to write file");

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

    // Merge with --no-commit (should succeed - clean tree)
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(
            &repo,
            "merge",
            &["main", "--no-commit", "--no-remove"],
            Some(&feature_wt),
        );
        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----
        🔄 [36mMerging 1 commit to [1m[36mmain[0m[36m @ [2m[SHA][0m (no commit/squash/rebase needed)
        [107m [0m  * [SHA][33m ([m[1;36mHEAD[m[33m -> [m[1;32mfeature[m[33m)[m Add feature file
        [107m [0m   feature.txt | 1 [32m+[m
        [107m [0m   1 file changed, 1 insertion(+)
        ✅ [32mMerged to [1m[32mmain[0m[0m (1 commit, 1 file, [32m+1[0m)
        ✅ [32mWorktree preserved (--no-remove)[0m

        ----- stderr -----
        ");
    });
}

#[test]
fn test_merge_no_commit_with_dirty_tree() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create a feature worktree with a commit
    let feature_wt = repo.add_worktree("feature", "feature");
    fs::write(feature_wt.join("committed.txt"), "committed content").expect("Failed to write file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "committed.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add committed file"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Add uncommitted changes
    fs::write(feature_wt.join("uncommitted.txt"), "uncommitted content")
        .expect("Failed to write file");

    // Try to merge with --no-commit (should fail - dirty tree)
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd =
            make_snapshot_cmd(&repo, "merge", &["main", "--no-commit"], Some(&feature_wt));
        assert_cmd_snapshot!(cmd, @r"
        success: false
        exit_code: 1
        ----- stdout -----
        ❌ [31mWorking tree has uncommitted changes[0m

        💡 [2mCommit or stash them first[0m

        ----- stderr -----
        ");
    });
}

#[test]
fn test_merge_no_commit_no_squash_no_remove_redundant() {
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

    // Create a feature worktree with commits (clean tree)
    let feature_wt = repo.add_worktree("feature", "feature");
    fs::write(feature_wt.join("feature.txt"), "feature content").expect("Failed to write file");

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

    // Merge with --no-commit --no-squash --no-remove (redundant but valid - should succeed)
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(
            &repo,
            "merge",
            &["main", "--no-commit", "--no-squash", "--no-remove"],
            Some(&feature_wt),
        );
        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----
        🔄 [36mMerging 1 commit to [1m[36mmain[0m[36m @ [2m[SHA][0m (no commit/squash/rebase needed)
        [107m [0m  * [SHA][33m ([m[1;36mHEAD[m[33m -> [m[1;32mfeature[m[33m)[m Add feature file
        [107m [0m   feature.txt | 1 [32m+[m
        [107m [0m   1 file changed, 1 insertion(+)
        ✅ [32mMerged to [1m[32mmain[0m[0m (1 commit, 1 file, [32m+1[0m)
        ✅ [32mWorktree preserved (--no-remove)[0m

        ----- stderr -----
        ");
    });
}

#[test]
fn test_merge_no_commits() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create a worktree for main
    repo.add_main_worktree();

    // Create a feature worktree with NO commits (just branched from main)
    let feature_wt = repo.add_worktree("no-commits", "no-commits");

    // Merge without any commits - should skip both squashing and rebasing
    snapshot_merge("merge_no_commits", &repo, &["main"], Some(&feature_wt));
}

#[test]
fn test_merge_no_commits_with_changes() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create a worktree for main
    repo.add_main_worktree();

    // Create a feature worktree with NO commits but WITH uncommitted changes
    let feature_wt = repo.add_worktree("no-commits-dirty", "no-commits-dirty");
    fs::write(feature_wt.join("newfile.txt"), "new content").expect("Failed to write file");

    // Merge - should commit the changes, skip squashing (only 1 commit), and skip rebasing (at merge base)
    snapshot_merge(
        "merge_no_commits_with_changes",
        &repo,
        &["main"],
        Some(&feature_wt),
    );
}

#[test]
fn test_merge_rebase_fast_forward() {
    // Test fast-forward case: branch has no commits, main moved ahead
    // Should show "Fast-forwarded to main" without progress message
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create a feature worktree with NO commits (just branched from main)
    let feature_wt = repo.add_worktree("fast-forward-test", "fast-forward-test");

    // Advance main with a new commit (in the primary worktree which is on main)
    fs::write(repo.root_path().join("main-update.txt"), "main content")
        .expect("Failed to write file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "main-update.txt"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to add file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Update main"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to commit");

    // Merge - should fast-forward (no commits to replay)
    snapshot_merge(
        "merge_rebase_fast_forward",
        &repo,
        &["main"],
        Some(&feature_wt),
    );
}

#[test]
fn test_merge_rebase_true_rebase() {
    // Test true rebase case: branch has commits and main moved ahead
    // Should show "Rebasing onto main..." progress message
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create a feature worktree with a commit
    let feature_wt = repo.add_worktree("true-rebase-test", "true-rebase-test");
    fs::write(feature_wt.join("feature.txt"), "feature content").expect("Failed to write file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "feature.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add feature"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Advance main with a new commit (in the primary worktree which is on main)
    fs::write(repo.root_path().join("main-update.txt"), "main content")
        .expect("Failed to write file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "main-update.txt"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to add file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Update main"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to commit");

    // Merge - should show rebasing progress (has commits to replay)
    snapshot_merge(
        "merge_rebase_true_rebase",
        &repo,
        &["main"],
        Some(&feature_wt),
    );
}

#[test]
fn test_merge_primary_on_different_branch() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    repo.switch_primary_to("develop");
    assert_eq!(repo.current_branch(), "develop");

    // Create a feature worktree and make a commit
    let feature_wt = repo.add_worktree("feature-from-develop", "feature-from-develop");
    fs::write(feature_wt.join("feature.txt"), "feature content").expect("Failed to write file");

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

    snapshot_merge(
        "merge_primary_on_different_branch",
        &repo,
        &["main"],
        Some(&feature_wt),
    );

    // Verify primary stayed on develop (we don't switch branches, only worktrees)
    assert_eq!(repo.current_branch(), "develop");
}

#[test]
fn test_merge_primary_on_different_branch_dirty() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Make main and develop diverge - modify file.txt on main
    fs::write(repo.root_path().join("file.txt"), "main version").expect("Failed to modify file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "file.txt"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to add file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Update file on main"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to commit");

    // Create a develop branch from the previous commit (before the main update)
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    let output = cmd
        .args(["rev-parse", "HEAD~1"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to get previous commit");
    let base_commit = String::from_utf8_lossy(&output.stdout).trim().to_string();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["switch", "-c", "develop", &base_commit])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to create develop branch");

    // Modify file.txt in develop (uncommitted) to a different value
    // This will conflict when trying to switch to main
    fs::write(repo.root_path().join("file.txt"), "develop local changes")
        .expect("Failed to modify file");

    // Create a feature worktree and make a commit
    let feature_wt = repo.add_worktree("feature-dirty-primary", "feature-dirty-primary");
    fs::write(feature_wt.join("feature.txt"), "feature content").expect("Failed to write file");

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

    // Try to merge to main - should fail because primary has uncommitted changes that conflict
    snapshot_merge(
        "merge_primary_on_different_branch_dirty",
        &repo,
        &["main"],
        Some(&feature_wt),
    );
}

#[test]
fn test_merge_race_condition_commit_after_push() {
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
    fs::write(feature_wt.join("feature.txt"), "feature content").expect("Failed to write file");

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

    // Merge to main (this pushes the branch to main)
    snapshot_merge(
        "merge_race_condition_before_new_commit",
        &repo,
        &["main", "--no-remove"],
        Some(&feature_wt),
    );

    // RACE CONDITION: Simulate another developer adding a commit to the feature branch
    // after the merge/push but before worktree removal and branch deletion.
    // Since feature is already checked out in feature_wt, we'll add the commit directly there.
    fs::write(feature_wt.join("extra.txt"), "race condition commit").expect("Failed to write file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "extra.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add extra file (race condition)"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Now simulate what wt merge would do: remove the worktree
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["worktree", "remove", feature_wt.to_str().unwrap()])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to remove worktree");

    // Try to delete the branch with -d (safe delete)
    // This should FAIL because the branch has the race condition commit not in main
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    let output = cmd
        .args(["branch", "-d", "feature"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to run git branch -d");

    // Verify the deletion failed (non-zero exit code)
    assert!(
        !output.status.success(),
        "git branch -d should fail when branch has unmerged commits"
    );

    // Verify the error message mentions the branch is not fully merged
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not fully merged") || stderr.contains("not merged"),
        "Error should mention branch is not fully merged, got: {}",
        stderr
    );

    // Verify the branch still exists (wasn't deleted)
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    let output = cmd
        .args(["branch", "--list", "feature"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to list branches");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("feature"),
        "Branch should still exist after failed deletion"
    );
}

#[test]
fn test_merge_to_non_default_target() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Switch back to main and add a commit there
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["switch", "main"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to switch to main");

    std::fs::write(repo.root_path().join("main-file.txt"), "main content")
        .expect("Failed to write main file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "main-file.txt"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to add main file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add main-specific file"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to commit to main");

    // Create a staging branch from BEFORE the main commit
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    let output = cmd
        .args(["rev-parse", "HEAD~1"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to get parent commit");
    let base_commit = String::from_utf8_lossy(&output.stdout).trim().to_string();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["switch", "-c", "staging", &base_commit])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to create staging branch");

    // Add a commit to staging to make it different from main
    std::fs::write(repo.root_path().join("staging-file.txt"), "staging content")
        .expect("Failed to write staging file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "staging-file.txt"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to add staging file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add staging-specific file"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to commit to staging");

    // Create a worktree for staging
    let staging_wt = repo
        .root_path()
        .parent()
        .unwrap()
        .join("test-repo.staging-wt");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["worktree", "add", staging_wt.to_str().unwrap(), "staging"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to add staging worktree");

    // Create a feature worktree from the base commit (before both main and staging diverged)
    let feature_wt = repo
        .root_path()
        .parent()
        .unwrap()
        .join("test-repo.feature-for-staging");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args([
        "worktree",
        "add",
        feature_wt.to_str().unwrap(),
        "-b",
        "feature-for-staging",
        &base_commit,
    ])
    .current_dir(repo.root_path())
    .output()
    .expect("Failed to add feature worktree");

    std::fs::write(feature_wt.join("feature.txt"), "feature content")
        .expect("Failed to write feature file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "feature.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add feature file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Add feature for staging"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit feature");

    // Merge to staging explicitly (NOT to main)
    // This should rebase onto staging (which has staging-file.txt)
    // NOT onto main (which has main-file.txt)
    snapshot_merge(
        "merge_to_non_default_target",
        &repo,
        &["staging"],
        Some(&feature_wt),
    );
}

#[test]
fn test_merge_squash_with_working_tree_creates_backup() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create a worktree for main
    repo.add_main_worktree();

    // Create a feature worktree with multiple commits
    let feature_wt = repo.add_worktree("feature", "feature");

    // First commit
    std::fs::write(feature_wt.join("file1.txt"), "content 1").expect("Failed to write file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "file1.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "feat: add file 1"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Second commit
    std::fs::write(feature_wt.join("file2.txt"), "content 2").expect("Failed to write file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "file2.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "feat: add file 2"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Add uncommitted tracked changes that will be included in the squash
    std::fs::write(feature_wt.join("file1.txt"), "updated content 1")
        .expect("Failed to write file");

    // Merge with squash (default behavior)
    // This should create a backup before squashing because there are uncommitted changes
    snapshot_merge_with_env(
        "merge_squash_with_working_tree_creates_backup",
        &repo,
        &["main"],
        Some(&feature_wt),
        &[
            ("WORKTRUNK_COMMIT_GENERATION__COMMAND", "echo"),
            ("WORKTRUNK_COMMIT_GENERATION__ARGS", "fix: update files"),
        ],
    );

    // Verify that a backup was created in the reflog
    // Note: The worktree has been removed by the merge, so we check from the repo root
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    let output = cmd
        .args(["reflog", "show", "refs/wt-backup/feature"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to show reflog");

    let reflog = String::from_utf8_lossy(&output.stdout);
    assert!(
        reflog.contains("feature → main (squash)"),
        "Expected backup in reflog, but reflog was: {}",
        reflog
    );
}

#[test]
fn test_merge_when_default_branch_missing_worktree() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");
    // Move primary off default branch so no worktree holds it
    repo.switch_primary_to("develop");

    snapshot_merge("merge_default_branch_missing_worktree", &repo, &[], None);
}
