use crate::common::{
    TestRepo, make_snapshot_cmd_with_global_flags, repo, repo_with_remote, setup_snapshot_settings,
};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::path::Path;

// Snapshot helpers

fn snapshot_switch(test_name: &str, repo: &TestRepo, args: &[&str]) {
    snapshot_switch_impl(test_name, repo, args, &[], None);
}

fn snapshot_switch_with_global_flags(
    test_name: &str,
    repo: &TestRepo,
    args: &[&str],
    global_flags: &[&str],
) {
    snapshot_switch_impl(test_name, repo, args, global_flags, None);
}

fn snapshot_switch_from_dir(test_name: &str, repo: &TestRepo, args: &[&str], cwd: &Path) {
    snapshot_switch_impl(test_name, repo, args, &[], Some(cwd));
}

fn snapshot_switch_impl(
    test_name: &str,
    repo: &TestRepo,
    args: &[&str],
    global_flags: &[&str],
    cwd: Option<&Path>,
) {
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd_with_global_flags(repo, "switch", args, cwd, global_flags);
        assert_cmd_snapshot!(test_name, cmd);
    });
}
// Basic switch tests
#[rstest]
fn test_switch_create_new_branch(repo: TestRepo) {
    snapshot_switch("switch_create_new", &repo, &["--create", "feature-x"]);
}

#[rstest]
fn test_switch_create_existing_branch_error(mut repo: TestRepo) {
    // Create a branch first
    repo.add_worktree("feature-y");

    // Try to create it again - should error
    snapshot_switch(
        "switch_create_existing_error",
        &repo,
        &["--create", "feature-y"],
    );
}

#[rstest]
fn test_switch_create_with_remote_branch_only(#[from(repo_with_remote)] repo: TestRepo) {
    use std::process::Command;

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

#[rstest]
fn test_switch_existing_branch(mut repo: TestRepo) {
    repo.add_worktree("feature-z");

    // Switch to it (should find existing worktree)
    snapshot_switch("switch_existing_branch", &repo, &["feature-z"]);
}

/// Test switching to existing worktree when shell integration is configured but not active.
///
/// When shell integration is configured in user's rc files (e.g., .zshrc) but the user
/// runs `wt` binary directly (not through the shell wrapper), show a warning that explains
/// the actual situation: shell IS configured, but cd can't happen because we're not
/// running through the shell function.
#[rstest]
fn test_switch_existing_with_shell_integration_configured(mut repo: TestRepo) {
    use std::fs;

    // Create a worktree first
    repo.add_worktree("shell-configured");

    // Simulate shell integration configured in user's shell rc files
    // (repo.home_path() is automatically set as HOME by clean_cli_env)
    let zshrc_path = repo.home_path().join(".zshrc");
    fs::write(
        &zshrc_path,
        "# Existing user zsh config\nif command -v wt >/dev/null 2>&1; then eval \"$(command wt config shell init zsh)\"; fi\n",
    )
    .unwrap();

    // Switch to existing worktree - should show warning about binary invoked directly
    // (different from "no shell integration" warning when shell is not configured at all)
    snapshot_switch(
        "switch_existing_with_shell_configured",
        &repo,
        &["shell-configured"],
    );
}

#[rstest]
fn test_switch_with_base_branch(repo: TestRepo) {
    repo.commit("Initial commit on main");

    snapshot_switch(
        "switch_with_base",
        &repo,
        &["--create", "--base", "main", "feature-with-base"],
    );
}

#[rstest]
fn test_switch_base_without_create_warning(repo: TestRepo) {
    snapshot_switch(
        "switch_base_without_create",
        &repo,
        &["--base", "main", "main"],
    );
}
// Internal mode tests
#[rstest]
fn test_switch_internal_mode(repo: TestRepo) {
    snapshot_switch_with_global_flags(
        "switch_internal_mode",
        &repo,
        &["--create", "internal-test"],
        &["--internal"],
    );
}

#[rstest]
fn test_switch_existing_worktree_internal(mut repo: TestRepo) {
    repo.add_worktree("existing-wt");

    snapshot_switch_with_global_flags(
        "switch_existing_internal",
        &repo,
        &["existing-wt"],
        &["--internal"],
    );
}

#[rstest]
fn test_switch_internal_with_execute(repo: TestRepo) {
    let execute_cmd = "echo 'line1'\necho 'line2'";

    snapshot_switch_with_global_flags(
        "switch_internal_with_execute",
        &repo,
        &["--create", "exec-internal", "--execute", execute_cmd],
        &["--internal"],
    );
}
// Error tests
#[rstest]
fn test_switch_error_missing_worktree_directory(mut repo: TestRepo) {
    let wt_path = repo.add_worktree("missing-wt");

    // Remove the worktree directory (but leave it registered in git)
    std::fs::remove_dir_all(&wt_path).unwrap();

    // Try to switch to the missing worktree (should fail)
    snapshot_switch("switch_error_missing_directory", &repo, &["missing-wt"]);
}

/// Test the `worktree_path_occupied` error when target path exists but isn't a worktree
#[rstest]
fn test_switch_error_path_occupied(repo: TestRepo) {
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
// Execute flag tests
#[rstest]
fn test_switch_execute_success(repo: TestRepo) {
    snapshot_switch(
        "switch_execute_success",
        &repo,
        &["--create", "exec-test", "--execute", "echo 'test output'"],
    );
}

#[rstest]
fn test_switch_execute_creates_file(repo: TestRepo) {
    let create_file_cmd = "echo 'test content' > test.txt";

    snapshot_switch(
        "switch_execute_creates_file",
        &repo,
        &["--create", "file-test", "--execute", create_file_cmd],
    );
}

#[rstest]
fn test_switch_execute_failure(repo: TestRepo) {
    snapshot_switch(
        "switch_execute_failure",
        &repo,
        &["--create", "fail-test", "--execute", "exit 1"],
    );
}

#[rstest]
fn test_switch_execute_with_existing_worktree(mut repo: TestRepo) {
    repo.add_worktree("existing-exec");

    let create_file_cmd = "echo 'existing worktree' > existing.txt";

    snapshot_switch(
        "switch_execute_existing",
        &repo,
        &["existing-exec", "--execute", create_file_cmd],
    );
}

#[rstest]
fn test_switch_execute_multiline(repo: TestRepo) {
    let multiline_cmd = "echo 'line1'\necho 'line2'\necho 'line3'";

    snapshot_switch(
        "switch_execute_multiline",
        &repo,
        &["--create", "multiline-test", "--execute", multiline_cmd],
    );
}
// --no-verify flag tests
#[rstest]
fn test_switch_no_config_commands_execute_still_runs(repo: TestRepo) {
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

#[rstest]
fn test_switch_no_config_commands_skips_post_start_commands(repo: TestRepo) {
    use std::fs;

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

    // Pre-approve the command (repo.home_path() is automatically set as HOME)
    let user_config_dir = repo.home_path().join(".config/worktrunk");
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
    snapshot_switch(
        "switch_no_hooks_skips_post_start",
        &repo,
        &["--create", "no-post-start", "--no-verify"],
    );
}

#[rstest]
fn test_switch_no_config_commands_with_existing_worktree(mut repo: TestRepo) {
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

#[rstest]
fn test_switch_no_config_commands_with_force(repo: TestRepo) {
    use std::fs;

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
    // (HOME is automatically set to repo.home_path() by clean_cli_env)
    snapshot_switch(
        "switch_no_hooks_with_force",
        &repo,
        &["--create", "force-no-hooks", "--force", "--no-verify"],
    );
}
// Branch inference and special branch tests
#[rstest]
fn test_switch_create_no_remote(repo: TestRepo) {
    // Deliberately NOT calling setup_remote to test local branch inference
    // Create a branch without specifying base - should infer default branch locally
    snapshot_switch("switch_create_no_remote", &repo, &["--create", "feature"]);
}

#[rstest]
fn test_switch_primary_on_different_branch(mut repo: TestRepo) {
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

#[rstest]
fn test_switch_previous_branch_no_history(repo: TestRepo) {
    // No checkout history, so wt switch - should fail with helpful error
    snapshot_switch("switch_previous_branch_no_history", &repo, &["-"]);
}

#[rstest]
fn test_switch_main_branch(repo: TestRepo) {
    use std::process::Command;

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

#[rstest]
fn test_create_with_base_main(repo: TestRepo) {
    // Create new branch from main using ^
    snapshot_switch(
        "create_with_base_main",
        &repo,
        &["--create", "new-feature", "--base", "^"],
    );
}

/// Test that normal switch (branch matches) does NOT show warning
#[rstest]
fn test_switch_no_warning_when_branch_matches(mut repo: TestRepo) {
    // Create a worktree for "feature" branch (normal case)
    repo.add_worktree("feature");

    // Switch to feature with shell integration - should NOT show any warning
    snapshot_switch_with_global_flags(
        "switch_no_warning_when_branch_matches",
        &repo,
        &["feature"],
        &["--internal"],
    );
}

/// Test switching to a worktree at an unexpected path shows a hint
#[rstest]
fn test_switch_path_mismatch_shows_hint(repo: TestRepo) {
    use std::process::Command;

    // Create a worktree at a non-standard path (sibling to repo, not following template)
    let wrong_path = repo.root_path().parent().unwrap().join("wrong-path");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args([
        "worktree",
        "add",
        wrong_path.to_str().unwrap(),
        "-b",
        "feature",
    ])
    .current_dir(repo.root_path())
    .output()
    .unwrap();

    // Switch to feature - should show hint about path mismatch
    snapshot_switch_with_global_flags(
        "switch_path_mismatch_shows_hint",
        &repo,
        &["feature"],
        &["--internal"],
    );
}

/// Test that switching to a branch errors when path is occupied by worktree on different branch
///
/// With branch-first lookup, if a worktree was created for "feature" but then switched to
/// "bugfix", `wt switch feature` can't find it (since it looks by branch name). When it
/// tries to create a new worktree, it fails because the path exists. The hint shows what
/// branch currently occupies the path.
#[rstest]
fn test_switch_error_path_occupied_different_branch(repo: TestRepo) {
    use std::process::Command;

    // Create a worktree for "feature" branch at expected path
    let feature_path = repo.root_path().parent().unwrap().join("repo.feature");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args([
        "worktree",
        "add",
        feature_path.to_str().unwrap(),
        "-b",
        "feature",
    ])
    .current_dir(repo.root_path())
    .output()
    .unwrap();

    // Switch that worktree to a different branch "bugfix"
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["switch", "-c", "bugfix"])
        .current_dir(&feature_path)
        .output()
        .unwrap();

    // Switch to feature - should error since path is occupied by bugfix worktree
    snapshot_switch_with_global_flags(
        "switch_error_path_occupied_different_branch",
        &repo,
        &["feature"],
        &["--internal"],
    );
}

/// Test that switching to a branch errors when path is occupied by detached HEAD worktree
#[rstest]
fn test_switch_error_path_occupied_detached(repo: TestRepo) {
    use std::process::Command;

    // Create a worktree for "feature" branch at expected path
    let feature_path = repo.root_path().parent().unwrap().join("repo.feature");
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args([
        "worktree",
        "add",
        feature_path.to_str().unwrap(),
        "-b",
        "feature",
    ])
    .current_dir(repo.root_path())
    .output()
    .unwrap();

    // Get the HEAD commit and detach
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    let output = cmd
        .args(["rev-parse", "HEAD"])
        .current_dir(&feature_path)
        .output()
        .unwrap();
    let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["checkout", "--detach", &commit])
        .current_dir(&feature_path)
        .output()
        .unwrap();

    // Switch to feature - should error since path is occupied by detached worktree
    snapshot_switch_with_global_flags(
        "switch_error_path_occupied_detached",
        &repo,
        &["feature"],
        &["--internal"],
    );
}

/// Test switching to default branch when main worktree is on a different branch
///
/// When the main worktree (repo root) has been switched to a feature branch via
/// `git checkout feature`, `wt switch main` should show the path-occupied error
/// with helpful commands to switch it back.
#[rstest]
fn test_switch_error_main_worktree_on_different_branch(repo: TestRepo) {
    use std::process::Command;

    // Switch the main worktree to a different branch
    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["checkout", "-b", "feature"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    // Now try to switch to main - should error because main worktree is on feature
    snapshot_switch_with_global_flags(
        "switch_error_main_worktree_on_different_branch",
        &repo,
        &["main"],
        &["--internal"],
    );
}

// Internal mode with execute tests
/// Test that --execute with exit code is emitted in directive mode shell script.
/// The shell wrapper will eval this script and propagate the exit code.
#[rstest]
fn test_switch_internal_execute_exit_code(repo: TestRepo) {
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
#[rstest]
fn test_switch_internal_execute_with_output_before_exit(repo: TestRepo) {
    // Execute command outputs then exits with code
    let cmd = "echo 'doing work'\nexit 7";

    snapshot_switch_with_global_flags(
        "switch_internal_execute_output_then_exit",
        &repo,
        &["--create", "output-exit-test", "--execute", cmd],
        &["--internal"],
    );
}
// History and ping-pong tests
/// Test that `wt switch -` uses actual current branch for recording history.
///
/// Bug scenario: If user changes worktrees without using `wt switch` (e.g., cd directly),
/// history becomes stale. The fix ensures we always use the actual current branch
/// when recording new history, not any previously stored value.
#[rstest]
fn test_switch_previous_with_stale_history(repo: TestRepo) {
    use std::process::Command;

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

/// Test realistic ping-pong behavior where we actually run from the correct worktree.
///
/// This simulates real usage with shell integration, where each `wt switch` actually
/// changes the working directory before the next command runs.
#[rstest]
fn test_switch_ping_pong_realistic(repo: TestRepo) {
    use std::process::Command;

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
#[rstest]
fn test_switch_missing_argument_shows_hints(repo: TestRepo) {
    // Run switch with no arguments - should show clap error plus hints
    snapshot_switch("switch_missing_argument_hints", &repo, &[]);
}

/// Test that --execute commands can read from stdin (stdin inheritance).
///
/// This verifies the fix for non-Unix platforms where stdin was incorrectly
/// set to Stdio::null() instead of Stdio::inherit(), breaking interactive
/// programs like `vim`, `python -i`, or `claude`.
///
/// The test pipes input to `wt switch --execute "cat"` and verifies the
/// cat command receives and outputs that input, proving stdin was inherited.
#[rstest]
fn test_switch_execute_stdin_inheritance(repo: TestRepo) {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let test_input = "stdin_inheritance_test_content\n";

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wt"));
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["switch", "--create", "stdin-test", "--execute", "cat"])
        .current_dir(repo.root_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().expect("failed to spawn wt");

    // Write test input to stdin and close it to signal EOF
    {
        let stdin = child.stdin.as_mut().expect("failed to get stdin");
        stdin
            .write_all(test_input.as_bytes())
            .expect("failed to write to stdin");
    }

    let output = child.wait_with_output().expect("failed to wait for child");
    let stdout = String::from_utf8_lossy(&output.stdout);

    // The cat command should have received our input via inherited stdin
    // and echoed it to stdout
    assert!(
        stdout.contains("stdin_inheritance_test_content"),
        "Expected cat to receive piped stdin. Got stdout: {}\nstderr: {}",
        stdout,
        String::from_utf8_lossy(&output.stderr)
    );
}
