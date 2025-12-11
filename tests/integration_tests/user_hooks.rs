//! Integration tests for user-level hooks (~/.config/worktrunk/config.toml)
//!
//! User hooks differ from project hooks:
//! - Run for all repositories
//! - Execute before project hooks
//! - Don't require approval
//! - Skipped together with project hooks via --no-verify

use crate::common::{
    TestRepo, make_snapshot_cmd, repo, setup_snapshot_settings, wait_for_file,
    wait_for_file_content,
};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::fs;
use std::process::Command;
use std::thread;
use std::time::Duration;

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

/// Skipped on Windows: snapshot output differs due to shell/path differences.
#[rstest]
#[cfg_attr(windows, ignore)]
fn test_user_post_create_hook_executes(repo: TestRepo) {
    // Write user config with post-create hook (no project config)
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[post-create]
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

/// Skipped on Windows: snapshot output differs due to shell/path differences.
#[rstest]
#[cfg_attr(windows, ignore)]
fn test_user_hooks_run_before_project_hooks(repo: TestRepo) {
    // Create project config with post-create hook
    repo.write_project_config(r#"post-create = "echo 'PROJECT_HOOK' >> hook_order.txt""#);
    repo.commit("Add project config");

    // Write user config with user hook AND pre-approve project command
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[post-create]
log = "echo 'USER_HOOK' >> hook_order.txt"

[projects."repo"]
approved-commands = ["echo 'PROJECT_HOOK' >> hook_order.txt"]
"#,
    );

    snapshot_switch("user_hooks_before_project", &repo, &["--create", "feature"]);

    // Verify execution order
    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    let order_file = worktree_path.join("hook_order.txt");
    assert!(order_file.exists(), "Hook order file should exist");

    let contents = fs::read_to_string(&order_file).unwrap();
    let lines: Vec<&str> = contents.lines().collect();

    assert_eq!(lines.len(), 2, "Should have two hooks executed");
    assert_eq!(lines[0], "USER_HOOK", "User hook should run first");
    assert_eq!(lines[1], "PROJECT_HOOK", "Project hook should run second");
}

/// Skipped on Windows: snapshot output differs due to shell/path differences.
#[rstest]
#[cfg_attr(windows, ignore)]
fn test_user_hooks_no_approval_required(repo: TestRepo) {
    // Write user config with hook but NO pre-approved commands
    // (unlike project hooks, user hooks don't require approval)
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[post-create]
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
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[post-create]
log = "echo 'USER_HOOK' > user_marker.txt"

[projects."repo"]
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
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[post-create]
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

/// Skipped on Windows: snapshot output differs due to shell/path differences.
#[rstest]
#[cfg_attr(windows, ignore)]
fn test_user_post_start_hook_executes(repo: TestRepo) {
    // Write user config with post-start hook (background)
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[post-start]
bg = "echo 'USER_POST_START_RAN' > user_bg_marker.txt"
"#,
    );

    snapshot_switch("user_post_start_executes", &repo, &["--create", "feature"]);

    // Wait for background hook to complete and write content
    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    let marker_file = worktree_path.join("user_bg_marker.txt");
    wait_for_file_content(&marker_file, Duration::from_secs(5));

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
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[post-start]
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

/// Skipped on Windows: snapshot output differs due to shell/path differences.
#[rstest]
#[cfg_attr(windows, ignore)]
fn test_user_pre_merge_hook_executes(mut repo: TestRepo) {
    // Create feature worktree with a commit
    let feature_wt = repo.add_worktree("feature");
    fs::write(feature_wt.join("feature.txt"), "feature content").unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.current_dir(&feature_wt)
        .args(["add", "."])
        .output()
        .unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.current_dir(&feature_wt)
        .args(["commit", "-m", "Add feature"])
        .output()
        .unwrap();

    // Write user config with pre-merge hook
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[pre-merge]
check = "echo 'USER_PRE_MERGE_RAN' > user_premerge.txt"
"#,
    );

    snapshot_merge(
        "user_pre_merge_executes",
        &repo,
        &["main", "--force", "--no-remove"],
        Some(&feature_wt),
    );

    // Verify user hook ran
    let marker_file = feature_wt.join("user_premerge.txt");
    assert!(marker_file.exists(), "User pre-merge hook should have run");
}

/// Skipped on Windows: snapshot output differs due to shell/path differences.
#[rstest]
#[cfg_attr(windows, ignore)]
fn test_user_pre_merge_hook_failure_blocks_merge(mut repo: TestRepo) {
    // Create feature worktree with a commit
    let feature_wt = repo.add_worktree("feature");
    fs::write(feature_wt.join("feature.txt"), "feature content").unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.current_dir(&feature_wt)
        .args(["add", "."])
        .output()
        .unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.current_dir(&feature_wt)
        .args(["commit", "-m", "Add feature"])
        .output()
        .unwrap();

    // Write user config with failing pre-merge hook
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[pre-merge]
check = "exit 1"
"#,
    );

    // Failing pre-merge hook should block the merge
    snapshot_merge(
        "user_pre_merge_failure",
        &repo,
        &["main", "--force", "--no-remove"],
        Some(&feature_wt),
    );
}

#[rstest]
fn test_user_pre_merge_skipped_with_no_verify(mut repo: TestRepo) {
    // Create feature worktree with a commit
    let feature_wt = repo.add_worktree("feature");
    fs::write(feature_wt.join("feature.txt"), "feature content").unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.current_dir(&feature_wt)
        .args(["add", "."])
        .output()
        .unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.current_dir(&feature_wt)
        .args(["commit", "-m", "Add feature"])
        .output()
        .unwrap();

    // Write user config with pre-merge hook that creates a marker
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[pre-merge]
check = "echo 'USER_PRE_MERGE' > user_premerge_marker.txt"
"#,
    );

    snapshot_merge(
        "user_pre_merge_skipped_no_verify",
        &repo,
        &["main", "--force", "--no-remove", "--no-verify"],
        Some(&feature_wt),
    );

    // User hook should NOT have run (--no-verify skips all hooks)
    let marker_file = feature_wt.join("user_premerge_marker.txt");
    assert!(
        !marker_file.exists(),
        "User pre-merge hook should be skipped with --no-verify"
    );
}

#[rstest]
#[cfg(unix)]
fn test_pre_merge_hook_receives_sigint(repo: TestRepo) {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    use std::io::Read;
    use std::process::Stdio;
    repo.commit("Initial commit");

    // Project pre-merge hook: write start, then sleep, then write done (if not interrupted)
    repo.write_project_config(
        r#"[pre-merge]
long = "sh -c 'echo start >> hook.log; sleep 30; echo done >> hook.log'"
"#,
    );
    repo.commit("Add pre-merge hook");

    // Spawn wt hook pre-merge (skip approval with --force)
    // Redirect stdout/stderr to null to prevent output leaking into test runner
    let mut cmd = crate::common::wt_command();
    cmd.current_dir(repo.root_path());
    cmd.args(["hook", "pre-merge", "--force"]);
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());
    let mut child = cmd.spawn().expect("failed to spawn wt hook pre-merge");

    // Wait until hook writes "start" to hook.log (verifies the hook is running)
    let hook_log = repo.root_path().join("hook.log");
    wait_for_file(&hook_log, Duration::from_secs(5));

    // Send SIGINT to wt (simulates Ctrl-C)
    kill(Pid::from_raw(child.id() as i32), Signal::SIGINT).expect("failed to send SIGINT");

    let status = child.wait().expect("failed to wait for wt");

    // Expect conventional Ctrl-C exit code 130
    assert_eq!(
        status.code(),
        Some(130),
        "wt should exit with 130 on SIGINT, status: {status:?}"
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

/// Skipped on Windows: snapshot output differs due to shell/path differences.
#[rstest]
#[cfg_attr(windows, ignore)]
fn test_user_post_merge_hook_executes(mut repo: TestRepo) {
    // Create feature worktree with a commit
    let feature_wt = repo.add_worktree("feature");
    fs::write(feature_wt.join("feature.txt"), "feature content").unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.current_dir(&feature_wt)
        .args(["add", "."])
        .output()
        .unwrap();

    let mut cmd = Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.current_dir(&feature_wt)
        .args(["commit", "-m", "Add feature"])
        .output()
        .unwrap();

    // Write user config with post-merge hook
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[post-merge]
notify = "echo 'USER_POST_MERGE_RAN' > user_postmerge.txt"
"#,
    );

    snapshot_merge(
        "user_post_merge_executes",
        &repo,
        &["main", "--force", "--no-remove"],
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

/// Skipped on Windows: Uses /tmp path and file locking prevents worktree removal.
#[rstest]
#[cfg_attr(windows, ignore)]
fn test_user_pre_remove_hook_executes(mut repo: TestRepo) {
    // Create a worktree to remove
    let _feature_wt = repo.add_worktree("feature");

    // Write user config with pre-remove hook
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[pre-remove]
cleanup = "echo 'USER_PRE_REMOVE_RAN' > /tmp/user_preremove_marker.txt"
"#,
    );

    snapshot_remove(
        "user_pre_remove_executes",
        &repo,
        &["feature", "--force-delete"],
        Some(repo.root_path()),
    );

    // Verify user hook ran (writes to /tmp since worktree is being removed)
    let marker_file = std::path::Path::new("/tmp/user_preremove_marker.txt");
    assert!(marker_file.exists(), "User pre-remove hook should have run");
    // Clean up
    let _ = fs::remove_file(marker_file);
}

/// Skipped on Windows: snapshot output differs due to shell/path differences.
#[rstest]
#[cfg_attr(windows, ignore)]
fn test_user_pre_remove_failure_blocks_removal(mut repo: TestRepo) {
    // Create a worktree to remove
    let feature_wt = repo.add_worktree("feature");

    // Write user config with failing pre-remove hook
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[pre-remove]
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

/// Skipped on Windows: snapshot output differs due to shell/path differences.
#[rstest]
#[cfg_attr(windows, ignore)]
fn test_user_pre_remove_skipped_with_no_verify(mut repo: TestRepo) {
    // Create a worktree to remove
    let feature_wt = repo.add_worktree("feature");

    // Write user config with pre-remove hook that would block
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[pre-remove]
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

/// Skipped on Windows: snapshot output differs due to shell/path differences.
#[rstest]
#[cfg_attr(windows, ignore)]
fn test_user_pre_commit_hook_executes(mut repo: TestRepo) {
    // Create feature worktree
    let feature_wt = repo.add_worktree("feature");

    // Add uncommitted changes (triggers pre-commit during merge)
    fs::write(feature_wt.join("uncommitted.txt"), "uncommitted content").unwrap();

    // Write user config with pre-commit hook
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[pre-commit]
lint = "echo 'USER_PRE_COMMIT_RAN' > user_precommit.txt"
"#,
    );

    snapshot_merge(
        "user_pre_commit_executes",
        &repo,
        &["main", "--force", "--no-remove"],
        Some(&feature_wt),
    );

    // Verify user hook ran
    let marker_file = feature_wt.join("user_precommit.txt");
    assert!(marker_file.exists(), "User pre-commit hook should have run");
}

/// Skipped on Windows: snapshot output differs due to shell/path differences.
#[rstest]
#[cfg_attr(windows, ignore)]
fn test_user_pre_commit_failure_blocks_commit(mut repo: TestRepo) {
    // Create feature worktree
    let feature_wt = repo.add_worktree("feature");

    // Add uncommitted changes
    fs::write(feature_wt.join("uncommitted.txt"), "uncommitted content").unwrap();

    // Write user config with failing pre-commit hook
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[pre-commit]
lint = "exit 1"
"#,
    );

    // Failing pre-commit hook should block the merge
    snapshot_merge(
        "user_pre_commit_failure",
        &repo,
        &["main", "--force", "--no-remove"],
        Some(&feature_wt),
    );
}

// ============================================================================
// Template Variable Tests
// ============================================================================

/// Skipped on Windows: snapshot output differs due to shell/path differences.
#[rstest]
#[cfg_attr(windows, ignore)]
fn test_user_hook_template_variables(repo: TestRepo) {
    // Write user config with hook using template variables
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[post-create]
vars = "echo 'repo={{ repo }} branch={{ branch }}' > template_vars.txt"
"#,
    );

    snapshot_switch("user_hook_template_vars", &repo, &["--create", "feature"]);

    // Verify template variables were expanded
    let worktree_path = repo.root_path().parent().unwrap().join("repo.feature");
    let vars_file = worktree_path.join("template_vars.txt");
    assert!(vars_file.exists(), "Template vars file should exist");

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

/// Skipped on Windows: snapshot output differs due to shell/path differences.
#[rstest]
#[cfg_attr(windows, ignore)]
fn test_user_and_project_post_start_both_run(repo: TestRepo) {
    // Create project config with post-start hook
    repo.write_project_config(r#"post-start = "echo 'PROJECT_POST_START' > project_bg.txt""#);
    repo.commit("Add project config");

    // Write user config with user hook AND pre-approve project command
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"

[post-start]
bg = "echo 'USER_POST_START' > user_bg.txt"

[projects."repo"]
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
    wait_for_file(&worktree_path.join("user_bg.txt"), Duration::from_secs(5));
    wait_for_file(
        &worktree_path.join("project_bg.txt"),
        Duration::from_secs(5),
    );

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
