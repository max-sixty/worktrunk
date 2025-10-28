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
fn test_merge_with_keep_flag() {
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

    // Merge with --keep flag (should not finish worktree)
    snapshot_merge(
        "merge_with_keep",
        &repo,
        &["main", "--keep"],
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

    // Merge with squash (no LLM configured - should use deterministic message)
    snapshot_merge(
        "merge_squash_deterministic",
        &repo,
        &["main", "--squash"],
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
    snapshot_merge_with_env(
        "merge_squash_with_llm",
        &repo,
        &["main", "--squash"],
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
    snapshot_merge_with_env(
        "merge_squash_llm_fallback",
        &repo,
        &["main", "--squash"],
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

    // Merge with squash - should skip squashing since there's only one commit
    snapshot_merge(
        "merge_squash_single_commit",
        &repo,
        &["main", "--squash"],
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
fn test_merge_auto_commit_with_message() {
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
    std::fs::write(feature_wt.join("feature.txt"), "initial").expect("Failed to write file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "feature.txt"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to add file");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "feat: start feature"])
        .current_dir(&feature_wt)
        .output()
        .expect("Failed to commit");

    // Add uncommitted tracked changes
    std::fs::write(feature_wt.join("feature.txt"), "completed feature")
        .expect("Failed to write file");

    // Merge with -m flag (custom instruction) and LLM configured
    snapshot_merge_with_env(
        "merge_auto_commit_with_message",
        &repo,
        &["main", "-m", "Complete the user dashboard feature"],
        Some(&feature_wt),
        &[
            ("WORKTRUNK_COMMIT_GENERATION__COMMAND", "echo"),
            (
                "WORKTRUNK_COMMIT_GENERATION__ARGS",
                "feat: complete user dashboard",
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

    // Merge with --squash - should auto-commit first, then squash all commits including the new one
    snapshot_merge_with_env(
        "merge_auto_commit_and_squash",
        &repo,
        &["main", "--squash"],
        Some(&feature_wt),
        &[
            ("WORKTRUNK_COMMIT_GENERATION__COMMAND", "echo"),
            // First message is for auto-commit
            (
                "WORKTRUNK_COMMIT_GENERATION__ARGS",
                "fix: update file 1 content",
            ),
        ],
    );
}

#[test]
fn test_merge_pre_merge_check_success() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create project config with pre-merge check
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create config dir");
    fs::write(config_dir.join("wt.toml"), r#"pre-merge-check = "exit 0""#)
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
        "merge_pre_merge_check_success",
        &repo,
        &["main", "--force"],
        Some(&feature_wt),
    );
}

#[test]
fn test_merge_pre_merge_check_failure() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create project config with failing pre-merge check
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create config dir");
    fs::write(config_dir.join("wt.toml"), r#"pre-merge-check = "exit 1""#)
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

    // Merge with --force - pre-merge check should fail and block merge
    snapshot_merge(
        "merge_pre_merge_check_failure",
        &repo,
        &["main", "--force"],
        Some(&feature_wt),
    );
}

#[test]
fn test_merge_pre_merge_check_no_hooks() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create project config with failing pre-merge check
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create config dir");
    fs::write(config_dir.join("wt.toml"), r#"pre-merge-check = "exit 1""#)
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

    // Merge with --no-hooks - should skip pre-merge checks and succeed
    snapshot_merge(
        "merge_pre_merge_check_no_hooks",
        &repo,
        &["main", "--no-hooks"],
        Some(&feature_wt),
    );
}

#[test]
fn test_merge_pre_merge_check_named() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create project config with named pre-merge checks
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create config dir");
    fs::write(
        config_dir.join("wt.toml"),
        r#"
[pre-merge-check]
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

    // Merge with --force - all pre-merge checks should pass
    snapshot_merge(
        "merge_pre_merge_check_named",
        &repo,
        &["main", "--force"],
        Some(&feature_wt),
    );
}

#[test]
fn test_merge_pre_merge_check_stdout_stderr_ordering() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Get path to the test fixture script
    let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let script_path = fixtures_dir.join("mixed-output.sh");

    // Create project config with two named pre-merge checks that both output to stdout and stderr
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create config dir");
    fs::write(
        config_dir.join("wt.toml"),
        format!(
            r#"
[pre-merge-check]
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
        "merge_pre_merge_check_stdout_stderr_ordering",
        &repo,
        &["main", "--force"],
        Some(&feature_wt),
    );
}

#[test]
fn test_merge_pre_merge_check_combined_output() {
    use crate::common::run_with_combined_output;

    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    // Create project config with two named pre-merge checks that output to BOTH stdout and stderr
    // This verifies the ordering is: header1 → command1 → stdout1 → stderr1 → header2 → command2 → stdout2 → stderr2
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create config dir");
    fs::write(
        config_dir.join("wt.toml"),
        r#"
[pre-merge-check]
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
        insta::assert_snapshot!("merge_pre_merge_check_combined_output", output);
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
