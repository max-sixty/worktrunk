//! Integration tests for user-level hooks (~/.config/worktrunk/config.toml)
//!
//! User hooks differ from project hooks:
//! - Run for all repositories
//! - Execute before project hooks
//! - Don't require approval
//! - Skipped together with project hooks via --no-hooks

use crate::common::{
    TestRepo, make_snapshot_cmd, make_snapshot_cmd_with_global_flags, repo, resolve_git_common_dir,
    setup_snapshot_settings, wait_for_file, wait_for_file_content, wait_for_file_count,
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
    // Write user config with pre-start hook (no project config)
    repo.write_test_config(
        r#"[pre-start]
log = "echo 'USER_POST_CREATE_RAN' > user_hook_marker.txt"
"#,
    );

    snapshot_switch("user_post_create_executes", &repo, &["--create", "feature"]);

    // Verify user hook actually ran
    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    let marker_file = worktree_path.join("user_hook_marker.txt");
    assert!(
        marker_file.exists(),
        "User pre-start hook should have created marker file"
    );

    let contents = fs::read_to_string(&marker_file).unwrap();
    assert!(
        contents.contains("USER_POST_CREATE_RAN"),
        "Marker file should contain expected content"
    );
}

#[rstest]
fn test_user_hooks_run_before_project_hooks(repo: TestRepo) {
    // Create project config with pre-start hook
    repo.write_project_config(r#"pre-start = "echo 'PROJECT_HOOK' >> hook_order.txt""#);
    repo.commit("Add project config");

    // Write user config with user hook AND pre-approve project command
    repo.write_test_config(
        r#"[pre-start]
log = "echo 'USER_HOOK' >> hook_order.txt"
"#,
    );
    repo.write_test_approvals(
        r#"[projects."../origin"]
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
        r#"[pre-start]
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
fn test_no_hooks_flag_skips_all_hooks(repo: TestRepo) {
    // Create project config with pre-start hook
    repo.write_project_config(r#"pre-start = "echo 'PROJECT_HOOK' > project_marker.txt""#);
    repo.commit("Add project config");

    // Write user config with both user hook and pre-approved project command
    repo.write_test_config(
        r#"[pre-start]
log = "echo 'USER_HOOK' > user_marker.txt"
"#,
    );
    repo.write_test_approvals(
        r#"[projects."../origin"]
approved-commands = ["echo 'PROJECT_HOOK' > project_marker.txt"]
"#,
    );

    // Create worktree with --no-hooks (skips ALL hooks)
    snapshot_switch(
        "no_hooks_skips_all_hooks",
        &repo,
        &["--create", "feature", "--no-hooks"],
    );

    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");

    // User hook should NOT have run
    let user_marker = worktree_path.join("user_marker.txt");
    assert!(
        !user_marker.exists(),
        "User hook should be skipped with --no-hooks"
    );

    // Project hook should also NOT have run (--no-hooks skips ALL hooks)
    let project_marker = worktree_path.join("project_marker.txt");
    assert!(
        !project_marker.exists(),
        "Project hook should also be skipped with --no-hooks"
    );
}

#[rstest]
fn test_user_post_create_hook_failure(repo: TestRepo) {
    // Write user config with failing hook
    repo.write_test_config(
        r#"[pre-start]
failing = "exit 1"
"#,
    );

    // Failing pre-start hook (via deprecated pre-start name) aborts with FailFast.
    // The worktree is already created before pre-start runs (it was renamed from
    // pre-start), so the worktree exists but the command exits non-zero.
    snapshot_switch("user_post_create_failure", &repo, &["--create", "feature"]);

    // Worktree exists (created before pre-start ran) but the command failed
    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    assert!(
        worktree_path.exists(),
        "Worktree should exist — it was created before pre-start ran"
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
fn test_background_hook_sees_worktrunk_foreground_env_var(repo: TestRepo) {
    // Background hooks run via the detached `wt hook run-pipeline` runner; wt
    // sets WORKTRUNK_FOREGROUND=-1 on that process so descendants (shell,
    // user command, nested `wt` invocations) can tell they're backgrounded.
    // `pre-*` hooks run synchronously in the foreground `wt` process, so they
    // must NOT see the sentinel.
    repo.write_test_config(
        r#"[post-start]
capture-bg = "echo \"bg=$WORKTRUNK_FOREGROUND\" > post_start_env.txt"

[pre-start]
capture-fg = "echo \"fg=$WORKTRUNK_FOREGROUND\" > pre_start_env.txt"
"#,
    );

    snapshot_switch(
        "background_hook_env_var_visible",
        &repo,
        &["--create", "feature"],
    );

    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");

    // pre-start wrote its marker synchronously; post-start runs in the
    // background, so poll for the file.
    let bg_marker = worktree_path.join("post_start_env.txt");
    wait_for_file_content(&bg_marker);
    let bg = fs::read_to_string(&bg_marker).unwrap();
    assert_eq!(
        bg.trim(),
        "bg=-1",
        "post-start (background) hook should see WORKTRUNK_FOREGROUND=-1, got {bg:?}"
    );

    let fg_marker = worktree_path.join("pre_start_env.txt");
    let fg = fs::read_to_string(&fg_marker).unwrap();
    assert_eq!(
        fg.trim(),
        "fg=",
        "pre-start (foreground) hook should NOT see WORKTRUNK_FOREGROUND set, got {fg:?}"
    );
}

#[rstest]
fn test_user_post_start_skipped_with_no_hooks(repo: TestRepo) {
    // Write user config with post-start hook
    repo.write_test_config(
        r#"[post-start]
bg = "echo 'USER_BG' > user_bg_marker.txt"
"#,
    );

    snapshot_switch(
        "user_post_start_skipped_no_hooks",
        &repo,
        &["--create", "feature", "--no-hooks"],
    );

    // Wait to ensure background hook would have had time to run
    thread::sleep(SLEEP_FOR_ABSENCE_CHECK);

    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    let marker_file = worktree_path.join("user_bg_marker.txt");
    assert!(
        !marker_file.exists(),
        "User post-start hook should be skipped with --no-hooks"
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
fn test_user_pre_merge_skipped_with_no_hooks(mut repo: TestRepo) {
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
        "user_pre_merge_skipped_no_hooks",
        &repo,
        &["main", "--yes", "--no-remove", "--no-hooks"],
        Some(&feature_wt),
    );

    // User hook should NOT have run (--no-hooks skips all hooks)
    let marker_file = feature_wt.join("user_premerge_marker.txt");
    assert!(
        !marker_file.exists(),
        "User pre-merge hook should be skipped with --no-hooks"
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

/// A signal-derived exit in one hook step must abort the rest of the pipeline
/// rather than treating the signal like an ordinary per-step failure. Drives
/// the `handle_command_error` interrupt branch end-to-end through the hook
/// path (the for-each test in `for_each.rs` covers the worktree-loop branch).
///
/// Implementation mirrors `test_for_each_aborts_on_signal_exit`: the first
/// hook step self-signals via SIGTERM after touching a marker. SIGINT against
/// the parent wt would kill the test harness, so we drive the same
/// `ChildProcessExited { signal: Some(_), .. }` path with an in-child signal.
#[rstest]
#[cfg(unix)]
fn test_pre_merge_pipeline_aborts_on_signal_exit(repo: TestRepo) {
    repo.commit("Initial commit");

    // Two pre-merge hooks: the first writes a marker then self-signals with
    // SIGTERM; the second (which must NOT run) would write its own marker.
    repo.write_project_config(
        r#"[pre-merge]
abort = "sh -c 'echo first >> hook.log; kill -TERM $$'"
after = "sh -c 'echo second >> hook.log'"
"#,
    );
    repo.commit("Add pre-merge hooks");

    let output = crate::common::wt_command()
        .current_dir(repo.root_path())
        .args(["hook", "pre-merge", "--yes"])
        .output()
        .expect("run wt hook pre-merge");

    // 128 + SIGTERM (15) = 143
    assert_eq!(
        output.status.code(),
        Some(143),
        "expected exit 143 (SIGTERM); got {:?}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr),
    );

    let hook_log = repo.root_path().join("hook.log");
    let contents = std::fs::read_to_string(&hook_log).unwrap_or_default();
    assert_eq!(
        contents.trim(),
        "first",
        "second hook step ran after the first was killed by signal; got: {contents:?}",
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

    // Post-merge runs in the destination (main) worktree (poll for pipeline runner)
    let main_worktree = repo.root_path();
    let marker_file = main_worktree.join("user_postmerge.txt");
    wait_for_file(&marker_file);
}

#[rstest]
fn test_combined_user_and_project_post_merge(mut repo: TestRepo) {
    repo.write_project_config(
        r#"[post-merge]
install = "echo 'PROJECT_RAN' > project_postmerge.txt"
"#,
    );
    repo.commit("Add project config");

    let feature_wt =
        repo.add_worktree_with_commit("feature", "feature.txt", "feature content", "Add feature");

    repo.write_test_config(
        r#"[post-merge]
sync = "echo 'USER_RAN' > user_postmerge.txt"
"#,
    );

    snapshot_merge(
        "combined_user_and_project_post_merge",
        &repo,
        &["main", "--yes", "--no-remove"],
        Some(&feature_wt),
    );

    let main_worktree = repo.root_path();
    wait_for_file(&main_worktree.join("user_postmerge.txt"));
    wait_for_file(&main_worktree.join("project_postmerge.txt"));
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
fn test_user_pre_remove_skipped_with_no_hooks(mut repo: TestRepo) {
    // Create a worktree to remove
    let feature_wt = repo.add_worktree("feature");

    // Write user config with pre-remove hook that would block
    repo.write_test_config(
        r#"[pre-remove]
block = "exit 1"
"#,
    );

    // With --no-hooks, all hooks (including the failing one) should be skipped
    snapshot_remove(
        "user_pre_remove_skipped_no_hooks",
        &repo,
        &["feature", "--force-delete", "--no-hooks"],
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
        "Worktree should be removed when --no-hooks skips failing hook"
    );
}

// ============================================================================
// User Post-Remove Hook Tests
// ============================================================================

#[rstest]
fn test_user_post_remove_hook_executes(mut repo: TestRepo) {
    // Create a worktree to remove
    let _feature_wt = repo.add_worktree("feature");

    // Write user config with post-remove hook
    // Hook writes to parent dir (temp dir) since the worktree itself is removed
    repo.write_test_config(
        r#"[post-remove]
cleanup = "echo 'USER_POST_REMOVE_RAN' > ../user_postremove_marker.txt"
"#,
    );

    snapshot_remove(
        "user_post_remove_executes",
        &repo,
        &["feature", "--force-delete"],
        Some(repo.root_path()),
    );

    // Wait for background hook to complete
    let marker_file = repo
        .root_path()
        .parent()
        .unwrap()
        .join("user_postremove_marker.txt");
    crate::common::wait_for_file(&marker_file);
    assert!(
        marker_file.exists(),
        "User post-remove hook should have run"
    );
}

/// Post-remove hooks run at the primary worktree, not cwd. When removing a
/// non-current worktree from a linked worktree, the output should show `@ [path]`
/// pointing to the primary worktree where hooks execute.
#[rstest]
fn test_post_remove_hooks_run_at_primary_worktree(mut repo: TestRepo) {
    let _feature_wt = repo.add_worktree("feature");
    let other_wt = repo.add_worktree("other");

    repo.write_test_config(
        r#"[post-remove]
cleanup = "echo done"
"#,
    );

    // Remove feature from the "other" worktree (not primary)
    snapshot_remove(
        "post_remove_runs_at_primary",
        &repo,
        &["feature", "--force-delete"],
        Some(&other_wt),
    );
}

/// Verify that post-remove hook template variables reference the removed worktree,
/// not the worktree where the hook executes from.
#[rstest]
fn test_user_post_remove_template_vars_reference_removed_worktree(mut repo: TestRepo) {
    // Create a worktree with a unique commit to verify commit capture
    let feature_wt_path =
        repo.add_worktree_with_commit("feature", "feature.txt", "feature content", "Add feature");

    // Get the commit SHA of the feature worktree BEFORE removal
    let feature_commit = repo
        .git_command()
        .args(["rev-parse", "HEAD"])
        .current_dir(&feature_wt_path)
        .run()
        .unwrap();
    let feature_commit = String::from_utf8_lossy(&feature_commit.stdout);
    let feature_commit = feature_commit.trim();
    let feature_short_commit = &feature_commit[..7];

    // Write user config that captures template variables to a file
    // Hook writes to parent dir (temp dir) since the worktree itself is removed
    repo.write_test_config(
        r#"[post-remove]
capture = "echo 'branch={{ branch }} worktree_path={{ worktree_path }} worktree_name={{ worktree_name }} commit={{ commit }} short_commit={{ short_commit }}' > ../postremove_vars.txt"
"#,
    );

    // Run from main worktree, remove the feature worktree
    repo.wt_command()
        .args(["remove", "feature", "--force-delete", "--yes"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    // Wait for background hook to complete
    let vars_file = repo
        .root_path()
        .parent()
        .unwrap()
        .join("postremove_vars.txt");
    crate::common::wait_for_file_content(&vars_file);

    let content = std::fs::read_to_string(&vars_file).unwrap();

    // Verify branch is the removed branch
    assert!(
        content.contains("branch=feature"),
        "branch should be the removed branch 'feature', got: {content}"
    );

    // Extract worktree name for cross-platform comparison.
    // Hooks run in Git Bash on Windows, which converts paths to MSYS2 format
    // (/c/Users/... instead of C:\Users\... or C:/Users/...). Instead of trying
    // to match exact path formats, verify the path ends with the worktree name.
    let feature_wt_name = feature_wt_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();

    // Verify worktree_path is the removed worktree's path (not the main worktree)
    // The worktree_path in hook output should end with the worktree directory name
    assert!(
        content.contains(&format!("/{feature_wt_name} "))
            || content.contains(&format!(r"\{feature_wt_name} ")),
        "worktree_path should end with the removed worktree's name '{feature_wt_name}', got: {content}"
    );

    // Verify worktree_name is the removed worktree's directory name
    assert!(
        content.contains(&format!("worktree_name={feature_wt_name}")),
        "worktree_name should be the removed worktree's name '{feature_wt_name}', got: {content}"
    );

    // Verify commit is the removed worktree's commit (not main worktree's commit)
    assert!(
        content.contains(&format!("commit={feature_commit}")),
        "commit should be the removed worktree's commit '{feature_commit}', got: {content}"
    );

    // Verify short_commit is the first 7 chars of the removed worktree's commit
    assert!(
        content.contains(&format!("short_commit={feature_short_commit}")),
        "short_commit should be '{feature_short_commit}', got: {content}"
    );
}

#[rstest]
fn test_user_post_remove_skipped_with_no_hooks(mut repo: TestRepo) {
    // Create a worktree to remove
    let feature_wt = repo.add_worktree("feature");

    // Write user config with post-remove hook that creates a marker
    repo.write_test_config(
        r#"[post-remove]
marker = "echo 'SHOULD_NOT_RUN' > ../no_hooks_postremove.txt"
"#,
    );

    snapshot_remove(
        "user_post_remove_no_hooks",
        &repo,
        &["feature", "--force-delete", "--no-hooks"],
        Some(repo.root_path()),
    );

    // Worktree should be removed
    let timeout = Duration::from_secs(5);
    let poll_interval = Duration::from_millis(50);
    let start = std::time::Instant::now();
    while feature_wt.exists() && start.elapsed() < timeout {
        thread::sleep(poll_interval);
    }
    assert!(
        !feature_wt.exists(),
        "Worktree should be removed with --no-hooks"
    );

    // Post-remove hook should NOT have run
    let marker_file = repo
        .root_path()
        .parent()
        .unwrap()
        .join("no_hooks_postremove.txt");
    thread::sleep(Duration::from_millis(500)); // Wait to ensure hook would have run if enabled
    assert!(
        !marker_file.exists(),
        "Post-remove hook should be skipped when --no-hooks is used"
    );
}

/// Verify that post-remove hooks run during `wt merge` (which removes the worktree).
/// This tests the main production use case for post-remove hooks.
#[rstest]
fn test_user_post_remove_hook_runs_during_merge(mut repo: TestRepo) {
    // Create feature worktree with a commit
    let feature_wt =
        repo.add_worktree_with_commit("feature", "feature.txt", "feature content", "Add feature");

    // Write user config with post-remove hook
    // Hook writes to temp dir (parent of repo) since worktree is removed
    repo.write_test_config(
        r#"[post-remove]
cleanup = "echo 'POST_REMOVE_DURING_MERGE' > ../merge_postremove_marker.txt"
"#,
    );

    // Run merge from feature worktree - this should trigger post-remove hooks
    repo.wt_command()
        .args(["merge", "main", "--yes"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();

    // Wait for background hook to complete
    let marker_file = repo
        .root_path()
        .parent()
        .unwrap()
        .join("merge_postremove_marker.txt");
    crate::common::wait_for_file_content(&marker_file);

    let contents = fs::read_to_string(&marker_file).unwrap();
    assert!(
        contents.contains("POST_REMOVE_DURING_MERGE"),
        "Post-remove hook should run during wt merge with expected content"
    );
}

/// When removing the current worktree (cd back to main), both post-remove and
/// post-switch hooks fire. They should appear on a single combined announcement line.
#[rstest]
fn test_combined_post_remove_and_post_switch_hooks(mut repo: TestRepo) {
    let feature_wt = repo.add_worktree("feature");

    // Configure both post-remove and post-switch user hooks
    repo.write_test_config(
        r#"[post-remove]
cleanup = "echo removed"

[post-switch]
notify = "echo switched"
"#,
    );

    // Remove from inside the feature worktree — triggers cd back to main,
    // which means changed_directory=true and both hook types fire.
    snapshot_remove(
        "combined_post_remove_and_post_switch",
        &repo,
        &["feature", "--force-delete"],
        Some(&feature_wt),
    );
}

// Note: The `return Ok(())` path in spawn_hooks_after_remove when UserConfig::load()
// fails is defensive code for an extremely rare race condition where config becomes
// invalid between command startup and hook execution. This is not easily testable
// without complex timing manipulation.

#[rstest]
fn test_standalone_hook_post_remove_invalid_template(repo: TestRepo) {
    // Write project config with invalid template syntax (unclosed braces)
    repo.write_project_config(r#"post-remove = "echo {{ invalid""#);

    let mut cmd = crate::common::wt_command();
    cmd.current_dir(repo.root_path());
    cmd.env("WORKTRUNK_CONFIG_PATH", repo.test_config_path());
    cmd.args(["hook", "post-remove", "--yes"]);

    let output = cmd.output().unwrap();
    assert!(
        !output.status.success(),
        "wt hook post-remove should fail with invalid template"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("syntax error"),
        "Error should mention template expansion failure, got: {stderr}"
    );
}

#[rstest]
fn test_standalone_hook_post_remove_name_filter_no_match(repo: TestRepo) {
    // Write project config with a named hook
    repo.write_project_config(
        r#"[post-remove]
cleanup = "echo cleanup"
"#,
    );

    let mut cmd = crate::common::wt_command();
    cmd.current_dir(repo.root_path());
    cmd.env("WORKTRUNK_CONFIG_PATH", repo.test_config_path());
    // Use a name filter that doesn't match any configured hook
    cmd.args(["hook", "post-remove", "nonexistent", "--yes"]);

    let output = cmd.output().unwrap();
    assert!(
        !output.status.success(),
        "wt hook post-remove should fail when name filter doesn't match"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("No hook named") || stderr.contains("nonexistent"),
        "Error should mention the unmatched filter, got: {stderr}"
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
// User Post-Commit Hook Tests (Background, via `wt step commit`)
// ============================================================================

/// Helper for step commit snapshots
fn snapshot_step_commit(
    test_name: &str,
    repo: &TestRepo,
    args: &[&str],
    cwd: Option<&std::path::Path>,
) {
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(repo, "step", &[], cwd);
        cmd.arg("commit");
        cmd.args(args);
        cmd.env(
            "WORKTRUNK_COMMIT__GENERATION__COMMAND",
            "cat >/dev/null && echo 'feat: test commit'",
        );
        assert_cmd_snapshot!(test_name, cmd);
    });
}

#[rstest]
fn test_user_post_commit_hook_executes(mut repo: TestRepo) {
    // Create feature worktree with staged changes
    let feature_wt = repo.add_worktree("feature");
    fs::write(feature_wt.join("new_file.txt"), "content").unwrap();

    // Write user config with post-commit hook
    repo.write_test_config(
        r#"[post-commit]
notify = "echo 'USER_POST_COMMIT_RAN' > user_postcommit.txt"
"#,
    );

    snapshot_step_commit("user_post_commit_executes", &repo, &[], Some(&feature_wt));

    // Post-commit runs in background in the worktree where the commit happened
    let marker_file = feature_wt.join("user_postcommit.txt");
    wait_for_file_content(&marker_file);

    let contents = fs::read_to_string(&marker_file).unwrap();
    assert!(
        contents.contains("USER_POST_COMMIT_RAN"),
        "User post-commit hook should have run, got: {contents}"
    );
}

#[rstest]
fn test_user_post_commit_skipped_with_no_hooks(mut repo: TestRepo) {
    // Create feature worktree with staged changes
    let feature_wt = repo.add_worktree("feature");
    fs::write(feature_wt.join("new_file.txt"), "content").unwrap();

    // Write user config with post-commit hook
    repo.write_test_config(
        r#"[post-commit]
notify = "echo 'USER_POST_COMMIT_RAN' > user_postcommit.txt"
"#,
    );

    snapshot_step_commit(
        "user_post_commit_skipped_no_hooks",
        &repo,
        &["--no-hooks"],
        Some(&feature_wt),
    );

    // Wait to ensure background hook would have had time to run
    thread::sleep(SLEEP_FOR_ABSENCE_CHECK);

    let marker_file = feature_wt.join("user_postcommit.txt");
    assert!(
        !marker_file.exists(),
        "User post-commit hook should be skipped with --no-hooks"
    );
}

#[rstest]
fn test_user_post_commit_failure_does_not_block_commit(mut repo: TestRepo) {
    // Create feature worktree with staged changes
    let feature_wt = repo.add_worktree("feature");
    fs::write(feature_wt.join("new_file.txt"), "content").unwrap();

    // Write user config with failing post-commit hook
    repo.write_test_config(
        r#"[post-commit]
failing = "exit 1"
"#,
    );

    snapshot_step_commit("user_post_commit_failure", &repo, &[], Some(&feature_wt));

    // The commit should have succeeded despite post-commit hook failure
    // (post-commit runs in background and doesn't affect exit code)
    let output = repo
        .git_command()
        .current_dir(&feature_wt)
        .args(["log", "--oneline", "-1"])
        .run()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("feat: test commit"),
        "Commit should have succeeded despite post-commit hook failure, got: {stdout}"
    );
}

// ============================================================================
// Template Variable Tests
// ============================================================================

#[rstest]
fn test_user_hook_template_variables(repo: TestRepo) {
    // Write user config with hook using template variables
    repo.write_test_config(
        r#"[pre-start]
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

#[rstest]
fn test_hook_template_variables_from_subdirectory(repo: TestRepo) {
    // Hook that writes template variables and pwd to files so we can verify their values.
    // This tests that running from a subdirectory still resolves worktree_path to the
    // worktree root (not "." or the subdirectory) and sets hook CWD to the root.
    repo.write_project_config(
        r#"pre-merge = "echo '{{ worktree_path }}' > wt_path.txt && echo '{{ worktree_name }}' > wt_name.txt && pwd > hook_cwd.txt""#,
    );
    repo.commit("Add pre-merge hook");

    // Create a subdirectory and run the hook from there
    let subdir = repo.root_path().join("src").join("components");
    fs::create_dir_all(&subdir).unwrap();

    let output = repo
        .wt_command()
        .args(["hook", "pre-merge", "--yes"])
        .current_dir(&subdir) // override: run from subdirectory
        .output()
        .expect("Failed to run wt hook pre-merge");

    assert!(
        output.status.success(),
        "wt hook pre-merge failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // worktree_path should be the worktree root, not "." or the subdirectory.
    // On Windows, to_posix_path() converts C:\... to /c/..., so check that the path
    // is not relative rather than using is_absolute() (which rejects POSIX-style paths).
    let wt_path = fs::read_to_string(repo.root_path().join("wt_path.txt"))
        .expect("wt_path.txt should exist (hook should run from worktree root, not subdirectory)");
    let wt_path = wt_path.trim();
    assert_ne!(wt_path, ".", "worktree_path should not be relative '.'");
    assert!(
        wt_path.ends_with("repo"),
        "worktree_path should end with repo dir name, got: {wt_path}"
    );

    // worktree_name should be the directory name, not "unknown"
    let wt_name =
        fs::read_to_string(repo.root_path().join("wt_name.txt")).expect("wt_name.txt should exist");
    assert_eq!(
        wt_name.trim(),
        "repo",
        "worktree_name should be the directory name, not 'unknown'"
    );

    // Hook CWD should be the worktree root, not the subdirectory
    let hook_cwd = fs::read_to_string(repo.root_path().join("hook_cwd.txt"))
        .expect("hook_cwd.txt should exist");
    let hook_cwd = hook_cwd.trim();
    assert!(
        !hook_cwd.contains("src/components"),
        "Hook should run from worktree root, not subdirectory. CWD was: {hook_cwd}"
    );
    assert!(
        hook_cwd.ends_with("repo"),
        "Hook CWD should be worktree root, got: {hook_cwd}"
    );
}

// ============================================================================
// Combined User and Project Hooks Tests
// ============================================================================

/// Test that both user and project unnamed hooks of the same type run and get unique log names.
/// This exercises the unnamed index tracking when multiple unnamed hooks share the same hook type.
#[rstest]
fn test_user_and_project_unnamed_post_start(repo: TestRepo) {
    // Create project config with unnamed post-start hook
    repo.write_project_config(r#"post-start = "echo 'PROJECT_POST_START' > project_bg.txt""#);
    repo.commit("Add project config");

    // Write user config with unnamed hook AND pre-approve project command
    repo.write_test_config(
        r#"post-start = "echo 'USER_POST_START' > user_bg.txt"
"#,
    );
    repo.write_test_approvals(
        r#"[projects."../origin"]
approved-commands = ["echo 'PROJECT_POST_START' > project_bg.txt"]
"#,
    );

    snapshot_switch(
        "user_and_project_unnamed_post_start",
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

#[rstest]
fn test_user_and_project_post_start_both_run(repo: TestRepo) {
    // Create project config with post-start hook
    repo.write_project_config(r#"post-start = "echo 'PROJECT_POST_START' > project_bg.txt""#);
    repo.commit("Add project config");

    // Write user config with user hook AND pre-approve project command
    repo.write_test_config(
        r#"[post-start]
bg = "echo 'USER_POST_START' > user_bg.txt"
"#,
    );
    repo.write_test_approvals(
        r#"[projects."../origin"]
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
    // Write project config with pre-start hook
    repo.write_project_config(r#"pre-start = "echo 'STANDALONE_POST_CREATE' > hook_ran.txt""#);

    let mut cmd = crate::common::wt_command();
    cmd.current_dir(repo.root_path());
    cmd.env("WORKTRUNK_CONFIG_PATH", repo.test_config_path());
    cmd.args(["hook", "pre-start", "--yes"]);

    let output = cmd.output().unwrap();
    assert!(output.status.success(), "wt hook pre-start should succeed");

    // Hook runs in background — wait for it to write the marker file
    let marker = repo.root_path().join("hook_ran.txt");
    crate::common::wait_for_file_content(&marker);
    let content = fs::read_to_string(&marker).unwrap();
    assert!(content.contains("STANDALONE_POST_CREATE"));
}

#[rstest]
fn test_standalone_hook_pre_start_fails_on_failure(repo: TestRepo) {
    // pre-start hooks use FailFast like all other pre-* hooks — consistent with
    // the symmetric pre (blocking, fail-fast) / post (background, warn) pattern.
    repo.write_project_config(r#"pre-start = "exit 1""#);

    let output = repo
        .wt_command()
        .args(["hook", "pre-start", "--yes"])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "wt hook pre-start should exit non-zero when the hook fails (fail-fast, like all pre-* hooks)"
    );
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

    // Hook runs in background — wait for it to write the marker file
    let marker = repo.root_path().join("hook_ran.txt");
    crate::common::wait_for_file_content(&marker);
    let content = fs::read_to_string(&marker).unwrap();
    assert!(content.contains("STANDALONE_POST_MERGE"));
}

#[rstest]
fn test_standalone_hook_post_merge_combined_user_and_project(repo: TestRepo) {
    // Both user and project configs contribute to post-merge — a single
    // `Running post-merge:` announce line must cover both sources.
    repo.write_project_config(r#"post-merge = "echo 'PROJECT_RAN' > project.txt""#);
    repo.write_test_config(
        r#"[post-merge]
notify = "echo 'USER_RAN' > user.txt"
"#,
    );

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "hook", &["post-merge", "--yes"], None);
        assert_cmd_snapshot!("standalone_hook_post_merge_combined_sources", cmd);
    });

    let root = repo.root_path();
    wait_for_file(&root.join("user.txt"));
    wait_for_file(&root.join("project.txt"));
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
fn test_standalone_hook_post_remove(repo: TestRepo) {
    // Write project config with post-remove hook
    repo.write_project_config(r#"post-remove = "echo 'STANDALONE_POST_REMOVE' > hook_ran.txt""#);

    let mut cmd = crate::common::wt_command();
    cmd.current_dir(repo.root_path());
    cmd.env("WORKTRUNK_CONFIG_PATH", repo.test_config_path());
    cmd.args(["hook", "post-remove", "--yes"]);

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "wt hook post-remove should succeed (spawns in background)"
    );

    // Wait for background hook to complete and write content
    let marker = repo.root_path().join("hook_ran.txt");
    crate::common::wait_for_file_content(&marker);
    let content = fs::read_to_string(&marker).unwrap();
    assert!(content.contains("STANDALONE_POST_REMOVE"));
}

#[rstest]
fn test_standalone_hook_post_remove_foreground(repo: TestRepo) {
    // Write project config with post-remove hook
    repo.write_project_config(r#"post-remove = "echo 'FOREGROUND_POST_REMOVE' > hook_ran.txt""#);

    let mut cmd = crate::common::wt_command();
    cmd.current_dir(repo.root_path());
    cmd.env("WORKTRUNK_CONFIG_PATH", repo.test_config_path());
    cmd.args(["hook", "post-remove", "--yes", "--foreground"]);

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "wt hook post-remove --foreground should succeed"
    );

    // Hook runs in foreground, so marker should exist immediately
    let marker = repo.root_path().join("hook_ran.txt");
    assert!(marker.exists(), "post-remove hook should have run");
    let content = fs::read_to_string(&marker).unwrap();
    assert!(content.contains("FOREGROUND_POST_REMOVE"));
}

#[rstest]
fn test_standalone_hook_no_hooks_configured(repo: TestRepo) {
    // No project config, no user config with hooks: `wt hook` should exit 0
    // with a warning — running hooks that don't exist is a no-op, not an error.
    let mut cmd = crate::common::wt_command();
    cmd.current_dir(repo.root_path());
    cmd.env("WORKTRUNK_CONFIG_PATH", repo.test_config_path());
    cmd.args(["hook", "pre-start", "--yes"]);

    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "wt hook should exit 0 when no hooks configured, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("No pre-start hooks configured"),
        "stderr should warn about missing hooks, got: {stderr}"
    );
}

// ============================================================================
// Dry-Run Tests
// ============================================================================

/// --dry-run shows expanded commands without executing them
#[rstest]
fn test_hook_dry_run_shows_expanded_command(repo: TestRepo) {
    repo.write_project_config(r#"pre-merge = "echo branch={{ branch }}""#);

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    // No --yes needed: --dry-run skips approval
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "hook",
        &["pre-merge", "--dry-run"],
        Some(repo.root_path()),
    ));
}

/// --dry-run does not execute the hook command
#[rstest]
fn test_hook_dry_run_does_not_execute(repo: TestRepo) {
    repo.write_project_config(r#"pre-start = "echo 'SHOULD_NOT_RUN' > hook_ran.txt""#);

    let mut cmd = crate::common::wt_command();
    cmd.current_dir(repo.root_path());
    cmd.env("WORKTRUNK_CONFIG_PATH", repo.test_config_path());
    cmd.args(["hook", "pre-start", "--dry-run"]);

    let output = cmd.output().unwrap();
    assert!(output.status.success(), "dry-run should succeed");

    // Hook should NOT have run
    let marker = repo.root_path().join("hook_ran.txt");
    assert!(
        !marker.exists(),
        "dry-run should not execute the hook command"
    );
}

/// --dry-run shows named hooks with source:name labels
#[rstest]
fn test_hook_dry_run_named_hooks(repo: TestRepo) {
    repo.write_project_config(
        r#"pre-merge = [
    {lint = "pre-commit run --all-files"},
    {test = "cargo test"},
]
"#,
    );

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "hook",
        &["pre-merge", "--dry-run"],
        Some(repo.root_path()),
    ));
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

    // Wait for log files: runner log + per-command log (cmd-0, unnamed single command)
    let log_dir = resolve_git_common_dir(repo.root_path()).join("wt/logs");
    wait_for_file_count(&log_dir, "log", 2);

    // Hook logs live at `{branch}/project/post-start/{name}.log`.
    let post_start_dir = log_dir
        .join(worktrunk::path::sanitize_for_filename("main"))
        .join("project")
        .join("post-start");
    let cmd_log = fs::read_dir(&post_start_dir)
        .unwrap_or_else(|e| panic!("reading {post_start_dir:?}: {e}"))
        .filter_map(|e| e.ok())
        .find(|e| e.file_name().to_string_lossy().contains("cmd-0"))
        .expect("Should have a cmd-0 log file");

    // Wait for content to be written (command runs async)
    wait_for_file_content(&cmd_log.path());
    let log_content = fs::read_to_string(cmd_log.path()).unwrap();

    // Verify the hook actually ran and wrote output (not just that file was created)
    assert!(
        log_content.contains("HOOK_OUTPUT_MARKER"),
        "Log should contain hook output, got: {log_content}"
    );
}

#[rstest]
fn test_concurrent_hook_multiple_failures(repo: TestRepo) {
    // Write project config with multiple named hooks (table format).
    // Map configs run as a concurrent group in one pipeline runner,
    // each command producing its own log file.
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

    // Wait for per-command log files: runner log + first + second
    let log_dir = resolve_git_common_dir(repo.root_path()).join("wt/logs");
    wait_for_file_count(&log_dir, "log", 3);

    // Hook logs live at `{branch}/project/post-start/{name}.log`.
    let post_start_dir = log_dir
        .join(worktrunk::path::sanitize_for_filename("main"))
        .join("project")
        .join("post-start");
    let log_files: Vec<_> = fs::read_dir(&post_start_dir)
        .unwrap_or_else(|e| panic!("reading {post_start_dir:?}: {e}"))
        .filter_map(|e| e.ok())
        .collect();

    // Verify each command's output is in its own log file
    for (task, expected) in [("first", "FIRST_OUTPUT"), ("second", "SECOND_OUTPUT")] {
        let log_file = log_files
            .iter()
            .find(|e| e.file_name().to_string_lossy().starts_with(task))
            .unwrap_or_else(|| panic!("should have log file for {task}"));

        wait_for_file_content(&log_file.path());
        let content = fs::read_to_string(log_file.path()).unwrap();
        assert!(
            content.contains(expected),
            "Log for {task} should contain {expected}, got: {content}"
        );
    }
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

#[rstest]
fn test_hook_multiple_name_filters(repo: TestRepo) {
    // Write project config with three named hooks
    repo.write_project_config(
        r#"pre-merge = [
    {first = "echo FIRST"},
    {second = "echo SECOND"},
    {third = "echo THIRD"},
]
"#,
    );

    // Run only "first" and "second" by passing multiple names — "third" should not run
    assert_cmd_snapshot!(
        "hook_multiple_name_filters",
        make_snapshot_cmd(
            &repo,
            "hook",
            &["pre-merge", "first", "second", "--yes"],
            None
        )
    );
}

#[rstest]
fn test_hook_multiple_name_filters_none_match(repo: TestRepo) {
    // Write project config with named hooks
    repo.write_project_config(
        r#"[pre-merge]
first = "echo FIRST"
"#,
    );

    // Run with multiple names that don't match any configured hook
    assert_cmd_snapshot!(
        "hook_multiple_name_filters_none_match",
        make_snapshot_cmd(&repo, "hook", &["pre-merge", "foo", "bar", "--yes"], None)
    );
}

// ============================================================================
// Custom Variable (--var) Tests
// ============================================================================

#[rstest]
fn test_var_flag_overrides_template_variable(repo: TestRepo) {
    // Write user config with a hook that uses a template variable
    repo.write_test_config(
        r#"[pre-start]
test = "echo '{{ target }}' > target_output.txt"
"#,
    );

    let output = repo
        .wt_command()
        .args([
            "hook",
            "pre-start",
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
        r#"[pre-start]
test = "echo '{{ target }} {{ remote }}' > multi_var_output.txt"
"#,
    );

    let output = repo
        .wt_command()
        .args([
            "hook",
            "pre-start",
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
        r#"[pre-start]
test = "echo '{{ branch }}' > branch_output.txt"
"#,
    );

    let output = repo
        .wt_command()
        .args([
            "hook",
            "pre-start",
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
        .args(["hook", "pre-start", "--var", "no_equals_sign"])
        .output()
        .expect("Failed to run wt");

    assert!(!output.status.success(), "Invalid --var format should fail");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("expected KEY=VALUE"),
        "Error should mention invalid format, got: {stderr}"
    );
}

#[rstest]
fn test_var_flag_custom_variable(repo: TestRepo) {
    // Custom variable names (not built-in template vars) are accepted and
    // injected into the template context, matching alias behavior.
    repo.write_test_config(
        r#"[pre-start]
test = "echo '{{ custom_var }}' > custom_var_output.txt"
"#,
    );

    let output = repo
        .wt_command()
        .args(["hook", "pre-start", "--yes", "--var", "custom_var=hello"])
        .output()
        .expect("Failed to run wt hook");

    assert!(
        output.status.success(),
        "Custom variable should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output_file = repo.root_path().join("custom_var_output.txt");
    let contents = fs::read_to_string(&output_file).unwrap();
    assert!(
        contents.contains("hello"),
        "Custom variable should be expanded, got: {contents}"
    );
}

#[rstest]
fn test_var_flag_last_value_wins(repo: TestRepo) {
    // Test that when the same variable is specified multiple times, the last value wins
    repo.write_test_config(
        r#"[pre-start]
test = "echo '{{ target }}' > target_output.txt"
"#,
    );

    let output = repo
        .wt_command()
        .args([
            "hook",
            "pre-start",
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
fn test_var_shorthand_overrides_template_variable(repo: TestRepo) {
    // `--KEY=VALUE` is equivalent to `--var KEY=VALUE` for template variables.
    repo.write_test_config(
        r#"[pre-start]
test = "echo '{{ branch }}' > shorthand_output.txt"
"#,
    );

    let output = repo
        .wt_command()
        .args(["hook", "pre-start", "--yes", "--branch=SHORTHAND_BRANCH"])
        .output()
        .expect("Failed to run wt hook");

    assert!(
        output.status.success(),
        "Hook should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output_file = repo.root_path().join("shorthand_output.txt");
    let contents = fs::read_to_string(&output_file).unwrap();
    assert!(
        contents.contains("SHORTHAND_BRANCH"),
        "Shorthand should override template variable, got: {contents}"
    );
}

#[rstest]
fn test_var_shorthand_mixed_with_long_form(repo: TestRepo) {
    // Shorthand and `--var` forms coexist in the same invocation.
    repo.write_test_config(
        r#"[pre-start]
test = "echo '{{ branch }} {{ target }}' > mixed_output.txt"
"#,
    );

    let output = repo
        .wt_command()
        .args([
            "hook",
            "pre-start",
            "--yes",
            "--branch=SHORT",
            "--var",
            "target=LONG",
        ])
        .output()
        .expect("Failed to run wt hook");

    assert!(output.status.success());

    let output_file = repo.root_path().join("mixed_output.txt");
    let contents = fs::read_to_string(&output_file).unwrap();
    assert!(
        contents.contains("SHORT") && contents.contains("LONG"),
        "Both forms should coexist, got: {contents}"
    );
}

#[rstest]
fn test_var_shorthand_custom_variable(repo: TestRepo) {
    // Custom variable names (not built-in template vars) are accepted and
    // injected into the template context, matching alias behavior. Hyphens in
    // variable names are canonicalized to underscores.
    repo.write_test_config(
        r#"[pre-start]
test = "echo '{{ my_env }}' > custom_output.txt"
"#,
    );

    let output = repo
        .wt_command()
        .args(["hook", "pre-start", "--yes", "--my-env=staging"])
        .output()
        .expect("Failed to run wt hook");

    assert!(
        output.status.success(),
        "Custom variable should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output_file = repo.root_path().join("custom_output.txt");
    let contents = fs::read_to_string(&output_file).unwrap();
    assert!(
        contents.contains("staging"),
        "Custom variable with hyphens should be canonicalized and expanded, got: {contents}"
    );
}

#[rstest]
fn test_shorthand_unreferenced_forwards_to_args(repo: TestRepo) {
    // `--KEY=VALUE` shorthand for an unreferenced KEY is smart-routed to
    // `{{ args }}` — the hook template captures the flag verbatim.
    repo.write_test_config(
        r#"[pre-start]
test = "echo '{{ args }}' > args_output.txt"
"#,
    );

    let output = repo
        .wt_command()
        .args(["hook", "pre-start", "--yes", "--unused-var=value"])
        .output()
        .expect("Failed to run wt hook");

    assert!(
        output.status.success(),
        "Hook should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let contents = fs::read_to_string(repo.root_path().join("args_output.txt")).unwrap();
    assert!(
        contents.contains("--unused-var=value"),
        "Unreferenced shorthand should be forwarded to {{{{ args }}}}, got: {contents}"
    );
}

#[rstest]
fn test_shorthand_referenced_binds_not_args(repo: TestRepo) {
    // When KEY is referenced by any hook template, `--KEY=VALUE` binds
    // `{{ KEY }}` and is NOT forwarded to `{{ args }}`.
    repo.write_test_config(
        r#"[pre-start]
test = "echo '{{ my_env }}:{{ args }}' > combined_output.txt"
"#,
    );

    let output = repo
        .wt_command()
        .args(["hook", "pre-start", "--yes", "--my-env=staging"])
        .output()
        .expect("Failed to run wt hook");

    assert!(output.status.success());

    let contents = fs::read_to_string(repo.root_path().join("combined_output.txt")).unwrap();
    assert_eq!(contents.trim(), "staging:");
}

#[rstest]
fn test_post_double_dash_forwards_to_args(repo: TestRepo) {
    // Tokens after `--` forward verbatim into `{{ args }}`.
    repo.write_test_config(
        r#"[pre-start]
test = "echo '{{ args }}' > dashdash_output.txt"
"#,
    );

    let output = repo
        .wt_command()
        .args(["hook", "pre-start", "--yes", "--", "--fast", "extra"])
        .output()
        .expect("Failed to run wt hook");

    assert!(
        output.status.success(),
        "Hook should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let contents = fs::read_to_string(repo.root_path().join("dashdash_output.txt")).unwrap();
    assert!(
        contents.contains("--fast") && contents.contains("extra"),
        "Post-`--` tokens should forward verbatim to {{{{ args }}}}, got: {contents}"
    );
}

#[rstest]
fn test_var_deprecation_warning(repo: TestRepo) {
    // Explicit `--var` still force-binds but emits a deprecation warning
    // pointing at `--KEY=VALUE` shorthand.
    repo.write_test_config(
        r#"[pre-start]
test = "echo '{{ my_env }}' > deprecated_output.txt"
"#,
    );

    let output = repo
        .wt_command()
        .args(["hook", "pre-start", "--yes", "--var", "my_env=staging"])
        .output()
        .expect("Failed to run wt hook");

    assert!(output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--var is deprecated"),
        "Expected --var deprecation warning, got: {stderr}"
    );

    let contents = fs::read_to_string(repo.root_path().join("deprecated_output.txt")).unwrap();
    assert_eq!(contents.trim(), "staging");
}

#[rstest]
fn test_args_indexing_and_length_in_hook_template(repo: TestRepo) {
    // `{{ args }}` is a ShellArgs sequence — indexing, length, and iteration
    // all work the same as in alias templates.
    repo.write_test_config(
        r#"[pre-start]
test = "echo '{{ args[0] | default(__WT_QUOT__none__WT_QUOT__) }}:{{ args | length }}' > args_seq.txt"
"#
        .replace("__WT_QUOT__", "'")
        .as_str(),
    );

    let output = repo
        .wt_command()
        .args(["hook", "pre-start", "--yes", "--", "first", "second"])
        .output()
        .expect("Failed to run wt hook");

    assert!(output.status.success());

    let contents = fs::read_to_string(repo.root_path().join("args_seq.txt")).unwrap();
    assert_eq!(contents.trim(), "first:2");
}

#[rstest]
fn test_mixed_var_shorthand_and_forwarded_args(repo: TestRepo) {
    // Explicit `--var` binds, referenced shorthand binds, unreferenced shorthand
    // + post-`--` tokens forward — all coexist in one invocation without
    // cross-contamination.
    repo.write_test_config(
        r#"[pre-start]
test = "echo '{{ my_env }}|{{ override }}|{{ args }}' > mixed_output.txt"
"#,
    );

    let output = repo
        .wt_command()
        .args([
            "hook",
            "pre-start",
            "--yes",
            "--my-env=prod",
            "--var",
            "override=forced",
            "--unused=x",
            "--",
            "extra",
        ])
        .output()
        .expect("Failed to run wt hook");

    assert!(
        output.status.success(),
        "Hook should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let contents = fs::read_to_string(repo.root_path().join("mixed_output.txt")).unwrap();
    let trimmed = contents.trim();
    assert!(
        trimmed.starts_with("prod|forced|"),
        "Expected my_env + override bound, got: {trimmed}"
    );
    assert!(
        trimmed.contains("--unused=x") && trimmed.contains("extra"),
        "Unreferenced + post-`--` tokens should forward to {{{{ args }}}}, got: {trimmed}"
    );
}

#[test]
fn test_var_shorthand_does_not_leak_into_hook_show() {
    // `wt hook show` doesn't accept `--var`, so shorthand preprocessing must
    // leave its argv alone — an unknown flag should still produce clap's
    // "unexpected argument" error, not a template-variable error.
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_wt"))
        .args(["hook", "show", "--branch=feature"])
        .output()
        .expect("Failed to run wt");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unexpected argument") || stderr.contains("--branch"),
        "Expected clap to reject --branch on `hook show`, got: {stderr}"
    );
}

#[rstest]
fn test_var_flag_deprecated_alias_works(repo: TestRepo) {
    // Test that deprecated variable aliases (main_worktree, repo_root, worktree) can be overridden
    repo.write_test_config(
        r#"[pre-start]
test = "echo '{{ main_worktree }}' > alias_output.txt"
"#,
    );

    let output = repo
        .wt_command()
        .args([
            "hook",
            "pre-start",
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

// ============================================================================
// Hook Order Preservation Tests (Issue #737)
// ============================================================================

/// Test that user hooks execute in TOML insertion order, not alphabetical
/// See: https://github.com/max-sixty/worktrunk/issues/737
#[rstest]
fn test_user_hooks_preserve_toml_order(repo: TestRepo) {
    // Write user config with hooks in specific order (NOT alphabetical: vscode, claude, copy, submodule)
    // If order were alphabetical, it would be: claude, copy, submodule, vscode
    repo.write_test_config(
        r#"[pre-start]
vscode = "echo '1' >> hook_order.txt"
claude = "echo '2' >> hook_order.txt"
copy = "echo '3' >> hook_order.txt"
submodule = "echo '4' >> hook_order.txt"
"#,
    );

    snapshot_switch("user_hooks_preserve_order", &repo, &["--create", "feature"]);

    // Verify execution order by reading the output file
    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    let order_file = worktree_path.join("hook_order.txt");
    assert!(order_file.exists(), "hook_order.txt should be created");

    let contents = fs::read_to_string(&order_file).unwrap();
    let lines: Vec<&str> = contents.lines().collect();

    // Hooks should execute in TOML order: 1, 2, 3, 4
    assert_eq!(
        lines,
        vec!["1", "2", "3", "4"],
        "Hooks should execute in TOML insertion order (vscode, claude, copy, submodule)"
    );
}

// ============================================================================
// User Pre-Switch Hook Tests
// ============================================================================

/// Test that a pre-switch hook executes before switching to an existing worktree
#[rstest]
fn test_user_pre_switch_hook_executes(mut repo: TestRepo) {
    // Create a worktree to switch to
    let _feature_wt = repo.add_worktree("feature");

    // Write user config with pre-switch hook that creates a marker in the current worktree
    repo.write_test_config(
        r#"[pre-switch]
check = "echo 'USER_PRE_SWITCH_RAN' > pre_switch_marker.txt"
"#,
    );

    snapshot_switch("user_pre_switch_executes", &repo, &["feature"]);

    // Verify user hook ran in the source worktree (main), not the destination
    let marker_file = repo.root_path().join("pre_switch_marker.txt");
    assert!(
        marker_file.exists(),
        "User pre-switch hook should have created marker in source worktree"
    );

    let contents = fs::read_to_string(&marker_file).unwrap();
    assert!(
        contents.contains("USER_PRE_SWITCH_RAN"),
        "Marker file should contain expected content"
    );
}

/// Test that a failing pre-switch hook blocks the switch (including --create)
#[rstest]
fn test_user_pre_switch_failure_blocks_switch(repo: TestRepo) {
    // Write user config with failing pre-switch hook
    repo.write_test_config(
        r#"[pre-switch]
block = "exit 1"
"#,
    );

    // Failing pre-switch should prevent worktree creation
    snapshot_switch("user_pre_switch_failure", &repo, &["--create", "feature"]);

    // Worktree should NOT have been created
    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    assert!(
        !worktree_path.exists(),
        "Worktree should not be created when pre-switch hook fails"
    );
}

/// Test that --no-hooks skips the pre-switch hook
#[rstest]
fn test_user_pre_switch_skipped_with_no_hooks(repo: TestRepo) {
    // Write user config with pre-switch hook that creates a marker
    repo.write_test_config(
        r#"[pre-switch]
check = "echo 'SHOULD_NOT_RUN' > pre_switch_marker.txt"
"#,
    );

    snapshot_switch(
        "user_pre_switch_no_hooks",
        &repo,
        &["--create", "feature", "--no-hooks"],
    );

    // Pre-switch hook should NOT have run (--no-hooks skips all hooks)
    let marker_file = repo.root_path().join("pre_switch_marker.txt");
    assert!(
        !marker_file.exists(),
        "Pre-switch hook should be skipped with --no-hooks"
    );
}

/// Test that `wt hook pre-switch` runs pre-switch hooks manually
#[rstest]
fn test_user_pre_switch_manual_hook(repo: TestRepo) {
    repo.write_test_config(
        r#"[pre-switch]
check = "echo 'MANUAL_PRE_SWITCH' > pre_switch_marker.txt"
"#,
    );

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "hook", &["pre-switch"], None);
        assert_cmd_snapshot!("user_pre_switch_manual", cmd);
    });

    let marker_file = repo.root_path().join("pre_switch_marker.txt");
    assert!(
        marker_file.exists(),
        "Manual pre-switch hook should have created marker"
    );
}

/// Test that `{{ branch }}` in pre-switch hooks is the destination branch argument, not the source.
#[rstest]
fn test_user_pre_switch_branch_var_is_destination(mut repo: TestRepo) {
    let _feature_wt = repo.add_worktree("feature-dest");

    // Write pre-switch hook that records {{ branch }} into a marker file
    repo.write_test_config(
        r#"[pre-switch]
check = "echo '{{ branch }}' > pre_switch_branch.txt"
"#,
    );

    snapshot_switch(
        "user_pre_switch_branch_destination",
        &repo,
        &["feature-dest"],
    );

    // {{ branch }} should be the destination branch, not the source (main)
    let marker_file = repo.root_path().join("pre_switch_branch.txt");
    assert!(
        marker_file.exists(),
        "Pre-switch hook should have created marker"
    );
    let contents = fs::read_to_string(&marker_file).unwrap();
    assert_eq!(
        contents.trim(),
        "feature-dest",
        "{{{{ branch }}}} should be the destination branch 'feature-dest', got: '{}'",
        contents.trim(),
    );
}

/// When removing the current worktree, post-switch hooks should fire
/// because the user is implicitly switched back to the primary worktree.
/// Regression test for https://github.com/max-sixty/worktrunk/issues/1450
///
/// Config is committed before creating the worktree, so both worktrees
/// have .config/wt.toml — isolating the bug to the deleted-cwd problem.
#[rstest]
fn test_remove_current_worktree_fires_post_switch_hook(mut repo: TestRepo) {
    // Write and commit project config BEFORE creating the worktree,
    // so the feature worktree also has .config/wt.toml
    repo.write_project_config(
        r#"post-switch = "echo 'POST_SWITCH_AFTER_REMOVE' > post_switch_marker.txt""#,
    );
    repo.commit("Add project config with post-switch hook");

    let feature_path = repo.add_worktree("feature");

    // Remove from WITHIN the feature worktree (current worktree removal)
    repo.wt_command()
        .args(["remove", "feature", "--force-delete", "--yes"])
        .current_dir(&feature_path)
        .output()
        .unwrap();

    // Post-switch hook should fire in the primary worktree
    let marker = repo.root_path().join("post_switch_marker.txt");
    wait_for_file_content(&marker);
    let content = fs::read_to_string(&marker).unwrap();
    assert!(
        content.contains("POST_SWITCH_AFTER_REMOVE"),
        "Post-switch hook should run when removing current worktree, got: {content}"
    );
}

// ==========================================================================
// Active model: directional template variables
// ==========================================================================

/// Pre-switch to existing worktree: worktree_path = destination (Active),
/// base_worktree_path = source, cwd = source.
#[rstest]
fn test_pre_switch_vars_point_to_destination(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    // Hook captures worktree_path, base_worktree_path, and cwd
    repo.write_test_config(
        r#"[pre-switch]
capture = "echo 'wt_path={{ worktree_path }} base={{ base }} base_wt={{ base_worktree_path }} cwd={{ cwd }}' > pre_switch_vars.txt"
"#,
    );

    repo.wt_command()
        .args(["switch", "feature", "--yes"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    let vars_file = repo.root_path().join("pre_switch_vars.txt");
    let content = fs::read_to_string(&vars_file).unwrap();

    let feature_name = feature_path.file_name().unwrap().to_string_lossy();
    let main_name = repo
        .root_path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();

    // worktree_path should be the destination (Active)
    assert!(
        content.contains(&format!("/{feature_name} "))
            || content.contains(&format!(r"\{feature_name} ")),
        "worktree_path should point to destination '{feature_name}', got: {content}"
    );

    // base should be the source branch
    assert!(
        content.contains("base=main"),
        "base should be source branch 'main', got: {content}"
    );

    // cwd should be the source (where the hook actually runs)
    assert!(
        content.contains(&format!("/{main_name}")) || content.contains(&format!(r"\{main_name}")),
        "cwd should point to source worktree '{main_name}', got: {content}"
    );
}

/// Regression test for #2309: `wt switch -` should resolve the symbolic
/// argument before setting up pre-switch hook template variables, so
/// `{{ target }}`, `{{ target_worktree_path }}`, and the Active bare vars
/// (`{{ worktree_path }}`, `{{ worktree_name }}`) reflect the actual destination
/// instead of the raw `-` argument or the source worktree.
#[rstest]
fn test_pre_switch_vars_with_dash_shortcut(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    // Establish switch history: main -> feature. This records main as previous,
    // so `wt switch -` from feature resolves back to main.
    repo.wt_command()
        .args(["switch", "feature", "--yes"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    // Install the pre-switch hook after the history-building switch so the
    // capture reflects only the `-` switch we care about.
    repo.write_test_config(
        r#"[pre-switch]
capture = "echo 'target={{ target }} target_wt={{ target_worktree_path }} wt_path={{ worktree_path }} wt_name={{ worktree_name }}' > pre_switch_dash.txt"
"#,
    );

    let switch_output = repo
        .wt_command()
        .args(["switch", "-", "--yes"])
        .current_dir(&feature_path)
        .output()
        .unwrap();

    assert!(
        switch_output.status.success(),
        "`wt switch -` should succeed with a pre-switch hook referencing target_worktree_path.\n\
         stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&switch_output.stdout),
        String::from_utf8_lossy(&switch_output.stderr),
    );

    let vars_file = feature_path.join("pre_switch_dash.txt");
    let content = fs::read_to_string(&vars_file).unwrap();

    let main_name = repo
        .root_path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();

    assert!(
        content.contains("target=main"),
        "{{{{ target }}}} should resolve to 'main' when using `-`, got: {content}"
    );
    // Both `target_worktree_path` and `worktree_path` should end at the main
    // worktree directory — the layout is `... /<main_name> <next-field>=`.
    assert!(
        content.contains(&format!("/{main_name} wt_path="))
            || content.contains(&format!(r"\{main_name} wt_path=")),
        "{{{{ target_worktree_path }}}} should point to the main worktree, got: {content}"
    );
    assert!(
        content.contains(&format!("/{main_name} wt_name="))
            || content.contains(&format!(r"\{main_name} wt_name=")),
        "{{{{ worktree_path }}}} should point to the main worktree (Active), got: {content}"
    );
    assert!(
        content.contains(&format!("wt_name={main_name}")),
        "{{{{ worktree_name }}}} should be the main worktree name, got: {content}"
    );
}

/// Post-remove: target/target_worktree_path point to where user ends up.
#[rstest]
fn test_post_remove_has_target_vars(mut repo: TestRepo) {
    repo.add_worktree("feature");

    repo.write_test_config(
        r#"[post-remove]
capture = "echo 'branch={{ branch }} target={{ target }} target_wt={{ target_worktree_path }}' > ../postremove_target.txt"
"#,
    );

    repo.wt_command()
        .args(["remove", "feature", "--force-delete", "--yes"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    let vars_file = repo
        .root_path()
        .parent()
        .unwrap()
        .join("postremove_target.txt");
    crate::common::wait_for_file_content(&vars_file);

    let content = fs::read_to_string(&vars_file).unwrap();

    // branch should be the removed branch (Active)
    assert!(
        content.contains("branch=feature"),
        "branch should be removed branch 'feature', got: {content}"
    );

    // target should be the destination branch (where user ends up)
    assert!(
        content.contains("target=main"),
        "target should be destination 'main', got: {content}"
    );

    // target_worktree_path should be the primary worktree
    let main_name = repo
        .root_path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    assert!(
        content.contains(&main_name),
        "target_worktree_path should contain primary worktree name '{main_name}', got: {content}"
    );
}

/// Post-switch for existing switches: base vars reference the source worktree.
#[rstest]
fn test_post_switch_has_base_vars_for_existing(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    // Post-switch hooks run in the DESTINATION worktree (feature), so write
    // to a path relative to the worktree that will exist after switch.
    repo.write_test_config(
        r#"[post-switch]
capture = "echo 'branch={{ branch }} base={{ base }}' > post_switch_base.txt"
"#,
    );

    repo.wt_command()
        .args(["switch", "feature", "--yes"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    // File is written in the destination (feature) worktree
    let vars_file = feature_path.join("post_switch_base.txt");
    crate::common::wait_for_file_content(&vars_file);

    let content = fs::read_to_string(&vars_file).unwrap();

    // branch should be the destination (Active)
    assert!(
        content.contains("branch=feature"),
        "branch should be destination 'feature', got: {content}"
    );

    // base should be the source branch we switched from
    assert!(
        content.contains("base=main"),
        "base should be source 'main', got: {content}"
    );
}

/// cwd always exists on disk — even when worktree_path points to a deleted directory.
#[rstest]
fn test_cwd_always_exists_in_post_remove(mut repo: TestRepo) {
    repo.add_worktree("feature");

    repo.write_test_config(
        r#"[post-remove]
check = "test -d {{ cwd }} && echo 'cwd_exists=true' > ../cwd_check.txt || echo 'cwd_exists=false' > ../cwd_check.txt"
"#,
    );

    repo.wt_command()
        .args(["remove", "feature", "--force-delete", "--yes"])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    let check_file = repo.root_path().parent().unwrap().join("cwd_check.txt");
    crate::common::wait_for_file_content(&check_file);

    let content = fs::read_to_string(&check_file).unwrap();
    assert!(
        content.contains("cwd_exists=true"),
        "cwd should point to an existing directory, got: {content}"
    );
}

// ============================================================================
// Pipeline Tests (list form)
// ============================================================================

#[rstest]
fn test_user_post_start_pipeline_serial_ordering(repo: TestRepo) {
    // Pipeline: serial step creates a marker, concurrent step reads it.
    // Serial steps run in order, so the marker exists when the
    // concurrent step runs.
    repo.write_test_config(
        r#"post-start = [
    "echo SETUP_DONE > pipeline_marker.txt",
    { bg = "cat pipeline_marker.txt > bg_saw_marker.txt" }
]
"#,
    );

    snapshot_switch(
        "user_post_start_pipeline_ordering",
        &repo,
        &["--create", "feature"],
    );

    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    let bg_file = worktree_path.join("bg_saw_marker.txt");
    wait_for_file_content(&bg_file);

    let content = fs::read_to_string(&bg_file).unwrap();
    assert!(
        content.contains("SETUP_DONE"),
        "Concurrent step should see serial step's output, got: {content}"
    );
}

#[rstest]
fn test_user_post_start_pipeline_failure_skips_later_steps(repo: TestRepo) {
    // First step fails → second step should not run (pipeline aborts on failure).
    repo.write_test_config(
        r#"post-start = [
    "exit 1",
    { bg = "echo SHOULD_NOT_RUN > should_not_exist.txt" }
]
"#,
    );

    snapshot_switch(
        "user_post_start_pipeline_failure",
        &repo,
        &["--create", "feature"],
    );

    // Give background commands time to run (if they were going to)
    thread::sleep(SLEEP_FOR_ABSENCE_CHECK);

    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    let marker_file = worktree_path.join("should_not_exist.txt");
    assert!(
        !marker_file.exists(),
        "Later pipeline steps should NOT run after serial step failure"
    );
}

#[rstest]
fn test_user_post_start_pipeline_lazy_vars_foreground(repo: TestRepo) {
    // Pipeline step 1 sets a var, step 2 uses it via {{ vars.name }}.
    // Foreground mode exercises the in-process lazy re-expansion path
    // in run_hook_with_filter.
    repo.write_test_config(
        r#"post-start = [
    "git config worktrunk.state.main.vars.name '{{ branch | sanitize }}-postgres'",
    { db = "echo {{ vars.name }} > lazy_expanded.txt" }
]
"#,
    );

    // Run the hook in foreground on the main worktree.
    // Step 1 uses `git config` directly (avoids needing `wt` on PATH in CI).
    let mut cmd = crate::common::wt_command();
    cmd.current_dir(repo.root_path());
    cmd.env("WORKTRUNK_CONFIG_PATH", repo.test_config_path());
    cmd.args(["hook", "post-start", "--yes", "--foreground"]);

    let output = cmd.output().expect("Failed to run foreground hook");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "Foreground hook should succeed.\nstdout: {}\nstderr: {stderr}",
        String::from_utf8_lossy(&output.stdout),
    );

    // With foreground, marker file should exist immediately
    let marker_file = repo.root_path().join("lazy_expanded.txt");
    assert!(
        marker_file.exists(),
        "Foreground lazy expansion should create marker file"
    );

    let content = fs::read_to_string(&marker_file).unwrap().trim().to_string();
    assert_eq!(
        content, "main-postgres",
        "Lazy step should see var set by prior step"
    );
}

#[rstest]
fn test_user_post_start_pipeline_lazy_vars_background(repo: TestRepo) {
    // Pipeline step 1 sets a var via git config (not `wt config` — bare `wt`
    // isn't on PATH in the detached background process). Step 2 references
    // {{ vars.name }}, which is expanded just-in-time by the background
    // pipeline runner reading fresh vars from git config.
    repo.write_test_config(
        r#"post-start = [
    "git config worktrunk.state.{{ branch }}.vars.name '{{ branch | sanitize }}-postgres'",
    { db = "echo {{ vars.name }} > lazy_bg_expanded.txt" }
]
"#,
    );

    snapshot_switch(
        "user_post_start_pipeline_lazy_vars_bg",
        &repo,
        &["--create", "feature"],
    );

    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    let marker_file = worktree_path.join("lazy_bg_expanded.txt");
    wait_for_file_content(&marker_file);

    let content = fs::read_to_string(&marker_file).unwrap().trim().to_string();
    assert_eq!(
        content, "feature-postgres",
        "Background lazy step should see var set by prior step"
    );
}

#[rstest]
fn test_user_post_start_pipeline_concurrent_all_run(repo: TestRepo) {
    // Concurrent group: both commands should run and produce output.
    repo.write_test_config(
        r#"post-start = [
    { a = "echo AAA > concurrent_a.txt", b = "echo BBB > concurrent_b.txt" }
]
"#,
    );

    snapshot_switch(
        "user_post_start_pipeline_concurrent_all",
        &repo,
        &["--create", "feature"],
    );

    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    let file_a = worktree_path.join("concurrent_a.txt");
    let file_b = worktree_path.join("concurrent_b.txt");
    wait_for_file_content(&file_a);
    wait_for_file_content(&file_b);

    let a = fs::read_to_string(&file_a).unwrap();
    let b = fs::read_to_string(&file_b).unwrap();
    assert!(
        a.contains("AAA"),
        "concurrent command 'a' should run, got: {a}"
    );
    assert!(
        b.contains("BBB"),
        "concurrent command 'b' should run, got: {b}"
    );
}

#[rstest]
fn test_user_post_start_pipeline_concurrent_partial_failure(repo: TestRepo) {
    // One command in a concurrent group fails. The other should still
    // complete (pipeline waits for all children), and later steps should
    // not run (group reported as failed).
    repo.write_test_config(
        r#"post-start = [
    { fail = "exit 1", ok = "echo SURVIVED > concurrent_survivor.txt" },
    "echo SHOULD_NOT_RUN > after_concurrent.txt"
]
"#,
    );

    snapshot_switch(
        "user_post_start_pipeline_concurrent_failure",
        &repo,
        &["--create", "feature"],
    );

    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");

    // The surviving command should complete despite the sibling failing.
    let survivor = worktree_path.join("concurrent_survivor.txt");
    wait_for_file_content(&survivor);
    let content = fs::read_to_string(&survivor).unwrap();
    assert!(
        content.contains("SURVIVED"),
        "Non-failing concurrent command should still complete, got: {content}"
    );

    // The step after the failed group should NOT run.
    thread::sleep(SLEEP_FOR_ABSENCE_CHECK);
    let after = worktree_path.join("after_concurrent.txt");
    assert!(
        !after.exists(),
        "Steps after a failed concurrent group should not run"
    );
}

#[rstest]
fn test_user_post_start_pipeline_shell_escaping(repo: TestRepo) {
    // Template values containing shell metacharacters must be safely
    // escaped. Step 1 sets a var with spaces, quotes, and a dollar sign.
    // Step 2 expands it into a shell command — without shell_escape=true,
    // the value would be word-split or trigger expansion.
    repo.write_test_config(
        r#"post-start = [
    "git config worktrunk.state.{{ branch }}.vars.tricky 'hello world $HOME \"quotes\"'",
    { check = "echo {{ vars.tricky }} > escaped_output.txt" }
]
"#,
    );

    // Use foreground so we can check the result immediately.
    let mut cmd = crate::common::wt_command();
    cmd.current_dir(repo.root_path());
    cmd.env("WORKTRUNK_CONFIG_PATH", repo.test_config_path());
    cmd.args(["hook", "post-start", "--yes", "--foreground"]);

    let output = cmd.output().expect("Failed to run foreground hook");
    assert!(
        output.status.success(),
        "Hook should succeed.\nstderr: {}",
        String::from_utf8_lossy(&output.stderr),
    );

    let marker_file = repo.root_path().join("escaped_output.txt");
    assert!(marker_file.exists(), "Escaped output file should exist");

    let content = fs::read_to_string(&marker_file).unwrap().trim().to_string();
    // The value should arrive intact — not word-split, not $HOME-expanded.
    assert!(
        content.contains("hello world"),
        "Spaces should not cause word splitting, got: {content}"
    );
    assert!(
        content.contains("$HOME"),
        "$HOME should be literal, not expanded, got: {content}"
    );
    assert!(
        content.contains("\"quotes\""),
        "Quotes should survive escaping, got: {content}"
    );
}

// ============================================================================
// Pipeline hook_name isolation (Bug 2 regression test)
// ============================================================================

#[rstest]
fn test_user_post_start_pipeline_hook_name_per_step(repo: TestRepo) {
    // Each step in a pipeline should see its own hook_name, not the first step's name.
    // Before the fix, step 2 would see step 1's hook_name because the shared pipeline
    // context included hook_name from the first command's context_json.
    repo.write_test_config(
        r#"post-start = [
    { step_one = "echo {{ hook_name }} > step_one_name.txt" },
    { step_two = "echo {{ hook_name }} > step_two_name.txt" }
]
"#,
    );

    snapshot_switch(
        "user_post_start_pipeline_hook_name_per_step",
        &repo,
        &["--create", "feature"],
    );

    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");

    let step_one_file = worktree_path.join("step_one_name.txt");
    let step_two_file = worktree_path.join("step_two_name.txt");
    wait_for_file_content(&step_one_file);
    wait_for_file_content(&step_two_file);

    let step_one_name = fs::read_to_string(&step_one_file)
        .unwrap()
        .trim()
        .to_string();
    let step_two_name = fs::read_to_string(&step_two_file)
        .unwrap()
        .trim()
        .to_string();
    assert_eq!(
        step_one_name, "step_one",
        "Step 1 should see its own hook_name"
    );
    assert_eq!(
        step_two_name, "step_two",
        "Step 2 should see its own hook_name, not step 1's"
    );
}

#[rstest]
fn test_user_post_switch_pipeline_via_switch_create(repo: TestRepo) {
    // Post-switch with pipeline config, triggered by `wt switch --create`.
    // This exercises the pipeline branch in `spawn_switch_background_hooks`,
    // which spawns each hook type's pipeline independently.
    repo.write_test_config(
        r#"post-switch = [
    "echo SWITCH_STEP_1 > switch_step1.txt",
    { check = "cat switch_step1.txt > switch_step2.txt" }
]
"#,
    );

    snapshot_switch(
        "user_post_switch_pipeline_via_create",
        &repo,
        &["--create", "feature"],
    );

    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    let step2_file = worktree_path.join("switch_step2.txt");
    wait_for_file_content(&step2_file);

    let content = fs::read_to_string(&step2_file).unwrap();
    assert!(
        content.contains("SWITCH_STEP_1"),
        "Pipeline serial ordering should be preserved for post-switch, got: {content}"
    );
}

// ============================================================================
// Post-remove pipeline (Bug 1 regression test)
// ============================================================================

#[rstest]
fn test_user_post_remove_pipeline_serial_ordering(mut repo: TestRepo) {
    // Post-remove with a pipeline config should preserve serial ordering.
    // Before the fix, prepare_post_remove_commands returned flat commands,
    // so pipeline configs lost serial/concurrent semantics.
    let _feature_wt = repo.add_worktree("feature");

    repo.write_test_config(
        r#"post-remove = [
    "echo REMOVE_STEP_1 > ../remove_step1.txt",
    "cat ../remove_step1.txt > ../remove_step2.txt"
]
"#,
    );

    snapshot_remove(
        "user_post_remove_pipeline_ordering",
        &repo,
        &["feature", "--force-delete"],
        Some(repo.root_path()),
    );

    // Step 2 reads step 1's output. With pipeline semantics, step 2 runs after step 1.
    let parent = repo.root_path().parent().unwrap();
    let step2_file = parent.join("remove_step2.txt");
    wait_for_file_content(&step2_file);

    let content = fs::read_to_string(&step2_file).unwrap();
    assert!(
        content.contains("REMOVE_STEP_1"),
        "Step 2 should see step 1's output (serial pipeline), got: {content}"
    );
}

// ============================================================================
// Name-filtered lazy template (Bug 3 regression test)
// ============================================================================

#[rstest]
fn test_standalone_hook_name_filtered_lazy_template(repo: TestRepo) {
    // A pipeline step that uses {{ vars.X }} should expand correctly when
    // name-filtered via `wt hook post-start db`. Before the fix, the flat
    // spawn path passed the raw unexpanded template to the shell.
    //
    // vars.* are read from git config, so we pre-set the value.
    repo.write_test_config(
        r#"post-start = [
    { setup = "echo setup" },
    { db = "echo {{ vars.name }} > lazy_filtered.txt" }
]
"#,
    );

    // Pre-set vars.name via git config (same mechanism as pipeline step 1 would use).
    // Test repo starts on main branch.
    repo.run_git(&["config", "worktrunk.state.main.vars.name", "test-db"]);

    // Run just the 'db' step by name. This goes through the flat background path
    // since name filtering bypasses the pipeline runner.
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "hook", &["post-start", "db"], None);
        assert_cmd_snapshot!("standalone_hook_name_filtered_lazy_template", cmd);
    });

    let marker_file = repo.root_path().join("lazy_filtered.txt");
    wait_for_file_content(&marker_file);

    let content = fs::read_to_string(&marker_file).unwrap().trim().to_string();
    assert_eq!(
        content, "test-db",
        "Lazy template should expand {{ vars.name }} from git config"
    );
}

/// Multi-remove hook announcements include the branch name for disambiguation
#[rstest]
fn test_multi_remove_hook_announcements_include_branch(repo: TestRepo) {
    // fixture already has feature-a, feature-b, feature-c worktrees
    repo.write_test_config(
        r#"[post-remove]
cleanup = "echo done"
"#,
    );

    snapshot_remove(
        "multi_remove_hook_branch_context",
        &repo,
        &["feature-a", "feature-b", "--force-delete"],
        Some(repo.root_path()),
    );
}

/// Foreground hooks pass the directive file through to child processes,
/// so inner `wt switch --create` can write cd directives back to the
/// parent shell via the CD directive file.
#[rstest]
fn test_foreground_hook_passes_directive_file(repo: TestRepo) {
    use crate::common::{configure_directive_files, directive_files, wt_bin};

    repo.commit("initial");

    let wt = wt_bin();
    let wt_str = wt.to_string_lossy();
    assert!(
        !wt_str.contains('\''),
        "wt binary path should not contain single quotes: {wt_str}"
    );
    let wt_toml = wt_str.replace('\\', r"\\");

    // Pre-start hook that creates a new worktree via `wt switch --create`.
    // If the CD directive file is passed through, the inner wt will write a
    // path to it. If scrubbed, it prints the "shell integration not
    // installed" hint instead.
    repo.write_test_config(&format!(
        r#"
[pre-start]
setup = "'{wt_toml}' switch --create hook-created --no-hooks"
"#,
    ));

    let (cd_path, exec_path, _guard) = directive_files();

    let mut cmd = repo.wt_command();
    configure_directive_files(&mut cmd, &cd_path, &exec_path);
    // Run the pre-start hook manually in foreground
    cmd.args(["hook", "pre-start", "setup"]);
    let output = cmd.output().unwrap();

    assert!(
        output.status.success(),
        "hook failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let cd_content = std::fs::read_to_string(&cd_path).unwrap_or_default();
    assert!(
        !cd_content.trim().is_empty(),
        "foreground hook running `wt switch --create` should write a path to \
         the CD directive file, got: {cd_content:?}"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("shell integration"),
        "inner wt should not warn about shell integration being uninstalled, got: {stderr}",
    );
}

// ============================================================================
// Pre-* Pipeline Concurrent Execution Tests
// ============================================================================

/// Pipeline blocks in pre-* hooks run their concurrent commands concurrently.
/// The second block has two commands — both should produce output with prefixed
/// labels (the concurrent execution style).
#[rstest]
fn test_pre_merge_pipeline_concurrent_block(repo: TestRepo) {
    repo.write_project_config(
        r#"[[pre-merge]]
setup = "echo SETUP"

[[pre-merge]]
lint = "echo LINT"
test = "echo TEST"
"#,
    );
    repo.commit("Add pipeline pre-merge hooks");

    let mut cmd = repo.wt_command();
    cmd.args(["hook", "pre-merge", "--yes"]);
    let output = cmd.output().unwrap();

    assert!(
        output.status.success(),
        "pre-merge pipeline should succeed.\nstderr: {}",
        String::from_utf8_lossy(&output.stderr),
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    // All three commands should have run.
    assert!(stderr.contains("SETUP"), "setup step should run: {stderr}");
    assert!(stderr.contains("LINT"), "lint command should run: {stderr}");
    assert!(stderr.contains("TEST"), "test command should run: {stderr}");

    // Concurrent commands get prefixed labels (e.g., "lint │ LINT").
    // Serial commands do not. The "│" separator confirms the concurrent path.
    assert!(
        stderr.contains("│ LINT") && stderr.contains("│ TEST"),
        "concurrent block commands should have prefixed labels: {stderr}",
    );
}

/// Deprecated single-table form (`[pre-merge]`) still runs commands serially,
/// even though the commands are parsed as a `Concurrent` step.
#[rstest]
fn test_pre_merge_deprecated_table_runs_serially(repo: TestRepo) {
    repo.write_project_config(
        r#"[pre-merge]
lint = "echo LINT"
test = "echo TEST"
"#,
    );
    repo.commit("Add table-form pre-merge hooks");

    let mut cmd = repo.wt_command();
    cmd.args(["hook", "pre-merge", "--yes"]);
    let output = cmd.output().unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("LINT") && stderr.contains("TEST"),
        "both commands should run: {stderr}",
    );
    // Serial execution does not use the "│" prefix labels.
    assert!(
        !stderr.contains("│ LINT") && !stderr.contains("│ TEST"),
        "deprecated table form should run serially (no prefix labels): {stderr}",
    );
}

/// A single `[[pre-merge]]` block (one block, multiple entries) runs
/// concurrently — the `[[]]` syntax is the pipeline form even with one block.
#[rstest]
fn test_pre_merge_single_pipeline_block_runs_concurrently(repo: TestRepo) {
    repo.write_project_config(
        r#"[[pre-merge]]
lint = "echo LINT"
test = "echo TEST"
"#,
    );
    repo.commit("Add single-block pipeline pre-merge hooks");

    let mut cmd = repo.wt_command();
    cmd.args(["hook", "pre-merge", "--yes"]);
    let output = cmd.output().unwrap();

    assert!(
        output.status.success(),
        "single-block pipeline should succeed.\nstderr: {}",
        String::from_utf8_lossy(&output.stderr),
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("│ LINT") && stderr.contains("│ TEST"),
        "single [[pre-merge]] block should run concurrently: {stderr}",
    );
}

/// Under `-v`, hooks print a grouped table of resolved template variables —
/// see issue #2309 for why this helps users understand scope-dependent gaps.
#[rstest]
fn test_hook_verbose_prints_variable_table(mut repo: TestRepo) {
    repo.add_worktree("feature");
    repo.write_test_config(
        r#"[pre-switch]
noop = "true"
"#,
    );

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd =
            make_snapshot_cmd_with_global_flags(&repo, "switch", &["feature"], None, &["-v"]);
        assert_cmd_snapshot!("hook_verbose_variable_table", cmd);
    });
}

/// Background-path variable dump dedups per hook type when both user and
/// project configs contribute to the same hook — the table prints once, not
/// once per source. Also exercises the `PreparedStep::Single` arm of
/// `print_background_variable_tables`, which the all-named-commands case in
/// `test_post_start_verbose_shows_per_hook_output` doesn't hit.
#[rstest]
fn test_hook_verbose_background_dedups_across_sources(repo: TestRepo) {
    repo.write_test_config(r#"post-start = "echo user-hook""#);
    repo.write_project_config(r#"post-start = "echo project-hook""#);
    repo.commit("add post-start");
    repo.write_test_approvals(
        r#"[projects."../origin"]
approved-commands = ["echo project-hook"]
"#,
    );

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd_with_global_flags(
            &repo,
            "switch",
            &["--create", "feature"],
            None,
            &["-v"],
        );
        assert_cmd_snapshot!("hook_verbose_background_dedup", cmd);
    });
}
