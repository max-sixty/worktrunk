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

    // Normalize platform-specific spawn-error text so the snapshot is
    // identical on Unix (`No such file or directory (os error 2)`) and
    // Windows (`program not found`).
    insta::with_settings!({
        filters => vec![
            (r"No such file or directory \(os error \d+\)", "[SPAWN_FAIL]"),
            (r"program not found", "[SPAWN_FAIL]"),
        ],
    }, {
        assert_cmd_snapshot!(make_snapshot_cmd(
            &repo,
            "step",
            &["for-each", "--", "nonexistent-command-12345", "--some-arg"],
            None,
        ));
    });
}

/// argv boundaries from the post-`--` args reach the program intact. The
/// command is exec'd directly — no implicit shell — so each argv element
/// arrives at the child as a single argument regardless of its content.
/// See issue #2461 for the legacy bug where joining argv into `sh -c`
/// collapsed `python3 -c 'import sys; print(sys.argv[1:])' 'a b'` into a
/// shell syntax error.
#[rstest]
#[cfg(unix)]
fn test_for_each_preserves_argv_quoting(repo: TestRepo) {
    let output = repo
        .wt_command()
        .args([
            "step",
            "for-each",
            "--",
            "python3",
            "-c",
            "import sys; print(sys.argv[1:])",
            "a b",
        ])
        .output()
        .expect("run wt step for-each");

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        output.status.success(),
        "for-each should succeed when argv quoting is preserved: {combined}",
    );
    assert!(
        combined.contains("['a b']"),
        "expected python to receive 'a b' as a single argv element, got: {combined}",
    );
    assert!(
        !combined.contains("Syntax error"),
        "for-each should not produce shell syntax errors when argv has quoted args: {combined}",
    );
}

/// Spawn failure (program not found) is the one branch in the failure
/// handler that does NOT downcast to `WorktrunkError::ChildProcessExited`,
/// and it appears in JSON mode as `exit_code: null`. Without this test the
/// spawn-failed JSON path is unreachable from the integration suite
/// (#2089 review).
#[rstest]
#[cfg(unix)]
fn test_for_each_json_spawn_failure(repo: TestRepo) {
    let mut cmd = repo.wt_command();
    cmd.args([
        "step",
        "for-each",
        "--format=json",
        "--",
        "nonexistent-command-12345",
    ]);
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
    // after the first signal. for-each exec's argv directly with no implicit
    // shell, so shell features (`&&`, `$$`, `$(...)`) need an explicit
    // `sh -c` invocation.
    let marker_dir = tempfile::tempdir().expect("create marker tmpdir");
    let marker_path = marker_dir.path().to_string_lossy().to_string();

    let shell_cmd = format!("touch {marker_path}/$(basename \"$(pwd)\") && kill -TERM $$");

    let output = repo
        .wt_command()
        .args(["step", "for-each", "--", "sh", "-c", &shell_cmd])
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

/// `{{ commit }}` must resolve per-worktree when iterating across worktrees
/// whose branches differ from the running worktree's branch. This exercises
/// the `rev-parse <branch>` fallback in `build_hook_context` — the on-branch
/// cache-reuse path would return the main worktree's SHA for every iteration.
#[rstest]
fn test_for_each_commit_matches_per_worktree_head(repo: TestRepo) {
    // Give each fixture feature worktree a distinct HEAD so a buggy cache
    // reuse (same SHA everywhere) fails visibly.
    let mut expected = std::collections::HashMap::new();
    expected.insert("main".to_string(), repo.git_output(&["rev-parse", "HEAD"]));
    for branch in ["feature-a", "feature-b", "feature-c"] {
        let wt_path = repo
            .root_path()
            .parent()
            .unwrap()
            .join(format!("repo.{branch}"));
        repo.run_git_in(
            &wt_path,
            &["commit", "--allow-empty", "-m", &format!("{branch} tip")],
        );
        let sha = repo
            .git_command()
            .args(["rev-parse", "HEAD"])
            .current_dir(&wt_path)
            .run()
            .unwrap();
        expected.insert(
            branch.to_string(),
            String::from_utf8_lossy(&sha.stdout).trim().to_owned(),
        );
    }

    // echo both fields so we can match each SHA to its branch.
    let output = repo
        .wt_command()
        .args([
            "step",
            "for-each",
            "--",
            "echo",
            "{{ branch }} {{ commit }}",
        ])
        .output()
        .expect("run wt step for-each");

    assert!(
        output.status.success(),
        "for-each failed: stderr={}",
        String::from_utf8_lossy(&output.stderr),
    );

    // for-each pipes foreground output through wt's styling layer, which may
    // wrap lines in ANSI codes. Substring-match `branch sha` anywhere in the
    // combined output — each worktree's echo contributes exactly one such
    // pairing, so a mis-resolved SHA (e.g., main's SHA appearing after
    // feature-b's branch label) is still caught.
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    for (branch, sha) in &expected {
        let needle = format!("{branch} {sha}");
        assert!(
            combined.contains(&needle),
            "expected {branch}'s {{{{ commit }}}} = {sha} in output\noutput={combined}",
        );
    }
}

/// `{{ commit }}` for a sibling worktree on detached HEAD must resolve to that
/// worktree's HEAD — not the running worktree's HEAD. The cache key for
/// `WorkingTree::head_sha` is the worktree path, and HEAD is per-worktree, so
/// reading via `repo.current_worktree()` in a detached sibling would return
/// the main worktree's SHA. Build a divergence: advance `feature-b` past the
/// shared tip, then detach it at that new SHA so its HEAD differs from main.
#[rstest]
fn test_for_each_commit_detached_sibling_matches_per_worktree_head(repo: TestRepo) {
    let feature_b_path = repo.worktree_path("feature-b").to_path_buf();
    repo.run_git_in(
        &feature_b_path,
        &["commit", "--allow-empty", "-m", "feature-b tip"],
    );
    repo.detach_head_in_worktree("feature-b");

    let main_sha = repo.git_output(&["rev-parse", "HEAD"]);
    let feature_b_sha = repo.head_sha_in(&feature_b_path);
    assert_ne!(
        main_sha, feature_b_sha,
        "detached sibling must have a distinct HEAD so a buggy fallback to the running worktree's SHA is visible",
    );

    let output = repo
        .wt_command()
        .args([
            "step",
            "for-each",
            "--",
            "echo",
            "{{ branch }} {{ commit }}",
        ])
        .output()
        .expect("run wt step for-each");

    assert!(
        output.status.success(),
        "for-each failed: stderr={}",
        String::from_utf8_lossy(&output.stderr),
    );

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    // Detached HEAD surfaces as `{{ branch }} == "HEAD"`. The sibling's commit
    // must be its own HEAD, not the running worktree's.
    let needle = format!("HEAD {feature_b_sha}");
    assert!(
        combined.contains(&needle),
        "expected detached sibling's {{{{ commit }}}} = {feature_b_sha} in output\noutput={combined}",
    );
    assert!(
        !combined.contains(&format!("HEAD {main_sha}")),
        "detached sibling's {{{{ commit }}}} must not resolve to main's SHA {main_sha}\noutput={combined}",
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
