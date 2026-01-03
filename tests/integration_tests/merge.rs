use crate::common::{
    TestRepo, make_snapshot_cmd, merge_scenario, repo, repo_with_alternate_primary,
    repo_with_feature_worktree, repo_with_main_worktree, repo_with_multi_commit_feature,
    setup_snapshot_settings,
};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::fs;
use std::path::{Path, PathBuf};

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

#[rstest]
fn test_merge_fast_forward(merge_scenario: (TestRepo, PathBuf)) {
    let (repo, feature_wt) = merge_scenario;

    // Merge feature into main
    snapshot_merge("merge_fast_forward", &repo, &["main"], Some(&feature_wt));
}

/// Test merge when running as a git subcommand (`git wt merge` instead of `wt merge`).
///
/// When git runs a subcommand, it sets `GIT_EXEC_PATH` in the environment.
/// Shell integration cannot work in this case because cd directives cannot
/// propagate through git's subprocess to the parent shell.
#[rstest]
fn test_merge_as_git_subcommand(merge_scenario: (TestRepo, PathBuf)) {
    let (repo, feature_wt) = merge_scenario;

    // Merge with GIT_EXEC_PATH set (simulating `git wt merge ...`)
    snapshot_merge_with_env(
        "merge_as_git_subcommand",
        &repo,
        &["main"],
        Some(&feature_wt),
        &[("GIT_EXEC_PATH", "/usr/lib/git-core")],
    );
}

#[rstest]
fn test_merge_when_primary_not_on_default_but_default_has_worktree(
    mut repo_with_alternate_primary: TestRepo,
) {
    let repo = &mut repo_with_alternate_primary;
    let feature_wt = repo.add_feature();

    snapshot_merge(
        "merge_when_primary_not_on_default_but_default_has_worktree",
        repo,
        &["main"],
        Some(&feature_wt),
    );
}

#[rstest]
fn test_merge_with_no_remove_flag(merge_scenario: (TestRepo, PathBuf)) {
    let (repo, feature_wt) = merge_scenario;

    // Merge with --no-remove flag (should not finish worktree)
    snapshot_merge(
        "merge_with_no_remove",
        &repo,
        &["main", "--no-remove"],
        Some(&feature_wt),
    );
}

#[rstest]
fn test_merge_already_on_target(repo: TestRepo) {
    // Already on main branch (repo root)
    snapshot_merge("merge_already_on_target", &repo, &[], None);
}

#[rstest]
fn test_merge_dirty_working_tree(mut repo: TestRepo) {
    // Create a feature worktree with uncommitted changes
    let feature_wt = repo.add_worktree("feature");
    std::fs::write(feature_wt.join("dirty.txt"), "uncommitted content").unwrap();

    // Try to merge (should fail due to dirty working tree)
    snapshot_merge(
        "merge_dirty_working_tree",
        &repo,
        &["main"],
        Some(&feature_wt),
    );
}

#[rstest]
fn test_merge_not_fast_forward(mut repo: TestRepo) {
    // Create commits in both branches
    // Add commit to main (repo root)
    std::fs::write(repo.root_path().join("main.txt"), "main content").unwrap();

    repo.run_git(&["add", "main.txt"]);
    repo.run_git(&["commit", "-m", "Add main file"]);

    // Create a feature worktree branching from before the main commit
    let feature_wt = repo.add_feature();

    // Try to merge (should fail or require actual merge)
    snapshot_merge(
        "merge_not_fast_forward",
        &repo,
        &["main"],
        Some(&feature_wt),
    );
}

/// Test that `wt merge --no-commit` shows merge-context hint when main has newer commits.
/// The --no-commit flag skips the rebase step, so the push fails with not-fast-forward error.
/// The hint should say "Run 'wt merge' again" (not "Use 'wt merge'").
#[rstest]
fn test_merge_no_commit_not_fast_forward(repo: TestRepo) {
    // Get the initial commit SHA to create feature branch from there
    let initial_sha = String::from_utf8_lossy(
        &repo
            .git_command()
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .trim()
    .to_string();

    // Add commit to main (this advances main beyond the initial commit)
    std::fs::write(repo.root_path().join("main.txt"), "main content").unwrap();
    repo.run_git(&["add", "main.txt"]);
    repo.run_git(&["commit", "-m", "Add main file"]);

    // Create feature worktree from the INITIAL commit (before main advanced)
    let feature_path = repo.root_path().parent().unwrap().join("feature");
    repo.run_git(&[
        "worktree",
        "add",
        "-b",
        "feature",
        feature_path.to_str().unwrap(),
        &initial_sha,
    ]);

    // Add a commit on feature branch
    std::fs::write(feature_path.join("feature.txt"), "feature content").unwrap();
    repo.run_git_in(&feature_path, &["add", "feature.txt"]);
    repo.run_git_in(&feature_path, &["commit", "-m", "Add feature file"]);

    // Try to merge with --no-commit --no-remove (skips rebase, so push fails with not-fast-forward)
    // Main has "Add main file" commit that feature doesn't have as ancestor
    snapshot_merge(
        "merge_no_commit_not_fast_forward",
        &repo,
        &["main", "--no-commit", "--no-remove"],
        Some(&feature_path),
    );
}

#[rstest]
fn test_merge_rebase_conflict(repo: TestRepo) {
    // Create a shared file
    std::fs::write(repo.root_path().join("shared.txt"), "initial content\n").unwrap();
    repo.run_git(&["add", "shared.txt"]);
    repo.commit("Add shared file");

    // Modify shared.txt in main branch (from the base commit)
    std::fs::write(repo.root_path().join("shared.txt"), "main version\n").unwrap();
    repo.run_git(&["add", "shared.txt"]);
    repo.run_git(&["commit", "-m", "Update shared.txt in main"]);

    // Create a feature worktree branching from before the main commit
    // We need to create it from the original commit, not current main
    let base_commit = String::from_utf8_lossy(
        &repo
            .git_command()
            .args(["rev-parse", "HEAD~1"])
            .output()
            .unwrap()
            .stdout,
    )
    .trim()
    .to_string();

    let feature_wt = repo.root_path().parent().unwrap().join("repo.feature");
    repo.run_git(&[
        "worktree",
        "add",
        feature_wt.to_str().unwrap(),
        "-b",
        "feature",
        &base_commit,
    ]);

    // Modify the same file with conflicting content
    std::fs::write(feature_wt.join("shared.txt"), "feature version\n").unwrap();
    repo.run_git_in(&feature_wt, &["add", "shared.txt"]);
    repo.run_git_in(
        &feature_wt,
        &["commit", "-m", "Update shared.txt in feature"],
    );

    // Try to merge - should fail with rebase conflict
    snapshot_merge("merge_rebase_conflict", &repo, &["main"], Some(&feature_wt));
}

#[rstest]
fn test_merge_to_default_branch(merge_scenario: (TestRepo, PathBuf)) {
    let (repo, feature_wt) = merge_scenario;

    // Merge without specifying target (should use default branch)
    snapshot_merge("merge_to_default", &repo, &[], Some(&feature_wt));
}

#[rstest]
fn test_merge_with_caret_symbol(merge_scenario: (TestRepo, PathBuf)) {
    let (repo, feature_wt) = merge_scenario;

    // Merge using ^ symbol (should resolve to default branch)
    snapshot_merge("merge_with_caret", &repo, &["^"], Some(&feature_wt));
}

#[rstest]
fn test_merge_error_detached_head(repo: TestRepo) {
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

#[rstest]
fn test_merge_squash_deterministic(mut repo_with_main_worktree: TestRepo) {
    let repo = &mut repo_with_main_worktree;
    // Create a feature worktree and make multiple commits
    let feature_wt = repo.add_worktree("feature");
    repo.commit_in_worktree(&feature_wt, "file1.txt", "content 1", "feat: add file 1");
    repo.commit_in_worktree(&feature_wt, "file2.txt", "content 2", "fix: update logic");
    repo.commit_in_worktree(&feature_wt, "file3.txt", "content 3", "docs: update readme");

    // Merge (squashing is now the default - no LLM configured, should use deterministic message)
    snapshot_merge(
        "merge_squash_deterministic",
        repo,
        &["main"],
        Some(&feature_wt),
    );
}

#[rstest]
fn test_merge_squash_with_llm(mut repo_with_main_worktree: TestRepo) {
    let repo = &mut repo_with_main_worktree;
    // Create a feature worktree and make multiple commits
    let feature_wt = repo.add_worktree("feature");
    repo.commit_in_worktree(
        &feature_wt,
        "auth.txt",
        "auth module",
        "feat: add authentication",
    );
    repo.commit_in_worktree(
        &feature_wt,
        "auth.txt",
        "auth module updated",
        "fix: handle edge case",
    );

    // Configure mock LLM command via config file
    // Use sh to consume stdin and return a fixed message
    let worktrunk_config = r#"
[commit-generation]
command = "sh"
args = ["-c", "cat >/dev/null && echo 'feat: implement user authentication system'"]
"#;
    fs::write(repo.test_config_path(), worktrunk_config).unwrap();

    // (squashing is now the default, no need for --squash flag)
    snapshot_merge("merge_squash_with_llm", repo, &["main"], Some(&feature_wt));
}

#[rstest]
fn test_merge_squash_llm_command_not_found(mut repo_with_main_worktree: TestRepo) {
    let repo = &mut repo_with_main_worktree;
    // Create a feature worktree and make multiple commits
    let feature_wt = repo.add_worktree("feature");
    repo.commit_in_worktree(&feature_wt, "file1.txt", "content 1", "feat: new feature");
    repo.commit_in_worktree(&feature_wt, "file2.txt", "content 2", "fix: bug fix");

    // Configure LLM command that doesn't exist - should error
    snapshot_merge_with_env(
        "merge_squash_llm_command_not_found",
        repo,
        &["main"],
        Some(&feature_wt),
        &[(
            "WORKTRUNK_COMMIT_GENERATION__COMMAND",
            "nonexistent-llm-command",
        )],
    );
}

#[rstest]
fn test_merge_squash_llm_error(mut repo_with_main_worktree: TestRepo) {
    let repo = &mut repo_with_main_worktree;
    // Test that LLM command errors show proper gutter formatting with full command

    // Create a feature worktree and make commits
    let feature_wt = repo.add_worktree("feature");
    repo.commit_in_worktree(&feature_wt, "file1.txt", "content 1", "feat: new feature");
    repo.commit_in_worktree(&feature_wt, "file2.txt", "content 2", "fix: bug fix");

    // Configure LLM command via config file with command that will fail
    // This tests that:
    // 1. The full command (with args) is shown in the error header
    // 2. The error output appears in a gutter
    // Note: We consume stdin first to avoid race condition where stdin write fails
    // before stderr is captured (broken pipe if process exits before reading stdin)
    let worktrunk_config = r#"
[commit-generation]
command = "sh"
args = ["-c", "cat > /dev/null; echo 'Error: connection refused' >&2 && exit 1"]
"#;
    fs::write(repo.test_config_path(), worktrunk_config).unwrap();

    snapshot_merge("merge_squash_llm_error", repo, &["main"], Some(&feature_wt));
}

#[rstest]
fn test_merge_squash_single_commit(mut repo_with_main_worktree: TestRepo) {
    let repo = &mut repo_with_main_worktree;
    // Create a feature worktree with only one commit
    let feature_wt =
        repo.add_worktree_with_commit("feature", "file1.txt", "content", "feat: single commit");

    // Merge (squashing is default) - should skip squashing since there's only one commit
    snapshot_merge(
        "merge_squash_single_commit",
        repo,
        &["main"],
        Some(&feature_wt),
    );
}

#[rstest]
fn test_merge_no_squash(repo_with_multi_commit_feature: TestRepo) {
    let repo = &repo_with_multi_commit_feature;
    let feature_wt = &repo.worktrees["feature"];

    // Merge with --no-squash - should NOT squash the commits
    snapshot_merge(
        "merge_no_squash",
        repo,
        &["main", "--no-squash"],
        Some(feature_wt),
    );
}

#[rstest]
fn test_merge_squash_empty_changes(mut repo_with_main_worktree: TestRepo) {
    let repo = &mut repo_with_main_worktree;
    // Create a feature worktree with commits that result in no net changes
    let feature_wt = repo.add_worktree("feature");

    // Get the initial content of file.txt (created by the initial commit)
    let file_path = feature_wt.join("file.txt");
    let initial_content = std::fs::read_to_string(&file_path).unwrap();

    // Commit 1: Modify file.txt
    repo.commit_in_worktree(&feature_wt, "file.txt", "change1", "Change 1");

    // Commit 2: Modify file.txt again
    repo.commit_in_worktree(&feature_wt, "file.txt", "change2", "Change 2");

    // Commit 3: Revert to original content
    repo.commit_in_worktree(
        &feature_wt,
        "file.txt",
        &initial_content,
        "Revert to initial",
    );

    // Merge (squashing is default) - should succeed even when commits result in no net changes
    snapshot_merge(
        "merge_squash_empty_changes",
        repo,
        &["main"],
        Some(&feature_wt),
    );
}

#[rstest]
fn test_merge_auto_commit_deterministic(mut repo_with_main_worktree: TestRepo) {
    let repo = &mut repo_with_main_worktree;
    // Create a feature worktree with a commit
    let feature_wt = repo.add_worktree_with_commit(
        "feature",
        "feature.txt",
        "initial content",
        "feat: initial feature",
    );

    // Now add uncommitted tracked changes
    std::fs::write(feature_wt.join("feature.txt"), "modified content").unwrap();

    // Merge - should auto-commit with deterministic message (no LLM configured)
    snapshot_merge(
        "merge_auto_commit_deterministic",
        repo,
        &["main"],
        Some(&feature_wt),
    );
}

#[rstest]
fn test_merge_auto_commit_with_llm(mut repo_with_main_worktree: TestRepo) {
    let repo = &mut repo_with_main_worktree;
    // Create a feature worktree with a commit
    let feature_wt = repo.add_worktree_with_commit(
        "feature",
        "auth.txt",
        "initial auth",
        "feat: add authentication",
    );

    // Now add uncommitted tracked changes
    std::fs::write(feature_wt.join("auth.txt"), "improved auth with validation").unwrap();

    // Configure mock LLM command via config file
    // Use sh to consume stdin and return a fixed message (must consume stdin for cross-platform compatibility)
    let worktrunk_config = r#"
[commit-generation]
command = "sh"
args = ["-c", "cat >/dev/null && echo 'fix: improve auth validation logic'"]
"#;
    fs::write(repo.test_config_path(), worktrunk_config).unwrap();

    // Merge with LLM configured - should auto-commit with LLM commit message
    snapshot_merge(
        "merge_auto_commit_with_llm",
        repo,
        &["main"],
        Some(&feature_wt),
    );
}

#[rstest]
fn test_merge_auto_commit_and_squash(repo_with_multi_commit_feature: TestRepo) {
    let repo = &repo_with_multi_commit_feature;
    let feature_wt = &repo.worktrees["feature"];

    // Add uncommitted tracked changes
    std::fs::write(feature_wt.join("file1.txt"), "updated content 1").unwrap();

    // Configure mock LLM command via config file
    // Use sh to consume stdin and return a fixed message (must consume stdin for cross-platform compatibility)
    let worktrunk_config = r#"
[commit-generation]
command = "sh"
args = ["-c", "cat >/dev/null && echo 'fix: update file 1 content'"]
"#;
    fs::write(repo.test_config_path(), worktrunk_config).unwrap();

    // Merge (squashing is default) - should stage uncommitted changes, then squash all commits including the staged changes
    snapshot_merge(
        "merge_auto_commit_and_squash",
        repo,
        &["main"],
        Some(feature_wt),
    );
}

#[rstest]
fn test_merge_with_untracked_files(mut repo_with_main_worktree: TestRepo) {
    let repo = &mut repo_with_main_worktree;
    // Create a feature worktree with one commit
    let feature_wt =
        repo.add_worktree_with_commit("feature", "file1.txt", "content 1", "feat: add file 1");

    // Add untracked files
    std::fs::write(feature_wt.join("untracked1.txt"), "untracked content 1").unwrap();
    std::fs::write(feature_wt.join("untracked2.txt"), "untracked content 2").unwrap();

    // Merge - should show warning about untracked files
    snapshot_merge_with_env(
        "merge_with_untracked_files",
        repo,
        &["main"],
        Some(&feature_wt),
        &[
            ("WORKTRUNK_COMMIT_GENERATION__COMMAND", "echo"),
            ("WORKTRUNK_COMMIT_GENERATION__ARGS", "fix: commit changes"),
        ],
    );
}

#[rstest]
fn test_merge_pre_merge_command_success(mut repo: TestRepo) {
    // Create project config with pre-merge command
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(config_dir.join("wt.toml"), r#"pre-merge = "exit 0""#).unwrap();

    repo.commit("Add config");

    let feature_wt = repo.add_feature();

    // Merge with --yes to skip approval prompts
    snapshot_merge(
        "merge_pre_merge_command_success",
        &repo,
        &["main", "--yes"],
        Some(&feature_wt),
    );
}

#[rstest]
fn test_merge_pre_merge_command_failure(mut repo: TestRepo) {
    // Create project config with failing pre-merge command
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(config_dir.join("wt.toml"), r#"pre-merge = "exit 1""#).unwrap();

    repo.commit("Add config");

    let feature_wt = repo.add_feature();

    // Merge with --yes - pre-merge command should fail and block merge
    snapshot_merge(
        "merge_pre_merge_command_failure",
        &repo,
        &["main", "--yes"],
        Some(&feature_wt),
    );
}

#[rstest]
fn test_merge_pre_merge_command_no_hooks(mut repo: TestRepo) {
    // Create project config with failing pre-merge command
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(config_dir.join("wt.toml"), r#"pre-merge = "exit 1""#).unwrap();

    repo.commit("Add config");

    let feature_wt = repo.add_feature();

    // Merge with --no-verify - should skip pre-merge commands and succeed
    snapshot_merge(
        "merge_pre_merge_command_no_hooks",
        &repo,
        &["main", "--no-verify"],
        Some(&feature_wt),
    );
}

#[rstest]
fn test_merge_pre_merge_command_named(mut repo: TestRepo) {
    // Create project config with named pre-merge commands
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("wt.toml"),
        r#"
[pre-merge]
format = "exit 0"
lint = "exit 0"
test = "exit 0"
"#,
    )
    .unwrap();

    repo.commit("Add config");

    let feature_wt = repo.add_feature();

    // Merge with --yes - all pre-merge commands should pass
    snapshot_merge(
        "merge_pre_merge_command_named",
        &repo,
        &["main", "--yes"],
        Some(&feature_wt),
    );
}

#[rstest]
fn test_merge_post_merge_command_success(mut repo: TestRepo) {
    // Create project config with post-merge command that writes a marker file
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("wt.toml"),
        r#"post-merge = "echo 'merged {{ branch }} to {{ target }}' > post-merge-ran.txt""#,
    )
    .unwrap();

    repo.commit("Add config");

    let feature_wt = repo.add_feature();

    // Merge with --yes
    snapshot_merge(
        "merge_post_merge_command_success",
        &repo,
        &["main", "--yes"],
        Some(&feature_wt),
    );

    // Verify the command ran in the main worktree (not the feature worktree)
    let marker_file = repo.root_path().join("post-merge-ran.txt");
    assert!(
        marker_file.exists(),
        "Post-merge command should have created marker file in main worktree"
    );
    let content = fs::read_to_string(&marker_file).unwrap();
    assert!(
        content.contains("merged feature to main"),
        "Marker file should contain correct branch and target: {}",
        content
    );
}

#[rstest]
fn test_merge_post_merge_command_skipped_with_no_verify(mut repo: TestRepo) {
    // Create project config with post-merge command that writes a marker file
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("wt.toml"),
        r#"post-merge = "echo 'merged {{ branch }} to {{ target }}' > post-merge-ran.txt""#,
    )
    .unwrap();

    repo.commit("Add config");

    let feature_wt = repo.add_feature();

    // Merge with --no-verify - hook should be skipped entirely
    snapshot_merge(
        "merge_post_merge_command_no_verify",
        &repo,
        &["main", "--yes", "--no-verify"],
        Some(&feature_wt),
    );

    // Verify the command did not run in the main worktree
    let marker_file = repo.root_path().join("post-merge-ran.txt");
    assert!(
        !marker_file.exists(),
        "Post-merge command should not run when --no-verify is set"
    );
}

#[rstest]
fn test_merge_post_merge_command_failure(mut repo: TestRepo) {
    // Create project config with failing post-merge command
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(config_dir.join("wt.toml"), r#"post-merge = "exit 1""#).unwrap();

    repo.commit("Add config");

    let feature_wt = repo.add_feature();

    // Merge with --yes - post-merge command should fail but merge should complete
    snapshot_merge(
        "merge_post_merge_command_failure",
        &repo,
        &["main", "--yes"],
        Some(&feature_wt),
    );
}

#[rstest]
fn test_merge_post_merge_command_named(mut repo: TestRepo) {
    // Create project config with named post-merge commands
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("wt.toml"),
        r#"
[post-merge]
notify = "echo 'Merge to {{ target }} complete' > notify.txt"
deploy = "echo 'Deploying branch {{ branch }}' > deploy.txt"
"#,
    )
    .unwrap();

    repo.commit("Add config");

    let feature_wt = repo.add_feature();

    // Merge with --yes
    snapshot_merge(
        "merge_post_merge_command_named",
        &repo,
        &["main", "--yes"],
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

#[rstest]
fn test_merge_post_merge_runs_with_nothing_to_merge(mut repo: TestRepo) {
    // Verify post-merge hooks run even when there's nothing to merge

    // Create project config with post-merge command
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("wt.toml"),
        r#"post-merge = "echo 'post-merge ran' > post-merge-ran.txt""#,
    )
    .unwrap();

    repo.commit("Add config");

    // Create a worktree for main (destination for post-merge commands)

    // Create a feature worktree with NO commits (already up-to-date with main)
    let feature_wt = repo.add_worktree("feature");

    // Merge with --yes - nothing to merge but post-merge should still run
    snapshot_merge(
        "merge_post_merge_runs_with_nothing_to_merge",
        &repo,
        &["main", "--yes"],
        Some(&feature_wt),
    );

    // Verify the post-merge command ran in the main worktree
    let marker_file = repo.root_path().join("post-merge-ran.txt");
    assert!(
        marker_file.exists(),
        "Post-merge command should run even when nothing to merge"
    );
}

#[rstest]
fn test_merge_post_merge_runs_from_main_branch(repo: TestRepo) {
    // Verify post-merge hooks run when merging from main to main (nothing to do)

    // Create project config with post-merge command
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("wt.toml"),
        r#"post-merge = "echo 'post-merge ran from main' > post-merge-ran.txt""#,
    )
    .unwrap();

    repo.commit("Add config");

    // Run merge from main branch (repo root) - nothing to merge
    snapshot_merge(
        "merge_post_merge_runs_from_main_branch",
        &repo,
        &["--yes"],
        None, // cwd = repo root = main branch
    );

    // Verify the post-merge command ran
    let marker_file = repo.root_path().join("post-merge-ran.txt");
    assert!(
        marker_file.exists(),
        "Post-merge command should run even when on main branch"
    );
}

#[rstest]
fn test_merge_pre_commit_command_success(mut repo: TestRepo) {
    // Create project config with pre-commit command
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("wt.toml"),
        r#"pre-commit = "echo 'Pre-commit check passed'""#,
    )
    .unwrap();

    repo.commit("Add config");

    // Create a feature worktree and make a change
    let feature_wt = repo.add_worktree("feature");
    fs::write(feature_wt.join("feature.txt"), "feature content").unwrap();

    // Merge with --yes (changes uncommitted, should trigger pre-commit hook)
    snapshot_merge(
        "merge_pre_commit_command_success",
        &repo,
        &["main", "--yes"],
        Some(&feature_wt),
    );
}

#[rstest]
fn test_merge_pre_commit_command_failure(mut repo: TestRepo) {
    // Create project config with failing pre-commit command
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(config_dir.join("wt.toml"), r#"pre-commit = "exit 1""#).unwrap();

    repo.commit("Add config");

    // Create a feature worktree and make a change
    let feature_wt = repo.add_worktree("feature");
    fs::write(feature_wt.join("feature.txt"), "feature content").unwrap();

    // Merge with --yes - pre-commit command should fail and block merge
    snapshot_merge(
        "merge_pre_commit_command_failure",
        &repo,
        &["main", "--yes"],
        Some(&feature_wt),
    );
}

#[rstest]
fn test_merge_pre_squash_command_success(mut repo: TestRepo) {
    // Create project config with pre-commit command (used for both squash and no-squash)
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("wt.toml"),
        "pre-commit = \"echo 'Pre-commit check passed'\"",
    )
    .unwrap();

    repo.commit("Add config");

    // Create a feature worktree and make commits
    let feature_wt = repo.add_feature();

    // Merge with --yes (squashing is now the default)
    snapshot_merge(
        "merge_pre_squash_command_success",
        &repo,
        &["main", "--yes"],
        Some(&feature_wt),
    );
}

#[rstest]
fn test_merge_pre_squash_command_failure(mut repo: TestRepo) {
    // Create project config with failing pre-commit command (used for both squash and no-squash)
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(config_dir.join("wt.toml"), r#"pre-commit = "exit 1""#).unwrap();

    repo.commit("Add config");

    let feature_wt = repo.add_feature();

    // Merge with --yes (squashing is default) - pre-commit command should fail and block merge
    snapshot_merge(
        "merge_pre_squash_command_failure",
        &repo,
        &["main", "--yes"],
        Some(&feature_wt),
    );
}

#[rstest]
fn test_merge_no_remote(#[from(repo_with_feature_worktree)] repo: TestRepo) {
    // Deliberately NOT calling setup_remote to test the error case
    let feature_wt = repo.worktree_path("feature");

    // Try to merge without specifying target (should fail - no remote to get default branch)
    snapshot_merge("merge_no_remote", &repo, &[], Some(feature_wt));
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
///
#[rstest]
fn test_readme_example_simple(repo: TestRepo) {
    // Snapshot the switch --create command (runs from bare repo)
    snapshot_switch(
        "readme_example_simple_switch",
        &repo,
        &["--create", "fix-auth"],
        None,
    );

    // Get the created worktree path and make a commit
    let feature_wt = repo.root_path().parent().unwrap().join("repo.fix-auth");
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
    std::fs::write(feature_wt.join("auth.rs"), auth_rs).unwrap();

    repo.run_git_in(&feature_wt, &["add", "auth.rs"]);
    repo.run_git_in(&feature_wt, &["commit", "-m", "Implement JWT validation"]);

    // Snapshot the merge command
    snapshot_merge("readme_example_simple", &repo, &["main"], Some(&feature_wt));
}

/// Generate README example: Complex merge with multiple hooks
/// This demonstrates advanced features - pre-merge hooks (tests, lints), post-merge hooks.
/// Shows the full power of worktrunk's automation capabilities.
///
/// Output is used in README.md "Advanced Features" or "Project Automation" section.
/// Source: tests/snapshots/integration__integration_tests__merge__readme_example_complex.snap
///
/// Skipped on Windows: Uses Unix shell commands (chmod, echo) for mock command scripts.
#[rstest]
#[cfg_attr(windows, ignore)]
fn test_readme_example_complex(mut repo: TestRepo) {
    // Create project config with multiple hooks
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();

    // Create mock commands for realistic output
    let bin_dir = repo.root_path().join(".bin");
    fs::create_dir_all(&bin_dir).unwrap();

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
    fs::write(bin_dir.join("cargo"), cargo_script).unwrap();

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
    fs::write(bin_dir.join("llm"), llm_script).unwrap();

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
[pre-merge]
"test" = "cargo test"
"lint" = "cargo clippy"

[post-merge]
"install" = "cargo install --path ."
"#;

    fs::write(config_dir.join("wt.toml"), config_content).unwrap();

    // Commit the config and mock cargo
    repo.run_git(&["add", ".config/wt.toml", ".bin"]);
    repo.run_git(&["commit", "-m", "Add project automation config"]);

    // Create a feature worktree and make multiple commits
    let feature_wt = repo.add_worktree("feature-auth");

    // First commit: token refresh
    let commit_one = r#"// Token refresh logic
pub fn refresh(secret: &str, expires_in: u32) -> String {
    format!("{}::{}", secret, expires_in)
}

pub fn needs_rotation(issued_at: u64, ttl: u64, now: u64) -> bool {
    now.saturating_sub(issued_at) > ttl
}
"#;
    std::fs::write(feature_wt.join("auth.rs"), commit_one).unwrap();
    repo.run_git_in(&feature_wt, &["add", "auth.rs"]);
    repo.run_git_in(&feature_wt, &["commit", "-m", "Add token refresh logic"]);

    // Second commit: JWT validation
    let commit_two = r#"// JWT validation
pub fn validate_signature(payload: &str, signature: &str) -> bool {
    !payload.is_empty() && signature.len() > 12
}

pub fn decode_claims(token: &str) -> Option<&str> {
    token.split('.').nth(1)
}
"#;
    std::fs::write(feature_wt.join("jwt.rs"), commit_two).unwrap();
    repo.run_git_in(&feature_wt, &["add", "jwt.rs"]);
    repo.run_git_in(&feature_wt, &["commit", "-m", "Implement JWT validation"]);

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
    std::fs::write(feature_wt.join("auth_test.rs"), commit_three).unwrap();
    repo.run_git_in(&feature_wt, &["add", "auth_test.rs"]);
    repo.run_git_in(&feature_wt, &["commit", "-m", "Add authentication tests"]);

    // Configure LLM in worktrunk config for deterministic, high-quality commit messages
    let llm_path = bin_dir.join("llm");
    let worktrunk_config = format!(
        r#"
[commit-generation]
command = "{}"
"#,
        llm_path.display()
    );
    fs::write(repo.test_config_path(), worktrunk_config).unwrap();

    // Merge with --yes to skip approval prompts for commands
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
        &["main", "--yes"],
        Some(&feature_wt),
        &[("PATH", &path_with_bin)],
    );
}

/// Generate README example: Creating worktree with post-create and post-start hooks
/// This demonstrates the hooks feature with realistic tool output (uv sync, dev server).
///
/// Output is used in README.md "Project Hooks" section.
/// Source: tests/snapshots/integration__integration_tests__merge__readme_example_hooks_post_create.snap
///
/// Skipped on Windows: Uses Unix shell commands (chmod, echo) for mock command scripts.
#[rstest]
#[cfg_attr(windows, ignore)]
fn test_readme_example_hooks_post_create(repo: TestRepo) {
    // Create project config with post-create and post-start hooks
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();

    // Create mock commands for realistic output
    let bin_dir = repo.root_path().join(".bin");
    fs::create_dir_all(&bin_dir).unwrap();

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
    fs::write(bin_dir.join("uv"), uv_script).unwrap();

    // Make scripts executable (Unix only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(bin_dir.join("uv")).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(bin_dir.join("uv"), perms).unwrap();
    }

    let config_content = r#"
[post-create]
"install" = "uv sync"

[post-start]
"dev" = "uv run dev"
"#;

    fs::write(config_dir.join("wt.toml"), config_content).unwrap();

    // Commit the config
    repo.run_git(&["add", ".config/wt.toml", ".bin"]);
    repo.run_git(&["commit", "-m", "Add project hooks"]);

    // Set PATH to include mock commands and run switch --create with --yes
    let path_with_bin = format!(
        "{}:/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        bin_dir.display()
    );

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "switch", &["--create", "feature-x", "--yes"], None);
        cmd.env("PATH", &path_with_bin);
        assert_cmd_snapshot!("readme_example_hooks_post_create", cmd);
    });
}

/// Generate README example: Merging with pre-merge hooks (test and lint)
/// This demonstrates the pre-merge hooks feature with realistic pytest and ruff output.
///
/// Output is used in README.md "Project Hooks" section.
/// Source: tests/snapshots/integration__integration_tests__merge__readme_example_hooks_pre_merge.snap
///
/// Skipped on Windows: Uses Unix shell commands (chmod, echo) for mock command scripts.
#[rstest]
#[cfg_attr(windows, ignore)]
fn test_readme_example_hooks_pre_merge(mut repo: TestRepo) {
    // Create project config with pre-merge hooks
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();

    // Create mock commands for realistic output
    let bin_dir = repo.root_path().join(".bin");
    fs::create_dir_all(&bin_dir).unwrap();

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
    fs::write(bin_dir.join("pytest"), pytest_script).unwrap();

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
    fs::write(bin_dir.join("ruff"), ruff_script).unwrap();

    // Mock llm command for commit message
    let llm_script = r#"#!/bin/sh
cat > /dev/null
cat << 'EOF'
feat(api): Add user authentication endpoints

Implement login and token refresh endpoints with JWT validation.
Includes comprehensive test coverage and input validation.
EOF
"#;
    fs::write(bin_dir.join("llm"), llm_script).unwrap();

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
    fs::write(bin_dir.join("uv"), uv_script).unwrap();

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
[pre-merge]
"test" = "uv run pytest"
"lint" = "uv run ruff check"
"#;

    fs::write(config_dir.join("wt.toml"), config_content).unwrap();

    // Commit the config
    repo.run_git(&["add", ".config/wt.toml", ".bin"]);
    repo.run_git(&["commit", "-m", "Add pre-merge hooks"]);

    // Create a feature worktree and make multiple commits
    let feature_wt = repo.add_worktree("feature-auth");

    // First commit - create initial auth.py with login endpoint
    fs::create_dir_all(feature_wt.join("api")).unwrap();
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
    std::fs::write(feature_wt.join("api/auth.py"), auth_py_v1).unwrap();
    repo.run_git_in(&feature_wt, &["add", "api/auth.py"]);
    repo.run_git_in(&feature_wt, &["commit", "-m", "Add login endpoint"]);

    // Second commit - add tests
    fs::create_dir_all(feature_wt.join("tests")).unwrap();
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
    std::fs::write(feature_wt.join("tests/test_auth.py"), test_auth_py).unwrap();
    repo.run_git_in(&feature_wt, &["add", "tests/test_auth.py"]);
    repo.run_git_in(&feature_wt, &["commit", "-m", "Add authentication tests"]);

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
    std::fs::write(feature_wt.join("api/auth.py"), auth_py_v2).unwrap();
    repo.run_git_in(&feature_wt, &["add", "api/auth.py"]);
    repo.run_git_in(&feature_wt, &["commit", "-m", "Add validation"]);

    // Configure LLM in worktrunk config
    let llm_path = bin_dir.join("llm");
    let worktrunk_config = format!(
        r#"
[commit-generation]
command = "{}"
"#,
        llm_path.display()
    );
    fs::write(repo.test_config_path(), worktrunk_config).unwrap();

    // Set PATH and merge with --yes
    let path_with_bin = format!(
        "{}:/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        bin_dir.display()
    );
    snapshot_merge_with_env(
        "readme_example_hooks_pre_merge",
        &repo,
        &["main", "--yes"],
        Some(&feature_wt),
        &[("PATH", &path_with_bin)],
    );
}

#[rstest]
fn test_merge_no_commit_with_clean_tree(mut repo_with_feature_worktree: TestRepo) {
    let repo = &mut repo_with_feature_worktree;
    let feature_wt = &repo.worktrees["feature"];

    // Merge with --no-commit (should succeed - clean tree)
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(
            repo,
            "merge",
            &["main", "--no-commit", "--no-remove"],
            Some(feature_wt),
        );
        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        [36m◎[39m [36mMerging 1 commit to [1mmain[22m @ [2mfc12499[22m (no commit/squash/rebase needed)[39m
        [107m [0m * [33mfc12499[m Add feature file
        [107m [0m  feature.txt | 1 [32m+[m
        [107m [0m  1 file changed, 1 insertion(+)
        [32m✓[39m [32mMerged to [1mmain[22m [90m(1 commit, 1 file, [32m+1[39m[39m[90m)[39m[39m
        [2m○[22m Worktree preserved (--no-remove)
        ");
    });
}

#[rstest]
fn test_merge_no_commit_with_dirty_tree(mut repo: TestRepo) {
    // Create a feature worktree with a commit
    let feature_wt = repo.add_worktree_with_commit(
        "feature",
        "committed.txt",
        "committed content",
        "Add committed file",
    );

    // Add uncommitted changes
    fs::write(feature_wt.join("uncommitted.txt"), "uncommitted content").unwrap();

    // Try to merge with --no-commit (should fail - dirty tree)
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd =
            make_snapshot_cmd(&repo, "merge", &["main", "--no-commit"], Some(&feature_wt));
        assert_cmd_snapshot!(cmd, @"
        success: false
        exit_code: 1
        ----- stdout -----

        ----- stderr -----
        [31m✗[39m [31mCannot merge with --no-commit: [1mfeature[22m has uncommitted changes[39m
        [2m↳[22m [2mCommit or stash changes first[22m
        ");
    });
}

#[rstest]
fn test_merge_no_commit_no_squash_no_remove_redundant(mut repo_with_feature_worktree: TestRepo) {
    let repo = &mut repo_with_feature_worktree;
    let feature_wt = &repo.worktrees["feature"];

    // Merge with --no-commit --no-squash --no-remove (redundant but valid - should succeed)
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(
            repo,
            "merge",
            &["main", "--no-commit", "--no-squash", "--no-remove"],
            Some(feature_wt),
        );
        assert_cmd_snapshot!(cmd, @r"
        success: true
        exit_code: 0
        ----- stdout -----

        ----- stderr -----
        [36m◎[39m [36mMerging 1 commit to [1mmain[22m @ [2mfc12499[22m (no commit/squash/rebase needed)[39m
        [107m [0m * [33mfc12499[m Add feature file
        [107m [0m  feature.txt | 1 [32m+[m
        [107m [0m  1 file changed, 1 insertion(+)
        [32m✓[39m [32mMerged to [1mmain[22m [90m(1 commit, 1 file, [32m+1[39m[39m[90m)[39m[39m
        [2m○[22m Worktree preserved (--no-remove)
        ");
    });
}

#[rstest]
fn test_merge_no_commits(mut repo_with_main_worktree: TestRepo) {
    let repo = &mut repo_with_main_worktree;

    // Create a feature worktree with NO commits (just branched from main)
    let feature_wt = repo.add_worktree("no-commits");

    // Merge without any commits - should skip both squashing and rebasing
    snapshot_merge("merge_no_commits", repo, &["main"], Some(&feature_wt));
}

#[rstest]
fn test_merge_no_commits_with_changes(mut repo_with_main_worktree: TestRepo) {
    let repo = &mut repo_with_main_worktree;

    // Create a feature worktree with NO commits but WITH uncommitted changes
    let feature_wt = repo.add_worktree("no-commits-dirty");
    fs::write(feature_wt.join("newfile.txt"), "new content").unwrap();

    // Merge - should commit the changes, skip squashing (only 1 commit), and skip rebasing (at merge base)
    snapshot_merge(
        "merge_no_commits_with_changes",
        repo,
        &["main"],
        Some(&feature_wt),
    );
}

#[rstest]
fn test_merge_rebase_fast_forward(mut repo: TestRepo) {
    // Test fast-forward case: branch has no commits, main moved ahead
    // Should show "Fast-forwarded to main" without progress message

    // Create a feature worktree with NO commits (just branched from main)
    let feature_wt = repo.add_worktree("fast-forward-test");

    // Advance main with a new commit (in the primary worktree which is on main)
    fs::write(repo.root_path().join("main-update.txt"), "main content").unwrap();
    repo.run_git(&["add", "main-update.txt"]);
    repo.run_git(&["commit", "-m", "Update main"]);

    // Merge - should fast-forward (no commits to replay)
    snapshot_merge(
        "merge_rebase_fast_forward",
        &repo,
        &["main"],
        Some(&feature_wt),
    );
}

#[rstest]
fn test_merge_rebase_true_rebase(mut repo: TestRepo) {
    // Test true rebase case: branch has commits and main moved ahead
    // Should show "Rebasing onto main..." progress message

    // Create a feature worktree with a commit
    let feature_wt = repo.add_worktree_with_commit(
        "true-rebase-test",
        "feature.txt",
        "feature content",
        "Add feature",
    );

    // Advance main with a new commit (in the primary worktree which is on main)
    fs::write(repo.root_path().join("main-update.txt"), "main content").unwrap();
    repo.run_git(&["add", "main-update.txt"]);
    repo.run_git(&["commit", "-m", "Update main"]);

    // Merge - should show rebasing progress (has commits to replay)
    snapshot_merge(
        "merge_rebase_true_rebase",
        &repo,
        &["main"],
        Some(&feature_wt),
    );
}

// =============================================================================
// --no-rebase tests
// =============================================================================

#[rstest]
fn test_merge_no_rebase_when_already_rebased(merge_scenario: (TestRepo, PathBuf)) {
    // Feature branch is based on main (no divergence), so --no-rebase should succeed
    let (repo, feature_wt) = merge_scenario;

    snapshot_merge(
        "merge_no_rebase_already_rebased",
        &repo,
        &["main", "--no-rebase"],
        Some(&feature_wt),
    );
}

#[rstest]
fn test_merge_no_rebase_when_not_rebased(mut repo: TestRepo) {
    // Create a feature worktree with a commit
    let feature_wt = repo.add_worktree_with_commit(
        "not-rebased-test",
        "feature.txt",
        "feature content",
        "Add feature",
    );

    // Advance main with a new commit (makes feature branch diverge)
    fs::write(repo.root_path().join("main-update.txt"), "main content").unwrap();
    repo.run_git(&["add", "main-update.txt"]);
    repo.run_git(&["commit", "-m", "Update main"]);

    // --no-rebase should fail because feature is not rebased onto main
    snapshot_merge(
        "merge_no_rebase_not_rebased",
        &repo,
        &["main", "--no-rebase"],
        Some(&feature_wt),
    );
}

#[rstest]
fn test_merge_primary_on_different_branch(mut repo: TestRepo) {
    repo.switch_primary_to("develop");
    assert_eq!(repo.current_branch(), "develop");

    // Create a feature worktree and make a commit
    let feature_wt = repo.add_worktree_with_commit(
        "feature-from-develop",
        "feature.txt",
        "feature content",
        "Add feature file",
    );

    snapshot_merge(
        "merge_primary_on_different_branch",
        &repo,
        &["main"],
        Some(&feature_wt),
    );

    // Verify primary stayed on develop (we don't switch branches, only worktrees)
    assert_eq!(repo.current_branch(), "develop");
}

#[rstest]
fn test_merge_primary_on_different_branch_dirty(mut repo: TestRepo) {
    // Make main and develop diverge - modify file.txt on main
    fs::write(repo.root_path().join("file.txt"), "main version").unwrap();
    repo.run_git(&["add", "file.txt"]);
    repo.run_git(&["commit", "-m", "Update file on main"]);

    // Create a develop branch from the previous commit (before the main update)
    let base_commit = String::from_utf8_lossy(
        &repo
            .git_command()
            .args(["rev-parse", "HEAD~1"])
            .output()
            .unwrap()
            .stdout,
    )
    .trim()
    .to_string();
    repo.run_git(&["switch", "-c", "develop", &base_commit]);

    // Modify file.txt in develop (uncommitted) to a different value
    // This will conflict when trying to switch to main
    fs::write(repo.root_path().join("file.txt"), "develop local changes").unwrap();

    // Create a feature worktree and make a commit
    let feature_wt = repo.add_worktree_with_commit(
        "feature-dirty-primary",
        "feature.txt",
        "feature content",
        "Add feature file",
    );

    // Try to merge to main - should fail because primary has uncommitted changes that conflict
    snapshot_merge(
        "merge_primary_on_different_branch_dirty",
        &repo,
        &["main"],
        Some(&feature_wt),
    );
}

#[rstest]
fn test_merge_race_condition_commit_after_push(mut repo_with_feature_worktree: TestRepo) {
    let repo = &mut repo_with_feature_worktree;
    let feature_wt = repo.worktrees["feature"].clone();

    // Merge to main (this pushes the branch to main)
    snapshot_merge(
        "merge_race_condition_before_new_commit",
        repo,
        &["main", "--no-remove"],
        Some(&feature_wt),
    );

    // RACE CONDITION: Simulate another developer adding a commit to the feature branch
    // after the merge/push but before worktree removal and branch deletion.
    // Since feature is already checked out in feature_wt, we'll add the commit directly there.
    fs::write(feature_wt.join("extra.txt"), "race condition commit").unwrap();
    repo.run_git_in(&feature_wt, &["add", "extra.txt"]);
    repo.run_git_in(
        &feature_wt,
        &["commit", "-m", "Add extra file (race condition)"],
    );

    // Now simulate what wt merge would do: remove the worktree
    repo.run_git(&["worktree", "remove", feature_wt.to_str().unwrap()]);

    // Try to delete the branch with -d (safe delete)
    // This should FAIL because the branch has the race condition commit not in main
    let output = repo
        .git_command()
        .args(["branch", "-d", "feature"])
        .output()
        .unwrap();

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
    let output = repo
        .git_command()
        .args(["branch", "--list", "feature"])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("feature"),
        "Branch should still exist after failed deletion"
    );
}

#[rstest]
fn test_merge_to_non_default_target(repo: TestRepo) {
    // Switch back to main and add a commit there
    repo.run_git(&["switch", "main"]);
    std::fs::write(repo.root_path().join("main-file.txt"), "main content").unwrap();
    repo.run_git(&["add", "main-file.txt"]);
    repo.run_git(&["commit", "-m", "Add main-specific file"]);

    // Create a staging branch from BEFORE the main commit
    let base_commit = String::from_utf8_lossy(
        &repo
            .git_command()
            .args(["rev-parse", "HEAD~1"])
            .output()
            .unwrap()
            .stdout,
    )
    .trim()
    .to_string();
    repo.run_git(&["switch", "-c", "staging", &base_commit]);

    // Add a commit to staging to make it different from main
    std::fs::write(repo.root_path().join("staging-file.txt"), "staging content").unwrap();
    repo.run_git(&["add", "staging-file.txt"]);
    repo.run_git(&["commit", "-m", "Add staging-specific file"]);

    // Switch back to main before creating the staging worktree
    repo.run_git(&["switch", "main"]);

    // Create a worktree for staging
    let staging_wt = repo.root_path().parent().unwrap().join("repo.staging-wt");
    repo.run_git(&["worktree", "add", staging_wt.to_str().unwrap(), "staging"]);

    // Create a feature worktree from the base commit (before both main and staging diverged)
    let feature_wt = repo
        .root_path()
        .parent()
        .unwrap()
        .join("repo.feature-for-staging");
    repo.run_git(&[
        "worktree",
        "add",
        feature_wt.to_str().unwrap(),
        "-b",
        "feature-for-staging",
        &base_commit,
    ]);

    std::fs::write(feature_wt.join("feature.txt"), "feature content").unwrap();
    repo.run_git_in(&feature_wt, &["add", "feature.txt"]);
    repo.run_git_in(&feature_wt, &["commit", "-m", "Add feature for staging"]);

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

#[rstest]
fn test_merge_squash_with_working_tree_creates_backup(mut repo_with_main_worktree: TestRepo) {
    let repo = &mut repo_with_main_worktree;

    // Create a feature worktree with multiple commits
    let feature_wt = repo.add_worktree("feature");

    // First commit
    std::fs::write(feature_wt.join("file1.txt"), "content 1").unwrap();
    repo.run_git_in(&feature_wt, &["add", "file1.txt"]);
    repo.run_git_in(&feature_wt, &["commit", "-m", "feat: add file 1"]);

    // Second commit
    std::fs::write(feature_wt.join("file2.txt"), "content 2").unwrap();
    repo.run_git_in(&feature_wt, &["add", "file2.txt"]);
    repo.run_git_in(&feature_wt, &["commit", "-m", "feat: add file 2"]);

    // Add uncommitted tracked changes that will be included in the squash
    std::fs::write(feature_wt.join("file1.txt"), "updated content 1").unwrap();

    // Merge with squash (default behavior)
    // This should create a backup before squashing because there are uncommitted changes
    snapshot_merge_with_env(
        "merge_squash_with_working_tree_creates_backup",
        repo,
        &["main"],
        Some(&feature_wt),
        &[
            ("WORKTRUNK_COMMIT_GENERATION__COMMAND", "echo"),
            ("WORKTRUNK_COMMIT_GENERATION__ARGS", "fix: update files"),
        ],
    );

    // Verify that a backup was created in the reflog
    // Note: The worktree has been removed by the merge, so we check from the repo root
    let output = repo
        .git_command()
        .args(["reflog", "show", "refs/wt-backup/feature"])
        .output()
        .unwrap();

    let reflog = String::from_utf8_lossy(&output.stdout);
    assert!(
        reflog.contains("feature → main (squash)"),
        "Expected backup in reflog, but reflog was: {}",
        reflog
    );
}

#[rstest]
fn test_merge_when_default_branch_missing_worktree(repo: TestRepo) {
    // Move primary off default branch so no worktree holds it
    repo.switch_primary_to("develop");

    snapshot_merge("merge_default_branch_missing_worktree", &repo, &[], None);
}

#[rstest]
fn test_merge_does_not_permanently_set_receive_deny_current_branch(
    merge_scenario: (TestRepo, PathBuf),
) {
    let (repo, feature_wt) = merge_scenario;

    // Explicitly set config to "refuse" - this would block pushes to checked-out branches
    repo.run_git(&["config", "receive.denyCurrentBranch", "refuse"]);

    // Perform merge - should succeed despite "refuse" setting because we use --receive-pack
    let mut cmd = make_snapshot_cmd(&repo, "merge", &["main"], Some(&feature_wt));
    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "Merge should succeed even with receive.denyCurrentBranch=refuse.\n\
         stdout: {}\n\
         stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // Check config after merge - should still be "refuse" (not permanently changed)
    let after = repo
        .git_command()
        .args(["config", "receive.denyCurrentBranch"])
        .output()
        .unwrap();
    let after_value = String::from_utf8_lossy(&after.stdout).trim().to_string();

    assert_eq!(
        after_value, "refuse",
        "receive.denyCurrentBranch should not be permanently modified by merge.\n\
         Expected: \"refuse\"\n\
         Got: {:?}",
        after_value
    );
}

/// Helper to snapshot step squash with env vars
fn snapshot_step_squash_with_env(
    test_name: &str,
    repo: &TestRepo,
    args: &[&str],
    cwd: Option<&Path>,
    env_vars: &[(&str, &str)],
) {
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(repo, "step", &[], cwd);
        cmd.arg("squash").args(args);
        for (key, value) in env_vars {
            cmd.env(key, value);
        }
        assert_cmd_snapshot!(test_name, cmd);
    });
}

/// Helper to snapshot step commit command
fn snapshot_step_commit_with_env(
    test_name: &str,
    repo: &TestRepo,
    args: &[&str],
    cwd: Option<&Path>,
    env_vars: &[(&str, &str)],
) {
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(repo, "step", &[], cwd);
        cmd.arg("commit").args(args);
        for (key, value) in env_vars {
            cmd.env(key, value);
        }
        assert_cmd_snapshot!(test_name, cmd);
    });
}

#[rstest]
fn test_step_squash_with_no_verify_flag(mut repo: TestRepo) {
    // Create a feature worktree with multiple commits
    let feature_wt = repo.add_worktree("feature");

    // Add a pre-commit hook so --no-verify has something to skip
    // Create in feature worktree since worktrees don't share working tree files
    fs::create_dir_all(feature_wt.join(".config")).expect("Failed to create .config");
    fs::write(
        feature_wt.join(".config/wt.toml"),
        "pre-commit = \"echo pre-commit check\"",
    )
    .expect("Failed to write wt.toml");

    // Commit the config as part of first commit to avoid untracked file warnings
    fs::write(feature_wt.join("file1.txt"), "content 1").expect("Failed to write file");
    repo.run_git_in(&feature_wt, &["add", ".config", "file1.txt"]);
    repo.run_git_in(&feature_wt, &["commit", "-m", "feat: add file 1"]);

    fs::write(feature_wt.join("file2.txt"), "content 2").expect("Failed to write file");
    repo.run_git_in(&feature_wt, &["add", "file2.txt"]);
    repo.run_git_in(&feature_wt, &["commit", "-m", "feat: add file 2"]);

    snapshot_step_squash_with_env(
        "step_squash_no_verify",
        &repo,
        &["--no-verify"],
        Some(&feature_wt),
        &[
            ("WORKTRUNK_COMMIT_GENERATION__COMMAND", "echo"),
            (
                "WORKTRUNK_COMMIT_GENERATION__ARGS",
                "squash: combined commits",
            ),
        ],
    );
}

#[rstest]
fn test_step_squash_with_stage_tracked_flag(mut repo: TestRepo) {
    let feature_wt = repo.add_worktree("feature");

    fs::write(feature_wt.join("file1.txt"), "content 1").expect("Failed to write file");
    repo.run_git_in(&feature_wt, &["add", "file1.txt"]);
    repo.run_git_in(&feature_wt, &["commit", "-m", "feat: add file 1"]);

    fs::write(feature_wt.join("file2.txt"), "content 2").expect("Failed to write file");
    repo.run_git_in(&feature_wt, &["add", "file2.txt"]);
    repo.run_git_in(&feature_wt, &["commit", "-m", "feat: add file 2"]);

    // Add uncommitted tracked changes
    fs::write(feature_wt.join("file1.txt"), "updated content").expect("Failed to write file");

    snapshot_step_squash_with_env(
        "step_squash_stage_tracked",
        &repo,
        &["--stage=tracked"],
        Some(&feature_wt),
        &[
            ("WORKTRUNK_COMMIT_GENERATION__COMMAND", "echo"),
            (
                "WORKTRUNK_COMMIT_GENERATION__ARGS",
                "squash: combined commits",
            ),
        ],
    );
}

#[rstest]
fn test_step_squash_with_both_flags(mut repo: TestRepo) {
    let feature_wt = repo.add_worktree("feature");

    // Add a pre-commit hook so --no-verify has something to skip
    // Create in feature worktree since worktrees don't share working tree files
    fs::create_dir_all(feature_wt.join(".config")).expect("Failed to create .config");
    fs::write(
        feature_wt.join(".config/wt.toml"),
        "pre-commit = \"echo pre-commit check\"",
    )
    .expect("Failed to write wt.toml");

    // Commit the config as part of first commit to avoid untracked file warnings
    fs::write(feature_wt.join("file1.txt"), "content 1").expect("Failed to write file");
    repo.run_git_in(&feature_wt, &["add", ".config", "file1.txt"]);
    repo.run_git_in(&feature_wt, &["commit", "-m", "feat: add file 1"]);

    fs::write(feature_wt.join("file2.txt"), "content 2").expect("Failed to write file");
    repo.run_git_in(&feature_wt, &["add", "file2.txt"]);
    repo.run_git_in(&feature_wt, &["commit", "-m", "feat: add file 2"]);

    fs::write(feature_wt.join("file1.txt"), "updated content").expect("Failed to write file");

    snapshot_step_squash_with_env(
        "step_squash_both_flags",
        &repo,
        &["--no-verify", "--stage=tracked"],
        Some(&feature_wt),
        &[
            ("WORKTRUNK_COMMIT_GENERATION__COMMAND", "echo"),
            (
                "WORKTRUNK_COMMIT_GENERATION__ARGS",
                "squash: combined commits",
            ),
        ],
    );
}

#[rstest]
fn test_step_squash_no_commits(mut repo: TestRepo) {
    // Test "nothing to squash; no commits ahead" message

    // Create a feature worktree but don't add any commits
    let feature_wt = repo.add_worktree("feature");

    snapshot_step_squash_with_env("step_squash_no_commits", &repo, &[], Some(&feature_wt), &[]);
}

#[rstest]
fn test_step_squash_single_commit(mut repo: TestRepo) {
    // Test "nothing to squash; already a single commit" message

    // Create a feature worktree with exactly one commit
    let feature_wt =
        repo.add_worktree_with_commit("feature", "file1.txt", "content 1", "feat: single commit");

    snapshot_step_squash_with_env(
        "step_squash_single_commit",
        &repo,
        &[],
        Some(&feature_wt),
        &[],
    );
}

#[rstest]
fn test_step_commit_with_no_verify_flag(repo: TestRepo) {
    // Add a pre-commit hook so --no-verify has something to skip
    fs::create_dir_all(repo.root_path().join(".config")).expect("Failed to create .config");
    fs::write(
        repo.root_path().join(".config/wt.toml"),
        "pre-commit = \"echo pre-commit check\"",
    )
    .expect("Failed to write wt.toml");

    fs::write(repo.root_path().join("file1.txt"), "content 1").expect("Failed to write file");

    snapshot_step_commit_with_env(
        "step_commit_no_verify",
        &repo,
        &["--no-verify"],
        None,
        &[
            ("WORKTRUNK_COMMIT_GENERATION__COMMAND", "echo"),
            ("WORKTRUNK_COMMIT_GENERATION__ARGS", "feat: add file"),
        ],
    );
}

#[rstest]
fn test_step_commit_with_stage_tracked_flag(repo: TestRepo) {
    fs::write(repo.root_path().join("tracked.txt"), "initial").expect("Failed to write file");
    repo.commit("add tracked file");

    fs::write(repo.root_path().join("tracked.txt"), "modified").expect("Failed to write file");
    fs::write(
        repo.root_path().join("untracked.txt"),
        "should not be staged",
    )
    .expect("Failed to write file");

    snapshot_step_commit_with_env(
        "step_commit_stage_tracked",
        &repo,
        &["--stage=tracked"],
        None,
        &[
            ("WORKTRUNK_COMMIT_GENERATION__COMMAND", "echo"),
            (
                "WORKTRUNK_COMMIT_GENERATION__ARGS",
                "fix: update tracked file",
            ),
        ],
    );
}

#[rstest]
fn test_step_commit_with_both_flags(repo: TestRepo) {
    // Add a pre-commit hook so --no-verify has something to skip
    fs::create_dir_all(repo.root_path().join(".config")).expect("Failed to create .config");
    fs::write(
        repo.root_path().join(".config/wt.toml"),
        "pre-commit = \"echo pre-commit check\"",
    )
    .expect("Failed to write wt.toml");

    fs::write(repo.root_path().join("tracked.txt"), "initial").expect("Failed to write file");
    repo.commit("add tracked file");

    fs::write(repo.root_path().join("tracked.txt"), "modified").expect("Failed to write file");

    snapshot_step_commit_with_env(
        "step_commit_both_flags",
        &repo,
        &["--no-verify", "--stage=tracked"],
        None,
        &[
            ("WORKTRUNK_COMMIT_GENERATION__COMMAND", "echo"),
            ("WORKTRUNK_COMMIT_GENERATION__ARGS", "fix: update file"),
        ],
    );
}

#[rstest]
fn test_step_commit_nothing_to_commit(repo: TestRepo) {
    // No changes made - commit should fail with "nothing to commit"
    snapshot_step_commit_with_env(
        "step_commit_nothing_to_commit",
        &repo,
        &["--stage=none"],
        None,
        &[
            ("WORKTRUNK_COMMIT_GENERATION__COMMAND", "echo"),
            (
                "WORKTRUNK_COMMIT_GENERATION__ARGS",
                "feat: this should fail",
            ),
        ],
    );
}

// =============================================================================
// Error message snapshot tests
// =============================================================================

#[rstest]
fn test_merge_error_uncommitted_changes_with_no_commit(mut repo_with_main_worktree: TestRepo) {
    // Tests the `uncommitted_changes()` error function when using --no-commit with dirty tree
    let repo = &mut repo_with_main_worktree;

    // Create a feature worktree
    let feature_wt = repo.add_worktree("feature");

    // Make uncommitted changes (dirty working tree)
    fs::write(feature_wt.join("dirty.txt"), "uncommitted content").unwrap();

    // Try to merge with --no-commit - should fail because working tree is dirty
    snapshot_merge(
        "merge_error_uncommitted_changes_no_commit",
        repo,
        &["main", "--no-commit", "--no-remove"],
        Some(&feature_wt),
    );
}

#[rstest]
fn test_merge_error_conflicting_changes_in_target(mut repo_with_alternate_primary: TestRepo) {
    // Tests the `conflicting_changes()` error function when target worktree has
    // uncommitted changes that overlap with files being pushed
    let repo = &mut repo_with_alternate_primary;

    // Create a feature worktree and commit a change to shared.txt
    let feature_wt = repo.add_worktree_with_commit(
        "feature",
        "shared.txt",
        "feature content",
        "Add shared.txt on feature",
    );

    // Get the main worktree path (created by repo_with_alternate_primary)
    let main_wt = repo.root_path().parent().unwrap().join("repo.main-wt");

    // Now make uncommitted changes to shared.txt in main worktree
    // This creates a conflict - we're trying to push changes to shared.txt
    // but main has uncommitted changes to the same file
    fs::write(
        main_wt.join("shared.txt"),
        "conflicting uncommitted content",
    )
    .unwrap();

    // Try to merge - should fail because of conflicting uncommitted changes
    snapshot_merge(
        "merge_error_conflicting_changes",
        repo,
        &["main"],
        Some(&feature_wt),
    );
}

// =============================================================================
// --show-prompt tests
// =============================================================================

#[rstest]
fn test_step_commit_show_prompt(repo: TestRepo) {
    // Create some staged changes so there's a diff to include in the prompt
    fs::write(repo.root_path().join("new_file.txt"), "new content").expect("Failed to write file");
    repo.git_command().args(["add", "new_file.txt"]);

    // The prompt should be written to stdout
    snapshot_step_commit_with_env(
        "step_commit_show_prompt",
        &repo,
        &["--show-prompt"],
        None,
        &[],
    );
}

#[rstest]
fn test_step_commit_show_prompt_no_staged_changes(repo: TestRepo) {
    // No staged changes - should still output the prompt (with empty diff)
    snapshot_step_commit_with_env(
        "step_commit_show_prompt_no_staged",
        &repo,
        &["--show-prompt"],
        None,
        &[],
    );
}

#[rstest]
fn test_step_squash_show_prompt(repo_with_multi_commit_feature: TestRepo) {
    let repo = repo_with_multi_commit_feature;

    // Get the feature worktree path
    let feature_wt = repo.worktree_path("feature");

    // Should output the squash prompt with commits and diff
    snapshot_step_squash_with_env(
        "step_squash_show_prompt",
        &repo,
        &["--show-prompt"],
        Some(feature_wt),
        &[],
    );
}

// =============================================================================
// step rebase tests
// =============================================================================

/// Helper to snapshot step rebase command
fn snapshot_step_rebase(test_name: &str, repo: &TestRepo, args: &[&str], cwd: Option<&Path>) {
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(repo, "step", &[], cwd);
        cmd.arg("rebase").args(args);
        assert_cmd_snapshot!(test_name, cmd);
    });
}

/// Test that step rebase correctly handles merge commits.
///
/// When a branch has merged main into it, the merge-base equals main's HEAD,
/// but there are still commits that need rebasing to linearize the history.
/// This test verifies that we don't incorrectly report "Already up-to-date".
#[rstest]
fn test_step_rebase_with_merge_commit(mut repo: TestRepo) {
    // Create a feature worktree with a commit
    let feature_wt = repo.add_worktree_with_commit(
        "feature-with-merge",
        "feature.txt",
        "feature content",
        "Add feature",
    );

    // Advance main with a new commit
    fs::write(repo.root_path().join("main-update.txt"), "main content").unwrap();
    repo.run_git(&["add", "main-update.txt"]);
    repo.run_git(&["commit", "-m", "Update main"]);

    // Merge main INTO the feature branch (creating a merge commit)
    let output = repo
        .git_command()
        .current_dir(&feature_wt)
        .args(["merge", "main", "-m", "Merge main into feature"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git merge failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Now step rebase should linearize the history (not report "Already up-to-date")
    snapshot_step_rebase(
        "step_rebase_with_merge_commit",
        &repo,
        &["main"],
        Some(&feature_wt),
    );
}
