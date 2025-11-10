use crate::common::{TestRepo, make_snapshot_cmd_with_global_flags, setup_snapshot_settings};
use insta_cmd::assert_cmd_snapshot;
use std::path::Path;

/// Helper to create snapshot with normalized paths and SHAs
fn snapshot_switch(test_name: &str, repo: &TestRepo, args: &[&str]) {
    snapshot_switch_with_home(test_name, repo, args, None, &[]);
}

/// Helper to create snapshot with global flags (e.g., --internal)
fn snapshot_switch_with_global_flags(
    test_name: &str,
    repo: &TestRepo,
    args: &[&str],
    global_flags: &[&str],
) {
    snapshot_switch_with_home(test_name, repo, args, None, global_flags);
}

/// Helper that also allows setting a custom HOME directory and global flags
fn snapshot_switch_with_home(
    test_name: &str,
    repo: &TestRepo,
    args: &[&str],
    temp_home: Option<&Path>,
    global_flags: &[&str],
) {
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd_with_global_flags(repo, "switch", args, None, global_flags);
        if let Some(home) = temp_home {
            cmd.env("HOME", home);
        }
        assert_cmd_snapshot!(test_name, cmd);
    });
}

#[test]
fn test_switch_create_new_branch() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    snapshot_switch("switch_create_new", &repo, &["--create", "feature-x"]);
}

#[test]
fn test_switch_create_existing_branch_error() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create a branch first
    repo.add_worktree("feature-y", "feature-y");

    // Try to create it again - should error
    snapshot_switch(
        "switch_create_existing_error",
        &repo,
        &["--create", "feature-y"],
    );
}

#[test]
fn test_switch_create_with_remote_branch_only() {
    use std::process::Command;

    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Set up a remote
    repo.setup_remote("main");

    // Create a branch on the remote only (no local branch)
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["branch", "remote-feature"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to create branch");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["push", "origin", "remote-feature"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to push to remote");

    // Delete the local branch
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["branch", "-D", "remote-feature"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to delete local branch");

    // Now we have origin/remote-feature but no local remote-feature
    // This should succeed with --create (previously would fail)
    snapshot_switch(
        "switch_create_remote_only",
        &repo,
        &["--create", "remote-feature"],
    );
}

#[test]
fn test_switch_existing_branch() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create a worktree for a branch
    repo.add_worktree("feature-z", "feature-z");

    // Switch to it (should find existing worktree)
    snapshot_switch("switch_existing_branch", &repo, &["feature-z"]);
}

#[test]
fn test_switch_with_base_branch() {
    let repo = TestRepo::new();
    repo.commit("Initial commit on main");

    snapshot_switch(
        "switch_with_base",
        &repo,
        &["--create", "--base", "main", "feature-with-base"],
    );
}

#[test]
fn test_switch_base_without_create_warning() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    snapshot_switch(
        "switch_base_without_create",
        &repo,
        &["--base", "main", "main"],
    );
}

#[test]
fn test_switch_internal_mode() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    snapshot_switch_with_global_flags(
        "switch_internal_mode",
        &repo,
        &["--create", "internal-test"],
        &["--internal"],
    );
}

#[test]
fn test_switch_existing_worktree_internal() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    repo.add_worktree("existing-wt", "existing-wt");

    snapshot_switch_with_global_flags(
        "switch_existing_internal",
        &repo,
        &["existing-wt"],
        &["--internal"],
    );
}

#[test]
fn test_switch_internal_with_execute() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    let execute_cmd = "echo 'line1'\necho 'line2'";

    snapshot_switch_with_global_flags(
        "switch_internal_with_execute",
        &repo,
        &["--create", "exec-internal", "--execute", execute_cmd],
        &["--internal"],
    );
}

#[test]
fn test_switch_error_missing_worktree_directory() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create a worktree
    let wt_path = repo.add_worktree("missing-wt", "missing-wt");

    // Remove the worktree directory (but leave it registered in git)
    std::fs::remove_dir_all(&wt_path).expect("Failed to remove worktree directory");

    // Try to switch to the missing worktree (should fail)
    snapshot_switch("switch_error_missing_directory", &repo, &["missing-wt"]);
}

#[test]
fn test_switch_execute_success() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    snapshot_switch(
        "switch_execute_success",
        &repo,
        &["--create", "exec-test", "--execute", "echo 'test output'"],
    );
}

#[test]
fn test_switch_execute_creates_file() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    let create_file_cmd = "echo 'test content' > test.txt";

    snapshot_switch(
        "switch_execute_creates_file",
        &repo,
        &["--create", "file-test", "--execute", create_file_cmd],
    );
}

#[test]
fn test_switch_execute_failure() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    snapshot_switch(
        "switch_execute_failure",
        &repo,
        &["--create", "fail-test", "--execute", "exit 1"],
    );
}

#[test]
fn test_switch_execute_with_existing_worktree() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create a worktree first
    repo.add_worktree("existing-exec", "existing-exec");

    let create_file_cmd = "echo 'existing worktree' > existing.txt";

    snapshot_switch(
        "switch_execute_existing",
        &repo,
        &["existing-exec", "--execute", create_file_cmd],
    );
}

#[test]
fn test_switch_execute_multiline() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    let multiline_cmd = "echo 'line1'\necho 'line2'\necho 'line3'";

    snapshot_switch(
        "switch_execute_multiline",
        &repo,
        &["--create", "multiline-test", "--execute", multiline_cmd],
    );
}

#[test]
fn test_switch_no_config_commands_execute_still_runs() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    snapshot_switch(
        "switch_no_hooks_execute_still_runs",
        &repo,
        &[
            "--create",
            "no-hooks-test",
            "--execute",
            "echo 'execute command runs'",
            "--no-hooks",
        ],
    );
}

#[test]
fn test_switch_no_config_commands_skips_post_start_commands() {
    use std::fs;
    use tempfile::TempDir;

    let temp_home = TempDir::new().unwrap();
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create project config with a command that would create a file
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create .config dir");

    let create_file_cmd = "echo 'marker' > marker.txt";

    fs::write(
        config_dir.join("wt.toml"),
        format!(r#"post-start-commands = ["{}"]"#, create_file_cmd),
    )
    .expect("Failed to write config");

    repo.commit("Add config");

    // Pre-approve the command
    let user_config_dir = temp_home.path().join(".config/worktrunk");
    fs::create_dir_all(&user_config_dir).expect("Failed to create user config dir");
    fs::write(
        user_config_dir.join("config.toml"),
        format!(
            r#"worktree-path = "../{{main-worktree}}.{{branch}}"

[projects."main"]
approved-commands = ["{}"]
"#,
            create_file_cmd
        ),
    )
    .expect("Failed to write user config");

    // With --no-hooks, the post-start command should be skipped
    snapshot_switch_with_home(
        "switch_no_hooks_skips_post_start",
        &repo,
        &["--create", "no-post-start", "--no-hooks"],
        Some(temp_home.path()),
        &[],
    );
}

#[test]
fn test_switch_no_config_commands_with_existing_worktree() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create a worktree first
    repo.add_worktree("existing-no-hooks", "existing-no-hooks");

    // With --no-hooks, the --execute command should still run
    snapshot_switch(
        "switch_no_hooks_existing",
        &repo,
        &[
            "existing-no-hooks",
            "--execute",
            "echo 'execute still runs'",
            "--no-hooks",
        ],
    );
}

#[test]
fn test_switch_no_config_commands_with_force() {
    use std::fs;
    use tempfile::TempDir;

    let temp_home = TempDir::new().unwrap();
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create project config with a command
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).expect("Failed to create .config dir");
    fs::write(
        config_dir.join("wt.toml"),
        r#"post-start-commands = ["echo 'test'"]"#,
    )
    .expect("Failed to write config");

    repo.commit("Add config");

    // With --no-hooks, even --force shouldn't execute config commands
    snapshot_switch_with_home(
        "switch_no_hooks_with_force",
        &repo,
        &["--create", "force-no-hooks", "--force", "--no-hooks"],
        Some(temp_home.path()),
        &[],
    );
}

#[test]
fn test_switch_create_no_remote() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");
    // Deliberately NOT calling setup_remote to test local branch inference

    // Create a branch without specifying base - should infer default branch locally
    snapshot_switch("switch_create_no_remote", &repo, &["--create", "feature"]);
}

#[test]
fn test_switch_primary_on_different_branch() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    repo.switch_primary_to("develop");
    assert_eq!(repo.current_branch(), "develop");

    // Create a feature worktree using the default branch (main)
    // This should work fine even though primary is on develop
    snapshot_switch(
        "switch_primary_on_different_branch",
        &repo,
        &["--create", "feature-from-main"],
    );

    // Also test switching to an existing branch
    repo.add_worktree("existing-branch", "existing-branch");
    snapshot_switch(
        "switch_to_existing_primary_on_different_branch",
        &repo,
        &["existing-branch"],
    );
}

#[test]
fn test_switch_previous_branch() {
    use std::process::Command;

    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create two branches
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["branch", "feature-a"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to create feature-a");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["branch", "feature-b"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to create feature-b");

    // Use wt switch to establish worktrunk.history
    snapshot_switch("switch_previous_branch_first", &repo, &["feature-a"]);
    snapshot_switch("switch_previous_branch_second", &repo, &["feature-b"]);

    // Now wt switch - should resolve to feature-b (the previous wt switch)
    snapshot_switch("switch_previous_branch", &repo, &["-"]);
}

#[test]
fn test_switch_previous_branch_no_history() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // No checkout history, so wt switch - should fail with helpful error
    snapshot_switch("switch_previous_branch_no_history", &repo, &["-"]);
}

#[test]
fn test_switch_previous_branch_with_worktrunk_history() {
    use std::process::Command;

    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create two branches
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["branch", "feature-x"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to create feature-x");

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["branch", "feature-y"])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to create feature-y");

    // Use wt switch to switch to feature-x (this records history)
    snapshot_switch(
        "switch_previous_branch_with_worktrunk_history_first",
        &repo,
        &["feature-x"],
    );

    // Switch to feature-y (this records feature-x in history)
    snapshot_switch(
        "switch_previous_branch_with_worktrunk_history_second",
        &repo,
        &["feature-y"],
    );

    // Now wt switch - should resolve to feature-x (from worktrunk.history)
    snapshot_switch(
        "switch_previous_branch_with_worktrunk_history_back",
        &repo,
        &["-"],
    );
}
