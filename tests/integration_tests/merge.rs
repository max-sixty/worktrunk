use crate::common::{
    TestRepo, make_snapshot_cmd, merge_scenario,
    mock_commands::{create_mock_cargo, create_mock_llm_auth},
    repo, repo_with_alternate_primary, repo_with_feature_worktree, repo_with_main_worktree,
    repo_with_multi_commit_feature, setup_snapshot_settings,
};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::fs;
use std::path::{Path, PathBuf};

/// Create a PATH with the given mock bin directory prepended, preserving variable case.
///
/// Returns (variable_name, value) where variable_name preserves the case found
/// in the environment (important for Windows where env vars are case-insensitive
/// but Rust stores them case-sensitively - using "PATH" when the system has "Path"
/// creates a duplicate).
fn make_path_with_mock_bin(bin_dir: &Path) -> (String, String) {
    // Find the actual PATH variable name to avoid creating a duplicate with different case
    let (path_var_name, current_path) = std::env::vars_os()
        .find(|(k, _)| k.eq_ignore_ascii_case("PATH"))
        .map(|(k, v)| (k.to_string_lossy().into_owned(), Some(v)))
        .unwrap_or(("PATH".to_string(), None));

    let mut paths: Vec<PathBuf> = current_path
        .as_deref()
        .map(|p| std::env::split_paths(p).collect())
        .unwrap_or_default();
    paths.insert(0, bin_dir.to_path_buf());
    let new_path = std::env::join_paths(&paths)
        .unwrap()
        .to_string_lossy()
        .into_owned();

    (path_var_name, new_path)
}

fn snapshot_merge_with_env(
    test_name: &str,
    repo: &TestRepo,
    args: &[&str],
    cwd: Option<&Path>,
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
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "merge",
        &["main"],
        Some(&feature_wt)
    ));
}

///
/// When git runs a subcommand, it sets `GIT_EXEC_PATH` in the environment.
/// Shell integration cannot work in this case because cd directives cannot
/// propagate through git's subprocess to the parent shell.
#[rstest]
fn test_merge_as_git_subcommand(merge_scenario: (TestRepo, PathBuf)) {
    let (repo, feature_wt) = merge_scenario;

    // Merge with GIT_EXEC_PATH set (simulating `git wt merge ...`)
    assert_cmd_snapshot!({
        let mut cmd = make_snapshot_cmd(&repo, "merge", &["main"], Some(&feature_wt));
        cmd.env("GIT_EXEC_PATH", "/usr/lib/git-core");
        cmd
    });
}

#[rstest]
fn test_merge_primary_not_on_default_with_default_worktree(
    mut repo_with_alternate_primary: TestRepo,
) {
    let repo = &mut repo_with_alternate_primary;
    let feature_wt = repo.add_feature();

    assert_cmd_snapshot!(make_snapshot_cmd(
        repo,
        "merge",
        &["main"],
        Some(&feature_wt)
    ));
}

#[rstest]
fn test_merge_with_no_remove_flag(merge_scenario: (TestRepo, PathBuf)) {
    let (repo, feature_wt) = merge_scenario;

    // Merge with --no-remove flag (should not finish worktree)
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "merge",
        &["main", "--no-remove"],
        Some(&feature_wt)
    ));
}

#[rstest]
fn test_merge_already_on_target(repo: TestRepo) {
    // Already on main branch (repo root)
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "merge", &[], None));
}

#[rstest]
fn test_merge_dirty_working_tree(mut repo: TestRepo) {
    // Create a feature worktree with uncommitted changes
    let feature_wt = repo.add_worktree("feature");
    std::fs::write(feature_wt.join("dirty.txt"), "uncommitted content").unwrap();

    // Try to merge (should fail due to dirty working tree)
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "merge",
        &["main"],
        Some(&feature_wt)
    ));
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
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "merge",
        &["main"],
        Some(&feature_wt)
    ));
}

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
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "merge",
        &["main", "--no-commit", "--no-remove"],
        Some(&feature_path)
    ));
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
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "merge",
        &["main"],
        Some(&feature_wt)
    ));
}

#[rstest]
fn test_merge_to_default_branch(merge_scenario: (TestRepo, PathBuf)) {
    let (repo, feature_wt) = merge_scenario;

    // Merge without specifying target (should use default branch)
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "merge", &[], Some(&feature_wt)));
}

#[rstest]
fn test_merge_with_caret_symbol(merge_scenario: (TestRepo, PathBuf)) {
    let (repo, feature_wt) = merge_scenario;

    // Merge using ^ symbol (should resolve to default branch)
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "merge", &["^"], Some(&feature_wt)));
}

#[rstest]
fn test_merge_error_detached_head(repo: TestRepo) {
    // Detach HEAD in the repo
    repo.detach_head();

    // Try to merge (should fail - detached HEAD)
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "merge",
        &["main"],
        Some(repo.root_path())
    ));
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
    assert_cmd_snapshot!(make_snapshot_cmd(
        repo,
        "merge",
        &["main"],
        Some(&feature_wt)
    ));
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
    assert_cmd_snapshot!(make_snapshot_cmd(
        repo,
        "merge",
        &["main"],
        Some(&feature_wt)
    ));
}

#[rstest]
fn test_merge_squash_llm_command_not_found(mut repo_with_main_worktree: TestRepo) {
    let repo = &mut repo_with_main_worktree;
    // Create a feature worktree and make multiple commits
    let feature_wt = repo.add_worktree("feature");
    repo.commit_in_worktree(&feature_wt, "file1.txt", "content 1", "feat: new feature");
    repo.commit_in_worktree(&feature_wt, "file2.txt", "content 2", "fix: bug fix");

    // Configure LLM command that doesn't exist - should error
    assert_cmd_snapshot!({
        let mut cmd = make_snapshot_cmd(repo, "merge", &["main"], Some(&feature_wt));
        cmd.env(
            "WORKTRUNK_COMMIT_GENERATION__COMMAND",
            "nonexistent-llm-command",
        );
        cmd
    });
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

    assert_cmd_snapshot!(make_snapshot_cmd(
        repo,
        "merge",
        &["main"],
        Some(&feature_wt)
    ));
}

#[rstest]
fn test_merge_squash_single_commit(mut repo_with_main_worktree: TestRepo) {
    let repo = &mut repo_with_main_worktree;
    // Create a feature worktree with only one commit
    let feature_wt =
        repo.add_worktree_with_commit("feature", "file1.txt", "content", "feat: single commit");

    // Merge (squashing is default) - should skip squashing since there's only one commit
    assert_cmd_snapshot!(make_snapshot_cmd(
        repo,
        "merge",
        &["main"],
        Some(&feature_wt)
    ));
}

#[rstest]
fn test_merge_no_squash(repo_with_multi_commit_feature: TestRepo) {
    let repo = &repo_with_multi_commit_feature;
    let feature_wt = &repo.worktrees["feature"];

    // Merge with --no-squash - should NOT squash the commits
    assert_cmd_snapshot!(make_snapshot_cmd(
        repo,
        "merge",
        &["main", "--no-squash"],
        Some(feature_wt)
    ));
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
    assert_cmd_snapshot!(make_snapshot_cmd(
        repo,
        "merge",
        &["main"],
        Some(&feature_wt)
    ));
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
    assert_cmd_snapshot!(make_snapshot_cmd(
        repo,
        "merge",
        &["main"],
        Some(&feature_wt)
    ));
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
    assert_cmd_snapshot!(make_snapshot_cmd(
        repo,
        "merge",
        &["main"],
        Some(&feature_wt)
    ));
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
    assert_cmd_snapshot!(make_snapshot_cmd(
        repo,
        "merge",
        &["main"],
        Some(feature_wt)
    ));
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
    assert_cmd_snapshot!({
        let mut cmd = make_snapshot_cmd(repo, "merge", &["main"], Some(&feature_wt));
        for (key, value) in &[
            ("WORKTRUNK_COMMIT_GENERATION__COMMAND", "echo"),
            ("WORKTRUNK_COMMIT_GENERATION__ARGS", "fix: commit changes"),
        ] {
            cmd.env(key, value);
        }
        cmd
    });
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
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "merge",
        &["main", "--yes"],
        Some(&feature_wt)
    ));
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
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "merge",
        &["main", "--yes"],
        Some(&feature_wt)
    ));
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
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "merge",
        &["main", "--no-verify"],
        Some(&feature_wt)
    ));
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
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "merge",
        &["main", "--yes"],
        Some(&feature_wt)
    ));
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
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "merge",
        &["main", "--yes"],
        Some(&feature_wt)
    ));

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
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "merge",
        &["main", "--yes", "--no-verify"],
        Some(&feature_wt)
    ));

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
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "merge",
        &["main", "--yes"],
        Some(&feature_wt)
    ));
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
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "merge",
        &["main", "--yes"],
        Some(&feature_wt)
    ));

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
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "merge",
        &["main", "--yes"],
        Some(&feature_wt)
    ));

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
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "merge", &["--yes"], None));

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
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "merge",
        &["main", "--yes"],
        Some(&feature_wt)
    ));
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
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "merge",
        &["main", "--yes"],
        Some(&feature_wt)
    ));
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
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "merge",
        &["main", "--yes"],
        Some(&feature_wt)
    ));
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
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "merge",
        &["main", "--yes"],
        Some(&feature_wt)
    ));
}

#[rstest]
fn test_merge_no_remote(#[from(repo_with_feature_worktree)] repo: TestRepo) {
    // Deliberately NOT calling setup_remote to test the error case
    let feature_wt = repo.worktree_path("feature");

    // Try to merge without specifying target (should fail - no remote to get default branch)
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "merge", &[], Some(feature_wt)));
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
    assert_cmd_snapshot!(
        "readme_example_simple_switch",
        make_snapshot_cmd(&repo, "switch", &["--create", "fix-auth"], None)
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
    assert_cmd_snapshot!(
        "readme_example_simple",
        make_snapshot_cmd(&repo, "merge", &["main"], Some(&feature_wt))
    );
}

/// Generate README example: Complex merge with multiple hooks
/// This demonstrates advanced features - pre-merge hooks (tests, lints), post-merge hooks.
/// Shows the full power of worktrunk's automation capabilities.
///
/// Output is used in README.md "Advanced Features" or "Project Automation" section.
/// Source: tests/snapshots/integration__integration_tests__merge__readme_example_complex.snap
#[rstest]
fn test_readme_example_complex(mut repo: TestRepo) {
    // Create project config with multiple hooks
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();

    // Create mock commands for realistic output (cross-platform)
    let bin_dir = repo.root_path().join(".bin");
    fs::create_dir_all(&bin_dir).unwrap();

    // Create cross-platform mock commands
    create_mock_cargo(&bin_dir);
    create_mock_llm_auth(&bin_dir);

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
    // On Windows, use .exe extension for the config-driven mock binary
    let llm_name = if cfg!(windows) { "llm.exe" } else { "llm" };
    let llm_path = bin_dir.join(llm_name);
    let llm_path_str = llm_path.to_string_lossy().replace('\\', "/");
    let worktrunk_config = format!(
        r#"
[commit-generation]
command = "{llm_path_str}"
"#
    );
    fs::write(repo.test_config_path(), worktrunk_config).unwrap();

    // Merge with --yes to skip approval prompts for commands
    let (path_var, path_with_bin) = make_path_with_mock_bin(&bin_dir);
    snapshot_merge_with_env(
        "readme_example_complex",
        &repo,
        &["main", "--yes"],
        Some(&feature_wt),
        &[(&path_var, &path_with_bin)],
    );
}

// NOTE: test_readme_example_hooks_post_create and test_readme_example_hooks_pre_merge
// were removed - they're covered by PTY-based tests in shell_wrapper.rs that capture
// combined stdout/stderr for README examples.

#[rstest]
fn test_merge_no_commit_with_clean_tree(mut repo_with_feature_worktree: TestRepo) {
    let repo = &mut repo_with_feature_worktree;
    let feature_wt = &repo.worktrees["feature"];

    // Merge with --no-commit (should succeed - clean tree)
    assert_cmd_snapshot!(make_snapshot_cmd(
        repo,
        "merge",
        &["main", "--no-commit", "--no-remove"],
        Some(feature_wt),
    ));
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
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "merge",
        &["main", "--no-commit"],
        Some(&feature_wt)
    ));
}

#[rstest]
fn test_merge_no_commit_no_squash_no_remove_redundant(mut repo_with_feature_worktree: TestRepo) {
    let repo = &mut repo_with_feature_worktree;
    let feature_wt = &repo.worktrees["feature"];

    // Merge with --no-commit --no-squash --no-remove (redundant but valid - should succeed)
    assert_cmd_snapshot!(make_snapshot_cmd(
        repo,
        "merge",
        &["main", "--no-commit", "--no-squash", "--no-remove"],
        Some(feature_wt),
    ));
}

#[rstest]
fn test_merge_no_commits(mut repo_with_main_worktree: TestRepo) {
    let repo = &mut repo_with_main_worktree;

    // Create a feature worktree with NO commits (just branched from main)
    let feature_wt = repo.add_worktree("no-commits");

    // Merge without any commits - should skip both squashing and rebasing
    assert_cmd_snapshot!(make_snapshot_cmd(
        repo,
        "merge",
        &["main"],
        Some(&feature_wt)
    ));
}

#[rstest]
fn test_merge_no_commits_with_changes(mut repo_with_main_worktree: TestRepo) {
    let repo = &mut repo_with_main_worktree;

    // Create a feature worktree with NO commits but WITH uncommitted changes
    let feature_wt = repo.add_worktree("no-commits-dirty");
    fs::write(feature_wt.join("newfile.txt"), "new content").unwrap();

    // Merge - should commit the changes, skip squashing (only 1 commit), and skip rebasing (at merge base)
    assert_cmd_snapshot!(make_snapshot_cmd(
        repo,
        "merge",
        &["main"],
        Some(&feature_wt)
    ));
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
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "merge",
        &["main"],
        Some(&feature_wt)
    ));
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
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "merge",
        &["main"],
        Some(&feature_wt)
    ));
}

// =============================================================================
// --no-rebase tests
// =============================================================================

#[rstest]
fn test_merge_no_rebase_when_already_rebased(merge_scenario: (TestRepo, PathBuf)) {
    // Feature branch is based on main (no divergence), so --no-rebase should succeed
    let (repo, feature_wt) = merge_scenario;

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "merge",
        &["main", "--no-rebase"],
        Some(&feature_wt)
    ));
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
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "merge",
        &["main", "--no-rebase"],
        Some(&feature_wt)
    ));
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

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "merge",
        &["main"],
        Some(&feature_wt)
    ));

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
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "merge",
        &["main"],
        Some(&feature_wt)
    ));
}

#[rstest]
fn test_merge_race_condition_commit_after_push(mut repo_with_feature_worktree: TestRepo) {
    let repo = &mut repo_with_feature_worktree;
    let feature_wt = repo.worktrees["feature"].clone();

    // Merge to main (this pushes the branch to main)
    assert_cmd_snapshot!(make_snapshot_cmd(
        repo,
        "merge",
        &["main", "--no-remove"],
        Some(&feature_wt)
    ));

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
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "merge",
        &["staging"],
        Some(&feature_wt)
    ));
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
    assert_cmd_snapshot!({
        let mut cmd = make_snapshot_cmd(repo, "merge", &["main"], Some(&feature_wt));
        for (key, value) in &[
            ("WORKTRUNK_COMMIT_GENERATION__COMMAND", "echo"),
            ("WORKTRUNK_COMMIT_GENERATION__ARGS", "fix: update files"),
        ] {
            cmd.env(key, value);
        }
        cmd
    });

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

    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "merge", &[], None));
}

#[rstest]
fn test_merge_doesnt_set_receive_deny_current_branch(merge_scenario: (TestRepo, PathBuf)) {
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

    assert_cmd_snapshot!({
        let mut cmd = make_snapshot_cmd(&repo, "step", &[], Some(&feature_wt));
        cmd.arg("squash").args(["--no-verify"]);
        for (key, value) in &[
            ("WORKTRUNK_COMMIT_GENERATION__COMMAND", "echo"),
            (
                "WORKTRUNK_COMMIT_GENERATION__ARGS",
                "squash: combined commits",
            ),
        ] {
            cmd.env(key, value);
        }
        cmd
    });
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

    assert_cmd_snapshot!({
        let mut cmd = make_snapshot_cmd(&repo, "step", &[], Some(&feature_wt));
        cmd.arg("squash").args(["--stage=tracked"]);
        for (key, value) in &[
            ("WORKTRUNK_COMMIT_GENERATION__COMMAND", "echo"),
            (
                "WORKTRUNK_COMMIT_GENERATION__ARGS",
                "squash: combined commits",
            ),
        ] {
            cmd.env(key, value);
        }
        cmd
    });
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

    assert_cmd_snapshot!({
        let mut cmd = make_snapshot_cmd(&repo, "step", &[], Some(&feature_wt));
        cmd.arg("squash").args(["--no-verify", "--stage=tracked"]);
        for (key, value) in &[
            ("WORKTRUNK_COMMIT_GENERATION__COMMAND", "echo"),
            (
                "WORKTRUNK_COMMIT_GENERATION__ARGS",
                "squash: combined commits",
            ),
        ] {
            cmd.env(key, value);
        }
        cmd
    });
}

#[rstest]
fn test_step_squash_no_commits(mut repo: TestRepo) {
    // Test "nothing to squash; no commits ahead" message

    // Create a feature worktree but don't add any commits
    let feature_wt = repo.add_worktree("feature");

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["squash"],
        Some(&feature_wt)
    ));
}

#[rstest]
fn test_step_squash_single_commit(mut repo: TestRepo) {
    // Test "nothing to squash; already a single commit" message

    // Create a feature worktree with exactly one commit
    let feature_wt =
        repo.add_worktree_with_commit("feature", "file1.txt", "content 1", "feat: single commit");

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["squash"],
        Some(&feature_wt)
    ));
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

    assert_cmd_snapshot!({
        let mut cmd = make_snapshot_cmd(&repo, "step", &[], None);
        cmd.arg("commit").args(["--no-verify"]);
        for (key, value) in &[
            ("WORKTRUNK_COMMIT_GENERATION__COMMAND", "echo"),
            ("WORKTRUNK_COMMIT_GENERATION__ARGS", "feat: add file"),
        ] {
            cmd.env(key, value);
        }
        cmd
    });
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

    assert_cmd_snapshot!({
        let mut cmd = make_snapshot_cmd(&repo, "step", &[], None);
        cmd.arg("commit").args(["--stage=tracked"]);
        for (key, value) in &[
            ("WORKTRUNK_COMMIT_GENERATION__COMMAND", "echo"),
            (
                "WORKTRUNK_COMMIT_GENERATION__ARGS",
                "fix: update tracked file",
            ),
        ] {
            cmd.env(key, value);
        }
        cmd
    });
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

    assert_cmd_snapshot!({
        let mut cmd = make_snapshot_cmd(&repo, "step", &[], None);
        cmd.arg("commit").args(["--no-verify", "--stage=tracked"]);
        for (key, value) in &[
            ("WORKTRUNK_COMMIT_GENERATION__COMMAND", "echo"),
            ("WORKTRUNK_COMMIT_GENERATION__ARGS", "fix: update file"),
        ] {
            cmd.env(key, value);
        }
        cmd
    });
}

#[rstest]
fn test_step_commit_nothing_to_commit(repo: TestRepo) {
    // No changes made - commit should fail with "nothing to commit"
    assert_cmd_snapshot!({
        let mut cmd = make_snapshot_cmd(&repo, "step", &[], None);
        cmd.arg("commit").args(["--stage=none"]);
        for (key, value) in &[
            ("WORKTRUNK_COMMIT_GENERATION__COMMAND", "echo"),
            (
                "WORKTRUNK_COMMIT_GENERATION__ARGS",
                "feat: this should fail",
            ),
        ] {
            cmd.env(key, value);
        }
        cmd
    });
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
    assert_cmd_snapshot!(make_snapshot_cmd(
        repo,
        "merge",
        &["main", "--no-commit", "--no-remove"],
        Some(&feature_wt)
    ));
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
    assert_cmd_snapshot!(make_snapshot_cmd(
        repo,
        "merge",
        &["main"],
        Some(&feature_wt)
    ));
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
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["commit", "--show-prompt"],
        None
    ));
}

#[rstest]
fn test_step_commit_show_prompt_no_staged_changes(repo: TestRepo) {
    // No staged changes - should still output the prompt (with empty diff)
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["commit", "--show-prompt"],
        None
    ));
}

#[rstest]
fn test_step_squash_show_prompt(repo_with_multi_commit_feature: TestRepo) {
    let repo = repo_with_multi_commit_feature;

    // Get the feature worktree path
    let feature_wt = repo.worktree_path("feature");

    // Should output the squash prompt with commits and diff
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["squash", "--show-prompt"],
        Some(feature_wt)
    ));
}

// =============================================================================
// step rebase tests
// =============================================================================

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
    assert_cmd_snapshot!({
        let mut cmd = make_snapshot_cmd(&repo, "step", &[], Some(&feature_wt));
        cmd.arg("rebase").args(["main"]);
        cmd
    });
}

/// Test `wt step rebase` when the feature branch is already up to date with main.
///
/// This should show "Already up to date with main" message.
#[rstest]
fn test_step_rebase_already_up_to_date(mut repo: TestRepo) {
    // Create a feature worktree but don't advance main (feature is based on main's HEAD)
    let feature_wt = repo.add_worktree("feature");

    // Run `wt step rebase` - should show "Already up to date with main"
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["rebase"],
        Some(&feature_wt)
    ));
}

// =============================================================================
// Target validation tests
// =============================================================================

#[rstest]
fn test_merge_invalid_target(mut repo: TestRepo) {
    // Create a feature worktree
    let feature_wt = repo.add_worktree("feature");

    // Try to merge into nonexistent branch - should fail with clear error
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "merge",
        &["nonexistent-branch"],
        Some(&feature_wt)
    ));
}

#[rstest]
fn test_step_rebase_invalid_target(mut repo: TestRepo) {
    // Create a feature worktree
    let feature_wt = repo.add_worktree("feature");

    // Try to rebase onto nonexistent ref - should fail with clear error
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["rebase", "nonexistent-ref"],
        Some(&feature_wt)
    ));
}

#[rstest]
fn test_step_rebase_accepts_tag(mut repo: TestRepo) {
    // Create a tag on main
    repo.run_git(&["tag", "v1.0"]);

    // Advance main
    fs::write(repo.root_path().join("after-tag.txt"), "content").unwrap();
    repo.run_git(&["add", "after-tag.txt"]);
    repo.run_git(&["commit", "-m", "After tag"]);

    // Create feature from current main
    let feature_wt = repo.add_worktree("feature");

    // Rebase onto the tag - should work (commit-ish accepted)
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["rebase", "v1.0"],
        Some(&feature_wt)
    ));
}
