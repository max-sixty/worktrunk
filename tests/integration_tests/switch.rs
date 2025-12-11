use crate::common::{TestRepo, make_snapshot_cmd_with_global_flags, setup_snapshot_settings};
use insta_cmd::assert_cmd_snapshot;
use std::path::Path;
use tempfile::TempDir;

/// Common setup for switch tests - creates repo with initial commit
fn setup_switch_repo() -> TestRepo {
    TestRepo::new()
}

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
///
/// IMPORTANT: Always uses an isolated HOME to prevent tests from being affected
/// by the developer's real shell configuration (e.g., shell integration in ~/.zshrc).
/// Pass `temp_home` to use a specific HOME (e.g., with shell integration configured).
fn snapshot_switch_with_home(
    test_name: &str,
    repo: &TestRepo,
    args: &[&str],
    temp_home: Option<&Path>,
    global_flags: &[&str],
) {
    // Create isolated HOME if not provided to ensure test determinism
    let default_home = TempDir::new().unwrap();
    let home = temp_home.unwrap_or_else(|| default_home.path());

    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd_with_global_flags(repo, "switch", args, None, global_flags);
        cmd.env("HOME", home);
        // Windows: the `home` crate uses USERPROFILE for home_dir()
        cmd.env("USERPROFILE", home);
        assert_cmd_snapshot!(test_name, cmd);
    });
}

#[test]
fn test_switch_create_new_branch() {
    let repo = setup_switch_repo();

    snapshot_switch("switch_create_new", &repo, &["--create", "feature-x"]);
}

#[test]
fn test_switch_create_existing_branch_error() {
    let mut repo = setup_switch_repo();

    // Create a branch first
    repo.add_worktree("feature-y");

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

    let mut repo = setup_switch_repo();

    // Set up a remote
    repo.setup_remote("main");

    // Create a branch on the remote only (no local branch)
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["branch", "remote-feature"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["push", "origin", "remote-feature"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    // Delete the local branch
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["branch", "-D", "remote-feature"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

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
    let mut repo = setup_switch_repo();

    // Create a worktree for a branch
    repo.add_worktree("feature-z");

    // Switch to it (should find existing worktree)
    snapshot_switch("switch_existing_branch", &repo, &["feature-z"]);
}

/// Test switching to existing worktree when shell integration is configured but not active.
///
/// When shell integration is configured in user's rc files (e.g., .zshrc) but the user
/// runs `wt switch` directly (not through the shell wrapper), we should show success
/// with a hint to use `wt switch` for cd, not a warning about missing shell integration.
#[test]
fn test_switch_existing_with_shell_integration_configured() {
    use std::fs;

    let temp_home = TempDir::new().unwrap();
    let mut repo = setup_switch_repo();

    // Simulate shell integration configured in user's shell rc files
    let zshrc_path = temp_home.path().join(".zshrc");
    fs::write(
        &zshrc_path,
        "# Existing user zsh config\nif command -v wt >/dev/null 2>&1; then eval \"$(command wt config shell init zsh)\"; fi\n",
    )
    .unwrap();

    // Create a worktree first
    repo.add_worktree("shell-configured");

    // Switch to existing worktree - should show success + "cd with: wt switch" hint
    // NOT the warning about "cannot cd (no shell integration)"
    snapshot_switch_with_home(
        "switch_existing_with_shell_configured",
        &repo,
        &["shell-configured"],
        Some(temp_home.path()),
        &[],
    );
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
    let repo = setup_switch_repo();

    snapshot_switch(
        "switch_base_without_create",
        &repo,
        &["--base", "main", "main"],
    );
}

#[test]
fn test_switch_internal_mode() {
    let repo = setup_switch_repo();

    snapshot_switch_with_global_flags(
        "switch_internal_mode",
        &repo,
        &["--create", "internal-test"],
        &["--internal"],
    );
}

#[test]
fn test_switch_existing_worktree_internal() {
    let mut repo = setup_switch_repo();

    repo.add_worktree("existing-wt");

    snapshot_switch_with_global_flags(
        "switch_existing_internal",
        &repo,
        &["existing-wt"],
        &["--internal"],
    );
}

#[test]
fn test_switch_internal_with_execute() {
    let repo = setup_switch_repo();

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
    let mut repo = setup_switch_repo();

    // Create a worktree
    let wt_path = repo.add_worktree("missing-wt");

    // Remove the worktree directory (but leave it registered in git)
    std::fs::remove_dir_all(&wt_path).unwrap();

    // Try to switch to the missing worktree (should fail)
    snapshot_switch("switch_error_missing_directory", &repo, &["missing-wt"]);
}

/// Skipped on Windows: snapshot output differs due to shell/path differences.
#[cfg_attr(windows, ignore)]
#[test]
fn test_switch_execute_success() {
    let repo = setup_switch_repo();

    snapshot_switch(
        "switch_execute_success",
        &repo,
        &["--create", "exec-test", "--execute", "echo 'test output'"],
    );
}

/// Skipped on Windows: snapshot output differs due to shell/path differences.
#[cfg_attr(windows, ignore)]
#[test]
fn test_switch_execute_creates_file() {
    let repo = setup_switch_repo();

    let create_file_cmd = "echo 'test content' > test.txt";

    snapshot_switch(
        "switch_execute_creates_file",
        &repo,
        &["--create", "file-test", "--execute", create_file_cmd],
    );
}

/// Skipped on Windows: Uses Unix shell command `exit 1`.
#[test]
#[cfg_attr(windows, ignore)]
fn test_switch_execute_failure() {
    let repo = setup_switch_repo();

    snapshot_switch(
        "switch_execute_failure",
        &repo,
        &["--create", "fail-test", "--execute", "exit 1"],
    );
}

/// Skipped on Windows: snapshot output differs due to shell/path differences.
#[cfg_attr(windows, ignore)]
#[test]
fn test_switch_execute_with_existing_worktree() {
    let mut repo = setup_switch_repo();

    // Create a worktree first
    repo.add_worktree("existing-exec");

    let create_file_cmd = "echo 'existing worktree' > existing.txt";

    snapshot_switch(
        "switch_execute_existing",
        &repo,
        &["existing-exec", "--execute", create_file_cmd],
    );
}

/// Skipped on Windows: snapshot output differs due to shell/path differences.
#[cfg_attr(windows, ignore)]
#[test]
fn test_switch_execute_multiline() {
    let repo = setup_switch_repo();

    let multiline_cmd = "echo 'line1'\necho 'line2'\necho 'line3'";

    snapshot_switch(
        "switch_execute_multiline",
        &repo,
        &["--create", "multiline-test", "--execute", multiline_cmd],
    );
}

/// Skipped on Windows: snapshot output differs due to shell/path differences.
#[cfg_attr(windows, ignore)]
#[test]
fn test_switch_no_config_commands_execute_still_runs() {
    let repo = setup_switch_repo();

    snapshot_switch(
        "switch_no_hooks_execute_still_runs",
        &repo,
        &[
            "--create",
            "no-hooks-test",
            "--execute",
            "echo 'execute command runs'",
            "--no-verify",
        ],
    );
}

#[test]
fn test_switch_no_config_commands_skips_post_start_commands() {
    use std::fs;
    use tempfile::TempDir;

    let temp_home = TempDir::new().unwrap();
    let repo = setup_switch_repo();

    // Create project config with a command that would create a file
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();

    let create_file_cmd = "echo 'marker' > marker.txt";

    fs::write(
        config_dir.join("wt.toml"),
        format!(r#"post-starts = ["{}"]"#, create_file_cmd),
    )
    .unwrap();

    repo.commit("Add config");

    // Pre-approve the command
    let user_config_dir = temp_home.path().join(".config/worktrunk");
    fs::create_dir_all(&user_config_dir).unwrap();
    fs::write(
        user_config_dir.join("config.toml"),
        format!(
            r#"worktree-path = "../{{{{ main_worktree }}}}.{{{{ branch }}}}"

[projects."main"]
approved-commands = ["{}"]
"#,
            create_file_cmd
        ),
    )
    .unwrap();

    // With --no-verify, the post-start command should be skipped
    snapshot_switch_with_home(
        "switch_no_hooks_skips_post_start",
        &repo,
        &["--create", "no-post-start", "--no-verify"],
        Some(temp_home.path()),
        &[],
    );
}

/// Skipped on Windows: snapshot output differs due to shell/path differences.
#[cfg_attr(windows, ignore)]
#[test]
fn test_switch_no_config_commands_with_existing_worktree() {
    let mut repo = setup_switch_repo();

    // Create a worktree first
    repo.add_worktree("existing-no-hooks");

    // With --no-verify, the --execute command should still run
    snapshot_switch(
        "switch_no_hooks_existing",
        &repo,
        &[
            "existing-no-hooks",
            "--execute",
            "echo 'execute still runs'",
            "--no-verify",
        ],
    );
}

#[test]
fn test_switch_no_config_commands_with_force() {
    use std::fs;
    use tempfile::TempDir;

    let temp_home = TempDir::new().unwrap();
    let repo = setup_switch_repo();

    // Create project config with a command
    let config_dir = repo.root_path().join(".config");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("wt.toml"),
        r#"post-starts = ["echo 'test'"]"#,
    )
    .unwrap();

    repo.commit("Add config");

    // With --no-verify, even --force shouldn't execute config commands
    snapshot_switch_with_home(
        "switch_no_hooks_with_force",
        &repo,
        &["--create", "force-no-hooks", "--force", "--no-verify"],
        Some(temp_home.path()),
        &[],
    );
}

#[test]
fn test_switch_create_no_remote() {
    let repo = setup_switch_repo();
    // Deliberately NOT calling setup_remote to test local branch inference

    // Create a branch without specifying base - should infer default branch locally
    snapshot_switch("switch_create_no_remote", &repo, &["--create", "feature"]);
}

#[test]
fn test_switch_primary_on_different_branch() {
    let mut repo = setup_switch_repo();

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
    repo.add_worktree("existing-branch");
    snapshot_switch(
        "switch_to_existing_primary_on_different_branch",
        &repo,
        &["existing-branch"],
    );
}

#[test]
fn test_switch_previous_branch_no_history() {
    let repo = setup_switch_repo();

    // No checkout history, so wt switch - should fail with helpful error
    snapshot_switch("switch_previous_branch_no_history", &repo, &["-"]);
}

#[test]
fn test_switch_main_branch() {
    use std::process::Command;

    let repo = setup_switch_repo();

    // Create a feature branch
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["branch", "feature-a"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    // Switch to feature-a first
    snapshot_switch("switch_main_branch_to_feature", &repo, &["feature-a"]);

    // Now wt switch ^ should resolve to main
    snapshot_switch("switch_main_branch", &repo, &["^"]);
}

#[test]
fn test_create_with_base_main() {
    let repo = setup_switch_repo();

    // Create new branch from main using ^
    snapshot_switch(
        "create_with_base_main",
        &repo,
        &["--create", "new-feature", "--base", "^"],
    );
}

#[test]
fn test_switch_default_branch_missing_worktree() {
    let repo = setup_switch_repo();

    // Move the primary worktree off the default branch so no worktree holds it
    repo.switch_primary_to("develop");

    snapshot_switch("switch_default_branch_missing_worktree", &repo, &["main"]);
}

/// Test that --execute with exit code is emitted in directive mode shell script.
/// The shell wrapper will eval this script and propagate the exit code.
#[test]
fn test_switch_internal_execute_exit_code() {
    let repo = setup_switch_repo();

    // wt succeeds (exit 0), but shell script contains "exit 42"
    // Shell wrapper will eval and return 42
    snapshot_switch_with_global_flags(
        "switch_internal_execute_exit_code",
        &repo,
        &["--create", "exit-code-test", "--execute", "exit 42"],
        &["--internal"],
    );
}

/// Test execute command failure propagation in directive mode.
/// When wt succeeds but the execute script would fail, wt still exits 0.
/// The shell wrapper handles the execute command's exit code.
#[test]
fn test_switch_internal_execute_with_output_before_exit() {
    let repo = setup_switch_repo();

    // Execute command outputs then exits with code
    let cmd = "echo 'doing work'\nexit 7";

    snapshot_switch_with_global_flags(
        "switch_internal_execute_output_then_exit",
        &repo,
        &["--create", "output-exit-test", "--execute", cmd],
        &["--internal"],
    );
}

// =============================================================================
// Error message snapshot tests
// =============================================================================

/// Test the `worktree_path_occupied` error when target path exists but isn't a worktree
#[test]
fn test_switch_error_path_occupied() {
    let repo = setup_switch_repo();

    // Calculate where the worktree would be created
    // Default path pattern is {repo_name}.{branch}
    let repo_name = repo.root_path().file_name().unwrap().to_str().unwrap();
    let expected_path = repo
        .root_path()
        .parent()
        .unwrap()
        .join(format!("{}.occupied-branch", repo_name));

    // Create a non-worktree directory at that path
    std::fs::create_dir_all(&expected_path).unwrap();
    std::fs::write(expected_path.join("some_file.txt"), "occupant content").unwrap();

    // Try to create a worktree with a branch that would use that path
    // Should fail with worktree_path_occupied error
    snapshot_switch(
        "switch_error_path_occupied",
        &repo,
        &["--create", "occupied-branch"],
    );

    // Cleanup
    std::fs::remove_dir_all(&expected_path).ok();
}

/// Test that `wt switch -` uses actual current branch for recording history.
///
/// Bug scenario: If user changes worktrees without using `wt switch` (e.g., cd directly),
/// history becomes stale. The fix ensures we always use the actual current branch
/// when recording new history, not any previously stored value.
#[test]
fn test_switch_previous_with_stale_history() {
    use std::process::Command;

    let repo = setup_switch_repo();

    // Create branches with worktrees
    for branch in ["branch-a", "branch-b", "branch-c"] {
        let mut cmd = Command::new("git");
        repo.configure_git_cmd(&mut cmd);
        cmd.args(["branch", branch])
            .current_dir(repo.root_path())
            .output()
            .unwrap();
    }

    // Switch to branch-a, then branch-b to establish history
    snapshot_switch("switch_stale_history_to_a", &repo, &["branch-a"]);
    snapshot_switch("switch_stale_history_to_b", &repo, &["branch-b"]);

    // Now manually set history to simulate user changing worktrees without wt switch.
    // History stores just the previous branch (branch-a from the earlier switches).
    // If user manually cd'd to branch-c's worktree, history would still say branch-a.
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["config", "worktrunk.history", "branch-a"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    // Run wt switch - from branch-b's worktree.
    // Should go to branch-a (what history says), and record actual current branch as new previous.
    snapshot_switch("switch_stale_history_first_dash", &repo, &["-"]);

    // Run wt switch - again.
    // Should go back to wherever we actually were (recorded as new previous in step above)
    snapshot_switch("switch_stale_history_second_dash", &repo, &["-"]);
}

/// Helper to run switch from a specific working directory (simulating actual worktree location)
fn snapshot_switch_from_dir(test_name: &str, repo: &TestRepo, args: &[&str], cwd: &Path) {
    let default_home = TempDir::new().unwrap();
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd_with_global_flags(repo, "switch", args, Some(cwd), &[]);
        cmd.env("HOME", default_home.path());
        // Windows: the `home` crate uses USERPROFILE for home_dir()
        cmd.env("USERPROFILE", default_home.path());
        assert_cmd_snapshot!(test_name, cmd);
    });
}

/// Test realistic ping-pong behavior where we actually run from the correct worktree.
///
/// This simulates real usage with shell integration, where each `wt switch` actually
/// changes the working directory before the next command runs.
#[test]
fn test_switch_ping_pong_realistic() {
    use std::process::Command;

    let repo = setup_switch_repo();

    // Create feature-a branch
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["branch", "feature-a"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    // Step 1: From main worktree, switch to feature-a (creates worktree)
    // History: current=feature-a, previous=main
    snapshot_switch_from_dir(
        "ping_pong_1_main_to_feature_a",
        &repo,
        &["feature-a"],
        repo.root_path(),
    );

    // Calculate feature-a worktree path
    let feature_a_path = repo.root_path().parent().unwrap().join(format!(
        "{}.feature-a",
        repo.root_path().file_name().unwrap().to_str().unwrap()
    ));

    // Step 2: From feature-a worktree, switch back to main
    // History: current=main, previous=feature-a
    snapshot_switch_from_dir(
        "ping_pong_2_feature_a_to_main",
        &repo,
        &["main"],
        &feature_a_path,
    );

    // Step 3: From main worktree, wt switch - should go to feature-a
    // History: current=feature-a, previous=main
    snapshot_switch_from_dir(
        "ping_pong_3_dash_to_feature_a",
        &repo,
        &["-"],
        repo.root_path(),
    );

    // Step 4: From feature-a worktree, wt switch - should go back to main
    // History: current=main, previous=feature-a
    snapshot_switch_from_dir("ping_pong_4_dash_to_main", &repo, &["-"], &feature_a_path);

    // Step 5: From main worktree, wt switch - should go to feature-a again (ping-pong!)
    // History: current=feature-a, previous=main
    snapshot_switch_from_dir(
        "ping_pong_5_dash_to_feature_a_again",
        &repo,
        &["-"],
        repo.root_path(),
    );
}

/// Test that `wt switch` without arguments shows helpful hints about shortcuts.
///
/// Skipped on Windows: Hints display differently on Windows.
#[test]
#[cfg_attr(windows, ignore)]
fn test_switch_missing_argument_shows_hints() {
    let repo = setup_switch_repo();

    // Run switch with no arguments - should show clap error plus hints
    snapshot_switch("switch_missing_argument_hints", &repo, &[]);
}
