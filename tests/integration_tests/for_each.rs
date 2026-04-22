//! Integration tests for `wt step for-each`

use crate::common::{TestRepo, make_snapshot_cmd, repo};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;

#[rstest]
fn test_for_each_single_worktree(repo: TestRepo) {
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["for-each", "--", "git", "status", "--short"],
        None,
    ));
}

#[rstest]
fn test_for_each_multiple_worktrees(mut repo: TestRepo) {
    repo.add_worktree("feature-a");
    repo.add_worktree("feature-b");

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["for-each", "--", "git", "branch", "--show-current"],
        None,
    ));
}

#[rstest]
fn test_for_each_command_fails_in_one(mut repo: TestRepo) {
    repo.add_worktree("feature");

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["for-each", "--", "git", "show", "nonexistent-ref"],
        None,
    ));
}

#[rstest]
fn test_for_each_no_args_error(repo: TestRepo) {
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "step", &["for-each"], None));
}

#[rstest]
fn test_for_each_with_detached_head(mut repo: TestRepo) {
    repo.add_worktree("detached-test");
    repo.detach_head_in_worktree("detached-test");

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["for-each", "--", "git", "status", "--short"],
        None,
    ));
}

#[rstest]
fn test_for_each_with_template(repo: TestRepo) {
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["for-each", "--", "echo", "Branch: {{ branch }}"],
        None,
    ));
}

#[rstest]
fn test_for_each_detached_branch_variable(mut repo: TestRepo) {
    repo.add_worktree("detached-test");
    repo.detach_head_in_worktree("detached-test");

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["for-each", "--", "echo", "Branch: {{ branch }}"],
        None,
    ));
}

#[rstest]
fn test_for_each_spawn_fails(mut repo: TestRepo) {
    repo.add_worktree("feature");

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["for-each", "--", "nonexistent-command-12345", "--some-arg"],
        None,
    ));
}

/// Force the shell spawn itself to fail (rather than the command inside the
/// shell exiting non-zero) by setting `PATH` to a directory that contains
/// only `git` (so wt can still operate) but no `sh` (so the child shell
/// spawn fails). This is the only branch in the failure handler that does
/// NOT downcast to `WorktrunkError::ChildProcessExited`, and it appears in
/// JSON mode as `exit_code: null`. Without this test the spawn-failed JSON
/// path is unreachable from the integration suite (#2089 review).
#[rstest]
#[cfg(unix)]
fn test_for_each_json_spawn_failure(repo: TestRepo) {
    use std::path::PathBuf;

    // Locate a real `git` so we can symlink it into the minimal PATH dir.
    // wt itself shells out to git constantly; clearing PATH entirely makes
    // wt fail before it ever reaches the shell-spawn branch we want to test.
    let git_path: PathBuf = std::env::var_os("PATH")
        .iter()
        .flat_map(std::env::split_paths)
        .map(|p| p.join("git"))
        .find(|p| p.is_file())
        .expect("git must be in PATH for tests");

    let tmp = tempfile::tempdir().expect("create tmpdir for minimal PATH");
    std::os::unix::fs::symlink(&git_path, tmp.path().join("git"))
        .expect("symlink git into minimal PATH");
    // Deliberately do NOT symlink `sh` — that's what makes the shell spawn fail.

    let mut cmd = repo.wt_command();
    cmd.env("PATH", tmp.path());
    cmd.args(["step", "for-each", "--format=json", "--", "true"]);
    let output = cmd.output().unwrap();

    assert!(
        !output.status.success(),
        "for-each should fail when shell spawn fails: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "for-each --format=json should emit valid JSON on spawn failure: {e}\nstdout: {stdout}"
        )
    });
    let items = json.as_array().expect("JSON output should be an array");
    assert!(!items.is_empty(), "expected at least one worktree result");
    for item in items {
        assert_eq!(item["success"], false);
        // Spawn failure ⇒ no exit code (vs. exit-code path which uses an integer)
        assert!(
            item["exit_code"].is_null(),
            "spawn failure should report exit_code: null, got {item}"
        );
        let error = item["error"]
            .as_str()
            .expect("error field should be a string");
        assert!(
            !error.is_empty(),
            "spawn failure error message should be non-empty"
        );
    }
}

/// Signal-derived exit (Ctrl-C, SIGTERM) in a child must abort the loop
/// rather than continuing into the remaining worktrees. Simulated here with
/// a command that self-signals via SIGTERM — this drives the same
/// `ChildProcessExited { signal: Some(_), .. }` path as a real Ctrl-C against
/// the wt process. Sending SIGINT to the parent wt process from an integration
/// test is impractical (it would kill the test harness), so we cover the
/// signal-detection branch via an equivalent in-child signal.
#[rstest]
#[cfg(unix)]
fn test_for_each_aborts_on_signal_exit(repo: TestRepo) {
    // The standard fixture already includes main + feature-{a,b,c} worktrees,
    // so we just need the command to abort on the first visit.

    // A marker file per visited worktree lets us assert that the loop stopped
    // after the first signal. for-each joins the post-`--` args with spaces
    // and runs the result through `sh -c`; we pass shell fragments that
    // touch a marker and then self-signal with SIGTERM.
    let marker_dir = tempfile::tempdir().expect("create marker tmpdir");
    let marker_path = marker_dir.path().to_string_lossy().to_string();

    let touch_cmd = format!("touch {marker_path}/$(basename \"$(pwd)\")");

    let output = repo
        .wt_command()
        .args([
            "step", "for-each", "--", &touch_cmd, "&&", "kill", "-TERM", "$$",
        ])
        .output()
        .expect("run wt step for-each");

    // Exit code: 128 + SIGTERM (15) = 143
    assert_eq!(
        output.status.code(),
        Some(143),
        "expected exit 143 (SIGTERM), got {:?}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr),
    );

    // Exactly one marker file should exist — the remaining worktrees must
    // not have been visited after the signal aborted the loop.
    let markers: Vec<_> = std::fs::read_dir(marker_dir.path())
        .expect("read marker dir")
        .filter_map(Result::ok)
        .collect();
    assert_eq!(
        markers.len(),
        1,
        "expected exactly one worktree visited before abort, got {}: stderr={}",
        markers.len(),
        String::from_utf8_lossy(&output.stderr),
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Interrupted"),
        "expected 'Interrupted' message in stderr, got: {stderr}"
    );
}

#[rstest]
fn test_for_each_skips_prunable_worktrees(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature");
    // Delete the worktree directory to make it prunable
    std::fs::remove_dir_all(&worktree_path).unwrap();

    // Verify git sees it as prunable
    let output = repo
        .git_command()
        .args(["worktree", "list", "--porcelain"])
        .run()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("prunable"),
        "Expected worktree to be prunable after deleting directory"
    );

    // wt step for-each should skip the prunable worktree and complete without errors
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["for-each", "--", "echo", "Running in {{ branch }}"],
        None,
    ));
}

// ============================================================================
// --format=json
// ============================================================================

#[rstest]
fn test_for_each_json(mut repo: TestRepo) {
    repo.commit("initial");
    repo.add_worktree("feature");

    let output = repo
        .wt_command()
        .args(["step", "for-each", "--format=json", "--", "true"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();
    let items = json.as_array().unwrap();
    assert!(items.len() >= 2, "expected at least 2 worktrees");
    for item in items {
        assert_eq!(item["success"], true);
        assert_eq!(item["exit_code"], 0);
        assert!(item["path"].as_str().is_some());
    }
    // feature worktree should be in results
    assert!(
        items.iter().any(|i| i["branch"] == "feature"),
        "feature branch should be in results"
    );
}

#[rstest]
fn test_for_each_json_with_failure(repo: TestRepo) {
    repo.commit("initial");

    let output = repo
        .wt_command()
        .args(["step", "for-each", "--format=json", "--", "false"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();
    let items = json.as_array().unwrap();
    assert!(!items.is_empty());
    for item in items {
        assert_eq!(item["success"], false);
        assert_eq!(item["exit_code"], 1);
        // error field contains the raw message from the child process
        assert_eq!(item["error"], "exit status: 1");
    }
}
