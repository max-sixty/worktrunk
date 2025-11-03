use crate::common::{TestRepo, make_snapshot_cmd, setup_snapshot_settings};
use insta_cmd::assert_cmd_snapshot;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Helper to create snapshot with normalized paths
fn snapshot_merge(test_name: &str, repo: &TestRepo, args: &[&str], cwd: Option<&std::path::Path>) {
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(repo, "merge", args, cwd);
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

#[test]
fn test_merge_fast_forward() {
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

    // Merge feature into main
    snapshot_merge("merge_fast_forward", &repo, &["main"], Some(&feature_wt));
}

#[test]
fn test_merge_with_no_remove_flag() {
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

    // Configure mock LLM command via environment variable
    // Use sh to consume stdin and return a fixed message
    // (squashing is now the default, no need for --squash flag)
    snapshot_merge_with_env(
        "merge_squash_with_llm",
        &repo,
        &["main"],
        Some(&feature_wt),
        &[
            ("WORKTRUNK_COMMIT_GENERATION__COMMAND", "sh"),
            ("WORKTRUNK_COMMIT_GENERATION__ARGS__0", "-c"),
            (
                "WORKTRUNK_COMMIT_GENERATION__ARGS__1",
                "cat >/dev/null && echo 'feat: implement user authentication system'",
            ),
        ],
    );
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

    // Merge (squashing is default) - should fail with helpful error about no net changes
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

    // Merge with LLM configured - should auto-commit with LLM-generated message
    snapshot_merge_with_env(
        "merge_auto_commit_with_llm",
        &repo,
        &["main"],
        Some(&feature_wt),
        &[
            ("WORKTRUNK_COMMIT_GENERATION__COMMAND", "echo"),
            (
                "WORKTRUNK_COMMIT_GENERATION__ARGS",
                "fix: improve auth validation logic",
            ),
        ],
    );
}

#[test]
fn test_merge_auto_commit_and_squash() {
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

    // Add uncommitted tracked changes
    std::fs::write(feature_wt.join("file1.txt"), "updated content 1")
        .expect("Failed to write file");

    // Merge (squashing is default) - should stage uncommitted changes, then squash all commits including the staged changes
    snapshot_merge_with_env(
        "merge_auto_commit_and_squash",
        &repo,
        &["main"],
        Some(&feature_wt),
        &[
            ("WORKTRUNK_COMMIT_GENERATION__COMMAND", "echo"),
            // Message is for the final squash commit
            (
                "WORKTRUNK_COMMIT_GENERATION__ARGS",
                "fix: update file 1 content",
            ),
        ],
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
fn test_merge_pre_merge_command_stdout_stderr_ordering() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Get path to the test fixture script
    let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let script_path = fixtures_dir.join("mixed-output.sh");

    // Create project config with two named pre-merge commands that both output to stdout and stderr
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create config dir");
    fs::write(
        config_dir.join("wt.toml"),
        format!(
            r#"
[pre-merge-command]
check1 = "{} check1 3"
check2 = "{} check2 3"
"#,
            script_path.display(),
            script_path.display()
        ),
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

    // Merge with --force - verify output ordering
    // Expected behavior in real usage (with 2>&1):
    //   check1 header → all check1 output → check2 header → all check2 output
    //
    // Note: The snapshot separates stdout/stderr for display, but in practice
    // they're properly interleaved when both go to the terminal.
    // The important verification is that within each stream, the ordering is correct.
    snapshot_merge(
        "merge_pre_merge_command_stdout_stderr_ordering",
        &repo,
        &["main", "--force"],
        Some(&feature_wt),
    );
}

#[test]
fn test_merge_pre_merge_command_combined_output() {
    use crate::common::run_with_combined_output;

    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create project config with two named pre-merge commands that output to BOTH stdout and stderr
    // This verifies the ordering is: header1 → command1 → stdout1 → stderr1 → header2 → command2 → stdout2 → stderr2
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create config dir");
    fs::write(
        config_dir.join("wt.toml"),
        r#"
[pre-merge-command]
first = "echo 'STDOUT-FROM-FIRST' && echo 'STDERR-FROM-FIRST' >&2"
second = "echo 'STDOUT-FROM-SECOND' && echo 'STDERR-FROM-SECOND' >&2"
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

    // Run merge and capture combined output
    let output = run_with_combined_output(&repo, "merge", &["main", "--force"], Some(&feature_wt));

    let settings = crate::common::setup_snapshot_settings(&repo);
    settings.bind(|| {
        insta::assert_snapshot!("merge_pre_merge_command_combined_output", output);
    });
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
        r#"post-merge-command = "echo 'merged {branch} to {target}' > post-merge-ran.txt""#,
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
notify = "echo 'Merge to {target} complete' > notify.txt"
deploy = "echo 'Deploying branch {branch}' > deploy.txt"
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
/// Source: tests/snapshots/integration__integration_tests__merge__readme_example_simple.snap
#[test]
fn test_readme_example_simple() {
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

    // Create a fix-auth worktree and make a commit
    let feature_wt = repo.add_worktree("fix-auth", "fix-auth");
    std::fs::write(feature_wt.join("auth.rs"), "// JWT validation code")
        .expect("Failed to write file");

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

    // Merge fix-auth into main
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

    // Make scripts executable
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
    std::fs::write(feature_wt.join("auth.rs"), "// Token refresh logic\n")
        .expect("Failed to write file");
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
    std::fs::write(feature_wt.join("jwt.rs"), "// JWT validation\n").expect("Failed to write file");
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
    std::fs::write(feature_wt.join("auth_test.rs"), "// Tests\n").expect("Failed to write file");
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
    // Set PATH to include .bin directory so mock cargo command is found
    let path_with_bin = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    snapshot_merge_with_env(
        "readme_example_complex",
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
        🔄 [36mMerging 1 commit to [1m[36mmain[0m @ [2m[SHA][0m (no commit/squash/rebase needed)

        * [SHA][33m ([m[1;36mHEAD[m[33m -> [m[1;32mfeature[m[33m)[m Add feature file

         feature.txt | 1 [32m+[m
         1 file changed, 1 insertion(+)

        ✅ [32mMerged to [1m[32mmain[0m[0m (1 commit, 1 file, [32m+1[0m)
        💡 [2mUse 'wt remove' to remove worktree[0m

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

        ----- stderr -----
        ❌ [31mWorking tree has uncommitted changes[0m

        💡 [2mCommit or stash them first[0m
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
        🔄 [36mMerging 1 commit to [1m[36mmain[0m @ [2m[SHA][0m (no commit/squash/rebase needed)

        * [SHA][33m ([m[1;36mHEAD[m[33m -> [m[1;32mfeature[m[33m)[m Add feature file

         feature.txt | 1 [32m+[m
         1 file changed, 1 insertion(+)

        ✅ [32mMerged to [1m[32mmain[0m[0m (1 commit, 1 file, [32m+1[0m)
        💡 [2mUse 'wt remove' to remove worktree[0m

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
