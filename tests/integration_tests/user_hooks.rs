//! Integration tests for user-level hooks (~/.config/worktrunk/config.toml)
//!
//! User hooks differ from project hooks:
//! - Run for all repositories
//! - Execute before project hooks
//! - Don't require approval
//! - Skipped together with project hooks via --no-verify

use crate::common::{
    TestRepo, make_snapshot_cmd, repo, resolve_git_common_dir, setup_snapshot_settings,
    wait_for_file, wait_for_file_content, wait_for_file_count,
};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::fs;
use std::thread;
use std::time::Duration;

// Note: Duration is still imported for SLEEP_FOR_ABSENCE_CHECK (testing command did NOT run)

/// Wait duration when checking file absence (testing command did NOT run).
const SLEEP_FOR_ABSENCE_CHECK: Duration = Duration::from_millis(500);

// ============================================================================
// User Post-Create Hook Tests
// ============================================================================

/// Helper to create snapshot for switch commands
fn snapshot_switch(test_name: &str, repo: &TestRepo, args: &[&str]) {
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(repo, "switch", args, None);
        assert_cmd_snapshot!(test_name, cmd);
    });
}

#[rstest]
fn test_user_post_create_hook_executes(repo: TestRepo) {
    // Write user config with post-create hook (no project config)
    repo.write_test_config(
        r#"[post-create]
log = "echo 'USER_POST_CREATE_RAN' > user_hook_marker.txt"
"#,
    );

    snapshot_switch("user_post_create_executes", &repo, &["--create", "feature"]);

    // Verify user hook actually ran
    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    let marker_file = worktree_path.join("user_hook_marker.txt");
    assert!(
        marker_file.exists(),
        "User post-create hook should have created marker file"
    );

    let contents = fs::read_to_string(&marker_file).unwrap();
    assert!(
        contents.contains("USER_POST_CREATE_RAN"),
        "Marker file should contain expected content"
    );
}

#[rstest]
fn test_user_hooks_run_before_project_hooks(repo: TestRepo) {
    // Create project config with post-create hook
    repo.write_project_config(r#"post-create = "echo 'PROJECT_HOOK' >> hook_order.txt""#);
    repo.commit("Add project config");

    // Write user config with user hook AND pre-approve project command
    repo.write_test_config(
        r#"[post-create]
log = "echo 'USER_HOOK' >> hook_order.txt"

[projects."../origin"]
approved-commands = ["echo 'PROJECT_HOOK' >> hook_order.txt"]
"#,
    );

    snapshot_switch("user_hooks_before_project", &repo, &["--create", "feature"]);

    // Verify execution order
    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    let order_file = worktree_path.join("hook_order.txt");
    assert!(order_file.exists());

    let contents = fs::read_to_string(&order_file).unwrap();
    let lines: Vec<&str> = contents.lines().collect();

    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0], "USER_HOOK", "User hook should run first");
    assert_eq!(lines[1], "PROJECT_HOOK", "Project hook should run second");
}

#[rstest]
fn test_user_hooks_no_approval_required(repo: TestRepo) {
    // Write user config with hook but NO pre-approved commands
    // (unlike project hooks, user hooks don't require approval)
    repo.write_test_config(
        r#"[post-create]
setup = "echo 'NO_APPROVAL_NEEDED' > no_approval.txt"
"#,
    );

    snapshot_switch("user_hooks_no_approval", &repo, &["--create", "feature"]);

    // Verify hook ran without approval
    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    let marker_file = worktree_path.join("no_approval.txt");
    assert!(
        marker_file.exists(),
        "User hook should run without pre-approval"
    );
}

#[rstest]
fn test_no_verify_flag_skips_all_hooks(repo: TestRepo) {
    // Create project config with post-create hook
    repo.write_project_config(r#"post-create = "echo 'PROJECT_HOOK' > project_marker.txt""#);
    repo.commit("Add project config");

    // Write user config with both user hook and pre-approved project command
    repo.write_test_config(
        r#"[post-create]
log = "echo 'USER_HOOK' > user_marker.txt"

[projects."../origin"]
approved-commands = ["echo 'PROJECT_HOOK' > project_marker.txt"]
"#,
    );

    // Create worktree with --no-verify (skips ALL hooks)
    snapshot_switch(
        "no_verify_skips_all_hooks",
        &repo,
        &["--create", "feature", "--no-verify"],
    );

    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");

    // User hook should NOT have run
    let user_marker = worktree_path.join("user_marker.txt");
    assert!(
        !user_marker.exists(),
        "User hook should be skipped with --no-verify"
    );

    // Project hook should also NOT have run (--no-verify skips ALL hooks)
    let project_marker = worktree_path.join("project_marker.txt");
    assert!(
        !project_marker.exists(),
        "Project hook should also be skipped with --no-verify"
    );
}

#[rstest]
fn test_user_post_create_hook_failure(repo: TestRepo) {
    // Write user config with failing hook
    repo.write_test_config(
        r#"[post-create]
failing = "exit 1"
"#,
    );

    // Failing user hook should produce warning but not block creation
    snapshot_switch("user_post_create_failure", &repo, &["--create", "feature"]);

    // Worktree should still be created despite hook failure
    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    assert!(
        worktree_path.exists(),
        "Worktree should be created even if post-create hook fails"
    );
}

// ============================================================================
// User Post-Start Hook Tests (Background)
// ============================================================================

#[rstest]
fn test_user_post_start_hook_executes(repo: TestRepo) {
    // Write user config with post-start hook (background)
    repo.write_test_config(
        r#"[post-start]
bg = "echo 'USER_POST_START_RAN' > user_bg_marker.txt"
"#,
    );

    snapshot_switch("user_post_start_executes", &repo, &["--create", "feature"]);

    // Wait for background hook to complete and write content
    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    let marker_file = worktree_path.join("user_bg_marker.txt");
    wait_for_file_content(&marker_file);

    let contents = fs::read_to_string(&marker_file).unwrap();
    assert!(
        contents.contains("USER_POST_START_RAN"),
        "User post-start hook should have run in background"
    );
}

#[rstest]
fn test_user_post_start_skipped_with_no_verify(repo: TestRepo) {
    // Write user config with post-start hook
    repo.write_test_config(
        r#"[post-start]
bg = "echo 'USER_BG' > user_bg_marker.txt"
"#,
    );

    snapshot_switch(
        "user_post_start_skipped_no_verify",
        &repo,
        &["--create", "feature", "--no-verify"],
    );

    // Wait to ensure background hook would have had time to run
    thread::sleep(SLEEP_FOR_ABSENCE_CHECK);

    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    let marker_file = worktree_path.join("user_bg_marker.txt");
    assert!(
        !marker_file.exists(),
        "User post-start hook should be skipped with --no-verify"
    );
}

// ============================================================================
// User Pre-Merge Hook Tests
// ============================================================================

/// Helper for merge snapshots
fn snapshot_merge(test_name: &str, repo: &TestRepo, args: &[&str], cwd: Option<&std::path::Path>) {
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(repo, "merge", args, cwd);
        assert_cmd_snapshot!(test_name, cmd);
    });
}

#[rstest]
fn test_user_pre_merge_hook_executes(mut repo: TestRepo) {
    // Create feature worktree with a commit
    let feature_wt =
        repo.add_worktree_with_commit("feature", "feature.txt", "feature content", "Add feature");

    // Write user config with pre-merge hook
    repo.write_test_config(
        r#"[pre-merge]
check = "echo 'USER_PRE_MERGE_RAN' > user_premerge.txt"
"#,
    );

    snapshot_merge(
        "user_pre_merge_executes",
        &repo,
        &["main", "--yes", "--no-remove"],
        Some(&feature_wt),
    );

    // Verify user hook ran
    let marker_file = feature_wt.join("user_premerge.txt");
    assert!(marker_file.exists(), "User pre-merge hook should have run");
}

#[rstest]
fn test_user_pre_merge_hook_failure_blocks_merge(mut repo: TestRepo) {
    // Create feature worktree with a commit
    let feature_wt =
        repo.add_worktree_with_commit("feature", "feature.txt", "feature content", "Add feature");

    // Write user config with failing pre-merge hook
    repo.write_test_config(
        r#"[pre-merge]
check = "exit 1"
"#,
    );

    // Failing pre-merge hook should block the merge
    snapshot_merge(
        "user_pre_merge_failure",
        &repo,
        &["main", "--yes", "--no-remove"],
        Some(&feature_wt),
    );
}

#[rstest]
fn test_user_pre_merge_skipped_with_no_verify(mut repo: TestRepo) {
    // Create feature worktree with a commit
    let feature_wt =
        repo.add_worktree_with_commit("feature", "feature.txt", "feature content", "Add feature");

    // Write user config with pre-merge hook that creates a marker
    repo.write_test_config(
        r#"[pre-merge]
check = "echo 'USER_PRE_MERGE' > user_premerge_marker.txt"
"#,
    );

    snapshot_merge(
        "user_pre_merge_skipped_no_verify",
        &repo,
        &["main", "--yes", "--no-remove", "--no-verify"],
        Some(&feature_wt),
    );

    // User hook should NOT have run (--no-verify skips all hooks)
    let marker_file = feature_wt.join("user_premerge_marker.txt");
    assert!(
        !marker_file.exists(),
        "User pre-merge hook should be skipped with --no-verify"
    );
}

///
/// Real Ctrl-C sends SIGINT to the entire foreground process group. We simulate this by:
/// 1. Spawning wt in its own process group (so we don't kill the test runner)
/// 2. Sending SIGINT to that process group (which includes wt and its hook children)
#[rstest]
#[cfg(unix)]
fn test_pre_merge_hook_receives_sigint(repo: TestRepo) {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;

    repo.commit("Initial commit");

    // Project pre-merge hook: write start, then sleep, then write done (if not interrupted)
    repo.write_project_config(
        r#"[pre-merge]
long = "sh -c 'echo start >> hook.log; sleep 30; echo done >> hook.log'"
"#,
    );
    repo.commit("Add pre-merge hook");

    // Spawn wt in its own process group (so SIGINT to that group doesn't kill the test)
    let mut cmd = crate::common::wt_command();
    cmd.current_dir(repo.root_path());
    cmd.args(["hook", "pre-merge", "--yes"]);
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());
    cmd.process_group(0); // wt becomes leader of its own process group
    let mut child = cmd.spawn().expect("failed to spawn wt hook pre-merge");

    // Wait until hook writes "start" to hook.log (verifies the hook is running)
    let hook_log = repo.root_path().join("hook.log");
    wait_for_file_content(&hook_log);

    // Send SIGINT to wt's process group (wt's PID == its PGID since it's the leader)
    // This simulates real Ctrl-C which sends SIGINT to the foreground process group
    let wt_pgid = Pid::from_raw(child.id() as i32);
    kill(Pid::from_raw(-wt_pgid.as_raw()), Signal::SIGINT).expect("failed to send SIGINT to pgrp");

    let status = child.wait().expect("failed to wait for wt");

    // wt was killed by signal, so code() returns None and we check the signal
    use std::os::unix::process::ExitStatusExt;
    assert!(
        status.signal() == Some(2) || status.code() == Some(130),
        "wt should be killed by SIGINT (signal 2) or exit 130, got: {status:?}"
    );

    // Give the (killed) hook a moment; it must not append "done"
    thread::sleep(Duration::from_millis(500));

    let mut contents = String::new();
    std::fs::File::open(&hook_log)
        .unwrap()
        .read_to_string(&mut contents)
        .unwrap();
    assert!(
        contents.trim() == "start",
        "hook should not have reached 'done'; got: {contents:?}"
    );
}

#[rstest]
#[cfg(unix)]
fn test_pre_merge_hook_receives_sigterm(repo: TestRepo) {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;

    repo.commit("Initial commit");

    // Project pre-merge hook: write start, then sleep, then write done (if not interrupted)
    repo.write_project_config(
        r#"[pre-merge]
long = "sh -c 'echo start >> hook.log; sleep 30; echo done >> hook.log'"
"#,
    );
    repo.commit("Add pre-merge hook");

    // Spawn wt in its own process group (so SIGTERM to that group doesn't kill the test)
    let mut cmd = crate::common::wt_command();
    cmd.current_dir(repo.root_path());
    cmd.args(["hook", "pre-merge", "--yes"]);
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());
    cmd.process_group(0); // wt becomes leader of its own process group
    let mut child = cmd.spawn().expect("failed to spawn wt hook pre-merge");

    // Wait until hook writes "start" to hook.log (verifies the hook is running)
    let hook_log = repo.root_path().join("hook.log");
    wait_for_file_content(&hook_log);

    // Send SIGTERM to wt's process group (wt's PID == its PGID since it's the leader)
    let wt_pgid = Pid::from_raw(child.id() as i32);
    kill(Pid::from_raw(-wt_pgid.as_raw()), Signal::SIGTERM)
        .expect("failed to send SIGTERM to pgrp");

    let status = child.wait().expect("failed to wait for wt");

    // wt was killed by signal, so code() returns None and we check the signal
    use std::os::unix::process::ExitStatusExt;
    assert!(
        status.signal() == Some(15) || status.code() == Some(143),
        "wt should be killed by SIGTERM (signal 15) or exit 143, got: {status:?}"
    );

    // Give the (killed) hook a moment; it must not append "done"
    thread::sleep(Duration::from_millis(500));

    let mut contents = String::new();
    std::fs::File::open(&hook_log)
        .unwrap()
        .read_to_string(&mut contents)
        .unwrap();
    assert!(
        contents.trim() == "start",
        "hook should not have reached 'done'; got: {contents:?}"
    );
}

// ============================================================================
// User Post-Merge Hook Tests
// ============================================================================

#[rstest]
fn test_user_post_merge_hook_executes(mut repo: TestRepo) {
    // Create feature worktree with a commit
    let feature_wt =
        repo.add_worktree_with_commit("feature", "feature.txt", "feature content", "Add feature");

    // Write user config with post-merge hook
    repo.write_test_config(
        r#"[post-merge]
notify = "echo 'USER_POST_MERGE_RAN' > user_postmerge.txt"
"#,
    );

    snapshot_merge(
        "user_post_merge_executes",
        &repo,
        &["main", "--yes", "--no-remove"],
        Some(&feature_wt),
    );

    // Post-merge runs in the destination (main) worktree
    let main_worktree = repo.root_path();
    let marker_file = main_worktree.join("user_postmerge.txt");
    assert!(
        marker_file.exists(),
        "User post-merge hook should have run in main worktree"
    );
}

// ============================================================================
// User Pre-Remove Hook Tests
// ============================================================================

/// Helper for remove snapshots
fn snapshot_remove(test_name: &str, repo: &TestRepo, args: &[&str], cwd: Option<&std::path::Path>) {
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(repo, "remove", args, cwd);
        assert_cmd_snapshot!(test_name, cmd);
    });
}

#[rstest]
fn test_user_pre_remove_hook_executes(mut repo: TestRepo) {
    // Create a worktree to remove
    let _feature_wt = repo.add_worktree("feature");

    // Write user config with pre-remove hook
    // Hook writes to parent dir (temp dir) since the worktree itself gets removed
    repo.write_test_config(
        r#"[pre-remove]
cleanup = "echo 'USER_PRE_REMOVE_RAN' > ../user_preremove_marker.txt"
"#,
    );

    snapshot_remove(
        "user_pre_remove_executes",
        &repo,
        &["feature", "--force-delete"],
        Some(repo.root_path()),
    );

    // Verify user hook ran (writes to parent dir since worktree is being removed)
    let marker_file = repo
        .root_path()
        .parent()
        .unwrap()
        .join("user_preremove_marker.txt");
    assert!(marker_file.exists(), "User pre-remove hook should have run");
}

#[rstest]
fn test_user_pre_remove_failure_blocks_removal(mut repo: TestRepo) {
    // Create a worktree to remove
    let feature_wt = repo.add_worktree("feature");

    // Write user config with failing pre-remove hook
    repo.write_test_config(
        r#"[pre-remove]
block = "exit 1"
"#,
    );

    snapshot_remove(
        "user_pre_remove_failure",
        &repo,
        &["feature", "--force-delete"],
        Some(repo.root_path()),
    );

    // Worktree should still exist (removal blocked by failing hook)
    assert!(
        feature_wt.exists(),
        "Worktree should not be removed when pre-remove hook fails"
    );
}

#[rstest]
fn test_user_pre_remove_skipped_with_no_verify(mut repo: TestRepo) {
    // Create a worktree to remove
    let feature_wt = repo.add_worktree("feature");

    // Write user config with pre-remove hook that would block
    repo.write_test_config(
        r#"[pre-remove]
block = "exit 1"
"#,
    );

    // With --no-verify, all hooks (including the failing one) should be skipped
    snapshot_remove(
        "user_pre_remove_skipped_no_verify",
        &repo,
        &["feature", "--force-delete", "--no-verify"],
        Some(repo.root_path()),
    );

    // Worktree should be removed (hooks skipped)
    // Background removal needs time to complete
    let timeout = Duration::from_secs(5);
    let poll_interval = Duration::from_millis(50);
    let start = std::time::Instant::now();
    while feature_wt.exists() && start.elapsed() < timeout {
        thread::sleep(poll_interval);
    }
    assert!(
        !feature_wt.exists(),
        "Worktree should be removed when --no-verify skips failing hook"
    );
}

// ============================================================================
// User Pre-Commit Hook Tests
// ============================================================================

#[rstest]
fn test_user_pre_commit_hook_executes(mut repo: TestRepo) {
    // Create feature worktree
    let feature_wt = repo.add_worktree("feature");

    // Add uncommitted changes (triggers pre-commit during merge)
    fs::write(feature_wt.join("uncommitted.txt"), "uncommitted content").unwrap();

    // Write user config with pre-commit hook
    repo.write_test_config(
        r#"[pre-commit]
lint = "echo 'USER_PRE_COMMIT_RAN' > user_precommit.txt"
"#,
    );

    snapshot_merge(
        "user_pre_commit_executes",
        &repo,
        &["main", "--yes", "--no-remove"],
        Some(&feature_wt),
    );

    // Verify user hook ran
    let marker_file = feature_wt.join("user_precommit.txt");
    assert!(marker_file.exists(), "User pre-commit hook should have run");
}

#[rstest]
fn test_user_pre_commit_failure_blocks_commit(mut repo: TestRepo) {
    // Create feature worktree
    let feature_wt = repo.add_worktree("feature");

    // Add uncommitted changes
    fs::write(feature_wt.join("uncommitted.txt"), "uncommitted content").unwrap();

    // Write user config with failing pre-commit hook
    repo.write_test_config(
        r#"[pre-commit]
lint = "exit 1"
"#,
    );

    // Failing pre-commit hook should block the merge
    snapshot_merge(
        "user_pre_commit_failure",
        &repo,
        &["main", "--yes", "--no-remove"],
        Some(&feature_wt),
    );
}

// ============================================================================
// Template Variable Tests
// ============================================================================

#[rstest]
fn test_user_hook_template_variables(repo: TestRepo) {
    // Write user config with hook using template variables
    repo.write_test_config(
        r#"[post-create]
vars = "echo 'repo={{ repo }} branch={{ branch }}' > template_vars.txt"
"#,
    );

    snapshot_switch("user_hook_template_vars", &repo, &["--create", "feature"]);

    // Verify template variables were expanded
    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    let vars_file = worktree_path.join("template_vars.txt");
    assert!(vars_file.exists());

    let contents = fs::read_to_string(&vars_file).unwrap();
    assert!(
        contents.contains("repo=repo"),
        "Should have expanded repo variable: {}",
        contents
    );
    assert!(
        contents.contains("branch=feature"),
        "Should have expanded branch variable: {}",
        contents
    );
}

// ============================================================================
// Combined User and Project Hooks Tests
// ============================================================================

#[rstest]
fn test_user_and_project_post_start_both_run(repo: TestRepo) {
    // Create project config with post-start hook
    repo.write_project_config(r#"post-start = "echo 'PROJECT_POST_START' > project_bg.txt""#);
    repo.commit("Add project config");

    // Write user config with user hook AND pre-approve project command
    repo.write_test_config(
        r#"[post-start]
bg = "echo 'USER_POST_START' > user_bg.txt"

[projects."../origin"]
approved-commands = ["echo 'PROJECT_POST_START' > project_bg.txt"]
"#,
    );

    snapshot_switch(
        "user_and_project_post_start",
        &repo,
        &["--create", "feature"],
    );

    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");

    // Wait for both background commands
    wait_for_file(&worktree_path.join("user_bg.txt"));
    wait_for_file(&worktree_path.join("project_bg.txt"));

    // Both should have run
    assert!(
        worktree_path.join("user_bg.txt").exists(),
        "User post-start should have run"
    );
    assert!(
        worktree_path.join("project_bg.txt").exists(),
        "Project post-start should have run"
    );
}

// ============================================================================
// Standalone Hook Execution Tests (wt hook <type>)
// ============================================================================

#[rstest]
fn test_standalone_hook_post_create(repo: TestRepo) {
    // Write project config with post-create hook
    repo.write_project_config(r#"post-create = "echo 'STANDALONE_POST_CREATE' > hook_ran.txt""#);

    let mut cmd = crate::common::wt_command();
    cmd.current_dir(repo.root_path());
    cmd.env("WORKTRUNK_CONFIG_PATH", repo.test_config_path());
    cmd.args(["hook", "post-create", "--yes"]);

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "wt hook post-create should succeed"
    );

    // Hook should have run
    let marker = repo.root_path().join("hook_ran.txt");
    assert!(marker.exists(), "post-create hook should have run");
    let content = fs::read_to_string(&marker).unwrap();
    assert!(content.contains("STANDALONE_POST_CREATE"));
}

#[rstest]
fn test_standalone_hook_post_start(repo: TestRepo) {
    // Write project config with post-start hook
    repo.write_project_config(r#"post-start = "echo 'STANDALONE_POST_START' > hook_ran.txt""#);

    let mut cmd = crate::common::wt_command();
    cmd.current_dir(repo.root_path());
    cmd.env("WORKTRUNK_CONFIG_PATH", repo.test_config_path());
    cmd.args(["hook", "post-start", "--yes"]);

    let output = cmd.output().unwrap();
    assert!(output.status.success(), "wt hook post-start should succeed");

    // Hook spawns in background - wait for marker file
    let marker = repo.root_path().join("hook_ran.txt");
    wait_for_file_content(&marker);
    let content = fs::read_to_string(&marker).unwrap();
    assert!(content.contains("STANDALONE_POST_START"));
}

#[rstest]
fn test_standalone_hook_post_start_foreground(repo: TestRepo) {
    // Write project config with post-start hook that echoes to both file and stdout
    repo.write_project_config(
        r#"post-start = "echo 'FOREGROUND_POST_START' && echo 'marker' > hook_ran.txt""#,
    );

    let mut cmd = crate::common::wt_command();
    cmd.current_dir(repo.root_path());
    cmd.env("WORKTRUNK_CONFIG_PATH", repo.test_config_path());
    cmd.args(["hook", "post-start", "--yes", "--foreground"]);

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "wt hook post-start --foreground should succeed"
    );

    // With --foreground, marker file should exist immediately (no waiting)
    let marker = repo.root_path().join("hook_ran.txt");
    assert!(
        marker.exists(),
        "hook should have completed synchronously with --foreground"
    );

    // Output should contain the hook's stdout (not just spawned message)
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("FOREGROUND_POST_START"),
        "hook stdout should appear in command output with --foreground, got: {stderr}"
    );
}

#[rstest]
fn test_standalone_hook_post_start_no_background_deprecated(repo: TestRepo) {
    // Test that --no-background still works but shows deprecation warning
    repo.write_project_config(
        r#"post-start = "echo 'DEPRECATED_FLAG' && echo 'marker' > hook_ran.txt""#,
    );

    let mut cmd = crate::common::wt_command();
    cmd.current_dir(repo.root_path());
    cmd.env("WORKTRUNK_CONFIG_PATH", repo.test_config_path());
    cmd.args(["hook", "post-start", "--yes", "--no-background"]);

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "wt hook post-start --no-background should succeed"
    );

    // With --no-background, marker file should exist immediately (no waiting)
    let marker = repo.root_path().join("hook_ran.txt");
    assert!(
        marker.exists(),
        "hook should have completed synchronously with --no-background"
    );

    // Output should contain deprecation warning
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--no-background is deprecated"),
        "should show deprecation warning, got: {stderr}"
    );

    // Hook output should still appear
    assert!(
        stderr.contains("DEPRECATED_FLAG"),
        "hook stdout should appear in command output, got: {stderr}"
    );
}

#[rstest]
fn test_standalone_hook_post_switch_no_background_deprecated(repo: TestRepo) {
    // Test that --no-background still works but shows deprecation warning for post-switch
    repo.write_project_config(
        r#"post-switch = "echo 'SWITCH_DEPRECATED' && echo 'marker' > hook_ran.txt""#,
    );

    let mut cmd = crate::common::wt_command();
    cmd.current_dir(repo.root_path());
    cmd.env("WORKTRUNK_CONFIG_PATH", repo.test_config_path());
    cmd.args(["hook", "post-switch", "--yes", "--no-background"]);

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "wt hook post-switch --no-background should succeed"
    );

    // Output should contain deprecation warning
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--no-background is deprecated"),
        "should show deprecation warning, got: {stderr}"
    );
}

#[rstest]
fn test_standalone_hook_pre_commit(repo: TestRepo) {
    // Write project config with pre-commit hook
    repo.write_project_config(r#"pre-commit = "echo 'STANDALONE_PRE_COMMIT' > hook_ran.txt""#);

    let mut cmd = crate::common::wt_command();
    cmd.current_dir(repo.root_path());
    cmd.env("WORKTRUNK_CONFIG_PATH", repo.test_config_path());
    cmd.args(["hook", "pre-commit", "--yes"]);

    let output = cmd.output().unwrap();
    assert!(output.status.success(), "wt hook pre-commit should succeed");

    // Hook should have run
    let marker = repo.root_path().join("hook_ran.txt");
    assert!(marker.exists(), "pre-commit hook should have run");
    let content = fs::read_to_string(&marker).unwrap();
    assert!(content.contains("STANDALONE_PRE_COMMIT"));
}

#[rstest]
fn test_standalone_hook_post_merge(repo: TestRepo) {
    // Write project config with post-merge hook
    repo.write_project_config(r#"post-merge = "echo 'STANDALONE_POST_MERGE' > hook_ran.txt""#);

    let mut cmd = crate::common::wt_command();
    cmd.current_dir(repo.root_path());
    cmd.env("WORKTRUNK_CONFIG_PATH", repo.test_config_path());
    cmd.args(["hook", "post-merge", "--yes"]);

    let output = cmd.output().unwrap();
    assert!(output.status.success(), "wt hook post-merge should succeed");

    // Hook should have run
    let marker = repo.root_path().join("hook_ran.txt");
    assert!(marker.exists(), "post-merge hook should have run");
    let content = fs::read_to_string(&marker).unwrap();
    assert!(content.contains("STANDALONE_POST_MERGE"));
}

#[rstest]
fn test_standalone_hook_pre_remove(repo: TestRepo) {
    // Write project config with pre-remove hook
    repo.write_project_config(r#"pre-remove = "echo 'STANDALONE_PRE_REMOVE' > hook_ran.txt""#);

    let mut cmd = crate::common::wt_command();
    cmd.current_dir(repo.root_path());
    cmd.env("WORKTRUNK_CONFIG_PATH", repo.test_config_path());
    cmd.args(["hook", "pre-remove", "--yes"]);

    let output = cmd.output().unwrap();
    assert!(output.status.success(), "wt hook pre-remove should succeed");

    // Hook should have run
    let marker = repo.root_path().join("hook_ran.txt");
    assert!(marker.exists(), "pre-remove hook should have run");
    let content = fs::read_to_string(&marker).unwrap();
    assert!(content.contains("STANDALONE_PRE_REMOVE"));
}

#[rstest]
fn test_standalone_hook_no_hooks_configured(repo: TestRepo) {
    // No project config, no user config with hooks
    let mut cmd = crate::common::wt_command();
    cmd.current_dir(repo.root_path());
    cmd.env("WORKTRUNK_CONFIG_PATH", repo.test_config_path());
    cmd.args(["hook", "post-create", "--yes"]);

    let output = cmd.output().unwrap();
    assert!(
        !output.status.success(),
        "wt hook should fail when no hooks configured"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("No post-create hook configured"),
        "Error should mention no hook configured, got: {stderr}"
    );
}

// ============================================================================
// Background Hook Execution Tests (post-start, post-switch)
// ============================================================================

#[rstest]
fn test_concurrent_hook_single_failure(repo: TestRepo) {
    // Write project config with a hook that writes output before failing
    repo.write_project_config(r#"post-start = "echo HOOK_OUTPUT_MARKER; exit 1""#);

    let mut cmd = crate::common::wt_command();
    cmd.current_dir(repo.root_path());
    cmd.env("WORKTRUNK_CONFIG_PATH", repo.test_config_path());
    cmd.args(["hook", "post-start", "--yes"]);

    let output = cmd.output().unwrap();
    // Background spawning always succeeds (spawn succeeded, failure is logged)
    assert!(
        output.status.success(),
        "wt hook post-start should succeed (spawns in background)"
    );

    // Wait for log file to be created and contain output
    let log_dir = resolve_git_common_dir(repo.root_path()).join("wt-logs");
    wait_for_file_count(&log_dir, "log", 1);

    // Find and read the log file
    let log_file = fs::read_dir(&log_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| e.path().extension().is_some_and(|ext| ext == "log"))
        .expect("Should have a log file");

    // Wait for content to be written (command runs async)
    wait_for_file_content(&log_file.path());
    let log_content = fs::read_to_string(log_file.path()).unwrap();

    // Verify the hook actually ran and wrote output (not just that file was created)
    assert!(
        log_content.contains("HOOK_OUTPUT_MARKER"),
        "Log should contain hook output, got: {log_content}"
    );
}

#[rstest]
fn test_concurrent_hook_multiple_failures(repo: TestRepo) {
    // Write project config with multiple named hooks (table format)
    repo.write_project_config(
        r#"[post-start]
first = "echo FIRST_OUTPUT"
second = "echo SECOND_OUTPUT"
"#,
    );

    let mut cmd = crate::common::wt_command();
    cmd.current_dir(repo.root_path());
    cmd.env("WORKTRUNK_CONFIG_PATH", repo.test_config_path());
    cmd.args(["hook", "post-start", "--yes"]);

    let output = cmd.output().unwrap();
    // Background spawning always succeeds (spawn succeeded)
    assert!(
        output.status.success(),
        "wt hook post-start should succeed (spawns in background)"
    );

    // Wait for both log files to be created
    let log_dir = resolve_git_common_dir(repo.root_path()).join("wt-logs");
    wait_for_file_count(&log_dir, "log", 2);

    // Collect log files and their contents
    let log_files: Vec<_> = fs::read_dir(&log_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "log"))
        .collect();
    assert_eq!(log_files.len(), 2, "Should have 2 log files");

    // Wait for content in both log files
    for log_file in &log_files {
        wait_for_file_content(&log_file.path());
    }

    // Collect all log contents
    let mut found_first = false;
    let mut found_second = false;
    for log_file in &log_files {
        let name = log_file.file_name().to_string_lossy().to_string();
        let content = fs::read_to_string(log_file.path()).unwrap();
        if name.contains("first") {
            assert!(
                content.contains("FIRST_OUTPUT"),
                "first log should contain FIRST_OUTPUT, got: {content}"
            );
            found_first = true;
        }
        if name.contains("second") {
            assert!(
                content.contains("SECOND_OUTPUT"),
                "second log should contain SECOND_OUTPUT, got: {content}"
            );
            found_second = true;
        }
    }
    assert!(found_first, "Should have log for 'first' hook");
    assert!(found_second, "Should have log for 'second' hook");
}

#[rstest]
fn test_concurrent_hook_user_and_project(repo: TestRepo) {
    // Write user config with post-start hook (using table format for named hook)
    repo.write_test_config(
        r#"[post-start]
user = "echo 'USER_HOOK' > user_hook_ran.txt"
"#,
    );

    // Write project config with post-start hook
    repo.write_project_config(r#"post-start = "echo 'PROJECT_HOOK' > project_hook_ran.txt""#);

    let mut cmd = crate::common::wt_command();
    cmd.current_dir(repo.root_path());
    cmd.env("WORKTRUNK_CONFIG_PATH", repo.test_config_path());
    cmd.args(["hook", "post-start", "--yes"]);

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "wt hook post-start should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Both hooks spawn in background - wait for marker files
    let user_marker = repo.root_path().join("user_hook_ran.txt");
    let project_marker = repo.root_path().join("project_hook_ran.txt");

    wait_for_file_content(&user_marker);
    wait_for_file_content(&project_marker);

    let user_content = fs::read_to_string(&user_marker).unwrap();
    let project_content = fs::read_to_string(&project_marker).unwrap();
    assert!(user_content.contains("USER_HOOK"));
    assert!(project_content.contains("PROJECT_HOOK"));
}

#[rstest]
fn test_concurrent_hook_post_switch(repo: TestRepo) {
    // Write project config with post-switch hook
    repo.write_project_config(r#"post-switch = "echo 'POST_SWITCH' > hook_ran.txt""#);

    let mut cmd = crate::common::wt_command();
    cmd.current_dir(repo.root_path());
    cmd.env("WORKTRUNK_CONFIG_PATH", repo.test_config_path());
    cmd.args(["hook", "post-switch", "--yes"]);

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "wt hook post-switch should succeed"
    );

    // Hook spawns in background - wait for marker file
    let marker = repo.root_path().join("hook_ran.txt");
    wait_for_file_content(&marker);
    let content = fs::read_to_string(&marker).unwrap();
    assert!(content.contains("POST_SWITCH"));
}

#[rstest]
fn test_concurrent_hook_with_name_filter(repo: TestRepo) {
    // Write project config with multiple named hooks
    repo.write_project_config(
        r#"[post-start]
first = "echo 'FIRST' > first.txt"
second = "echo 'SECOND' > second.txt"
"#,
    );

    // Run only the "first" hook by name
    let mut cmd = crate::common::wt_command();
    cmd.current_dir(repo.root_path());
    cmd.env("WORKTRUNK_CONFIG_PATH", repo.test_config_path());
    cmd.args(["hook", "post-start", "--yes", "first"]);

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "wt hook post-start --name first should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // First hook spawns in background - wait for marker file
    let first_marker = repo.root_path().join("first.txt");
    let second_marker = repo.root_path().join("second.txt");

    wait_for_file_content(&first_marker);

    // Fixed sleep for absence check - second hook should NOT have run
    thread::sleep(SLEEP_FOR_ABSENCE_CHECK);
    assert!(!second_marker.exists(), "second hook should NOT have run");
}

#[rstest]
fn test_concurrent_hook_invalid_name_filter(repo: TestRepo) {
    // Write project config with named hooks
    repo.write_project_config(
        r#"[post-start]
first = "echo 'FIRST'"
"#,
    );

    // Try to run a non-existent hook by name
    let mut cmd = crate::common::wt_command();
    cmd.current_dir(repo.root_path());
    cmd.env("WORKTRUNK_CONFIG_PATH", repo.test_config_path());
    cmd.args(["hook", "post-start", "--yes", "nonexistent"]);

    let output = cmd.output().unwrap();
    assert!(
        !output.status.success(),
        "wt hook post-start --name nonexistent should fail"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("nonexistent") && stderr.contains("No command named"),
        "Error should mention command not found, got: {stderr}"
    );
    // Should list available commands
    assert!(
        stderr.contains("project:first"),
        "Error should list available commands, got: {stderr}"
    );
}

// ============================================================================
// Custom Variable (--var) Tests
// ============================================================================

#[rstest]
fn test_var_flag_overrides_template_variable(repo: TestRepo) {
    // Write user config with a hook that uses a template variable
    repo.write_test_config(
        r#"[post-create]
test = "echo '{{ target }}' > target_output.txt"
"#,
    );

    let output = repo
        .wt_command()
        .args([
            "hook",
            "post-create",
            "--yes",
            "--var",
            "target=CUSTOM_TARGET",
        ])
        .output()
        .expect("Failed to run wt hook");

    assert!(output.status.success(), "Hook should succeed");

    let output_file = repo.root_path().join("target_output.txt");
    let contents = fs::read_to_string(&output_file).unwrap();
    assert!(
        contents.contains("CUSTOM_TARGET"),
        "Variable should be overridden in hook, got: {contents}"
    );
}

#[rstest]
fn test_var_flag_multiple_variables(repo: TestRepo) {
    // Write user config with a hook that uses multiple template variables
    repo.write_test_config(
        r#"[post-create]
test = "echo '{{ target }} {{ remote }}' > multi_var_output.txt"
"#,
    );

    let output = repo
        .wt_command()
        .args([
            "hook",
            "post-create",
            "--yes",
            "--var",
            "target=FIRST",
            "--var",
            "remote=SECOND",
        ])
        .output()
        .expect("Failed to run wt hook");

    assert!(output.status.success(), "Hook should succeed");

    let output_file = repo.root_path().join("multi_var_output.txt");
    let contents = fs::read_to_string(&output_file).unwrap();
    assert!(
        contents.contains("FIRST") && contents.contains("SECOND"),
        "Both variables should be overridden, got: {contents}"
    );
}

#[rstest]
fn test_var_flag_overrides_builtin_variable(repo: TestRepo) {
    // Write user config with a hook that uses the builtin branch variable
    repo.write_test_config(
        r#"[post-create]
test = "echo '{{ branch }}' > branch_output.txt"
"#,
    );

    let output = repo
        .wt_command()
        .args([
            "hook",
            "post-create",
            "--yes",
            "--var",
            "branch=CUSTOM_BRANCH_NAME",
        ])
        .output()
        .expect("Failed to run wt hook");

    assert!(output.status.success(), "Hook should succeed");

    let output_file = repo.root_path().join("branch_output.txt");
    let contents = fs::read_to_string(&output_file).unwrap();
    assert!(
        contents.contains("CUSTOM_BRANCH_NAME"),
        "Custom variable should override builtin, got: {contents}"
    );
}

#[rstest]
fn test_var_flag_invalid_format_fails() {
    // Test that invalid KEY=VALUE format is rejected
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_wt"))
        .args(["hook", "post-create", "--var", "no_equals_sign"])
        .output()
        .expect("Failed to run wt");

    assert!(!output.status.success(), "Invalid --var format should fail");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid KEY=VALUE") || stderr.contains("no `=` found"),
        "Error should mention invalid format, got: {stderr}"
    );
}

#[test]
fn test_var_flag_unknown_variable_fails() {
    // Test that unknown variable names are rejected
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_wt"))
        .args(["hook", "post-create", "--var", "custom_var=value"])
        .output()
        .expect("Failed to run wt");

    assert!(!output.status.success(), "Unknown variable should fail");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown variable"),
        "Error should mention unknown variable, got: {stderr}"
    );
}

#[rstest]
fn test_var_flag_last_value_wins(repo: TestRepo) {
    // Test that when the same variable is specified multiple times, the last value wins
    repo.write_test_config(
        r#"[post-create]
test = "echo '{{ target }}' > target_output.txt"
"#,
    );

    let output = repo
        .wt_command()
        .args([
            "hook",
            "post-create",
            "--yes",
            "--var",
            "target=FIRST",
            "--var",
            "target=SECOND",
        ])
        .output()
        .expect("Failed to run wt hook");

    assert!(output.status.success());

    let output_file = repo.root_path().join("target_output.txt");
    let contents = std::fs::read_to_string(&output_file).expect("Should have created output file");
    assert!(
        contents.contains("SECOND"),
        "Last --var value should win, got: {contents}"
    );
}

#[rstest]
fn test_var_flag_deprecated_alias_works(repo: TestRepo) {
    // Test that deprecated variable aliases (main_worktree, repo_root, worktree) can be overridden
    repo.write_test_config(
        r#"[post-create]
test = "echo '{{ main_worktree }}' > alias_output.txt"
"#,
    );

    let output = repo
        .wt_command()
        .args([
            "hook",
            "post-create",
            "--yes",
            "--var",
            "main_worktree=/custom/path",
        ])
        .output()
        .expect("Failed to run wt hook");

    assert!(output.status.success());

    let output_file = repo.root_path().join("alias_output.txt");
    let contents = std::fs::read_to_string(&output_file).expect("Should have created output file");
    assert!(
        contents.contains("/custom/path"),
        "Deprecated alias should be overridden, got: {contents}"
    );
}
