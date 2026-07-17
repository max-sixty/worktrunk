use crate::common::{TestRepo, repo, repo_with_remote};
use rstest::rstest;

#[rstest]
fn test_remote_push_dry_run_uses_branch_push_remote(#[from(repo_with_remote)] mut repo: TestRepo) {
    repo.add_feature();
    repo.run_git(&["config", "branch.feature.pushRemote", "origin"]);

    let output = repo
        .wt_command()
        .args(["push", "feature", "--dry-run"])
        .output()
        .expect("wt push should start");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success(), "wt push failed: {stderr}");
    assert!(stdout.contains("feature"), "missing branch: {stdout}");
    assert!(stdout.contains("origin"), "missing remote: {stdout}");
    assert!(
        stdout.contains("~/repo.feature"),
        "missing worktree path: {stdout}"
    );
}

#[rstest]
fn test_remote_push_dry_run_uses_push_default(#[from(repo_with_remote)] mut repo: TestRepo) {
    repo.add_feature();
    repo.run_git(&["config", "remote.pushDefault", "origin"]);

    let output = repo
        .wt_command()
        .args(["push", "feature", "--dry-run"])
        .output()
        .expect("wt push should start");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success(), "wt push failed: {stderr}");
    assert!(stdout.contains("origin"), "missing remote: {stdout}");
}

#[rstest]
fn test_remote_push_dry_run_uses_origin_fallback(#[from(repo_with_remote)] mut repo: TestRepo) {
    repo.add_feature();

    let output = repo
        .wt_command()
        .args(["push", "feature", "--dry-run"])
        .output()
        .expect("wt push should start");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success(), "wt push failed: {stderr}");
    assert!(
        stdout.contains("origin"),
        "missing origin fallback: {stdout}"
    );
}

#[rstest]
fn test_remote_push_dry_run_prefers_origin_over_checkout_default_remote(
    #[from(repo_with_remote)] mut repo: TestRepo,
) {
    repo.setup_custom_remote("upstream", "main");
    repo.run_git(&["config", "checkout.defaultRemote", "upstream"]);
    repo.add_feature();

    let output = repo
        .wt_command()
        .args(["push", "feature", "--dry-run"])
        .output()
        .expect("wt push should start");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success(), "wt push failed: {stderr}");
    assert!(stdout.contains("origin"), "missing origin: {stdout}");
    assert!(
        !stdout.contains("upstream"),
        "used checkout default remote: {stdout}"
    );
}

#[rstest]
fn test_remote_push_explicit_remote_overrides_metadata(
    #[from(repo_with_remote)] mut repo: TestRepo,
) {
    repo.add_feature();
    repo.run_git(&["config", "branch.feature.pushRemote", "other"]);

    let output = repo
        .wt_command()
        .args(["push", "feature", "origin", "--dry-run"])
        .output()
        .expect("wt push should start");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success(), "wt push failed: {stderr}");
    assert!(stdout.contains("origin"), "missing chosen remote: {stdout}");
    assert!(!stdout.contains("other"), "used metadata remote: {stdout}");
}

#[rstest]
fn test_remote_push_without_remote_explains_how_to_choose_remote() {
    let mut repo = TestRepo::with_initial_commit();
    repo.add_feature();

    let output = repo
        .wt_command()
        .args(["push", "feature", "--dry-run"])
        .output()
        .expect("wt push should start");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "wt push unexpectedly succeeded");
    assert!(
        stderr.contains("No push remote configured") && stderr.contains("feature"),
        "missing remote guidance: {stderr}"
    );
    assert!(
        stderr.contains("wt push feature") && stderr.contains("<remote>"),
        "missing explicit remote hint: {stderr}"
    );
}

#[rstest]
fn test_remote_push_without_linked_worktree_explains_how_to_create_one(repo: TestRepo) {
    let output = repo
        .wt_command()
        .args(["push", "feature", "--dry-run"])
        .output()
        .expect("wt push should start");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "wt push unexpectedly succeeded");
    assert!(
        stderr.contains("No linked worktree") && stderr.contains("feature"),
        "missing worktree guidance: {stderr}"
    );
    assert!(
        stderr.contains("wt switch feature"),
        "missing creation hint: {stderr}"
    );
}

#[rstest]
fn test_remote_push_reports_selected_remote_and_sets_upstream(
    #[from(repo_with_remote)] mut repo: TestRepo,
) {
    let feature_worktree = repo.add_feature();
    repo.run_git(&["config", "branch.feature.pushRemote", "origin"]);

    let output = repo
        .wt_command()
        .args(["push", "feature"])
        .output()
        .expect("wt push should start");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(output.status.success(), "wt push failed: {stderr}");
    assert!(stderr.contains("Pushed") && stderr.contains("feature"));
    assert!(stderr.contains("origin"), "missing remote: {stderr}");
    assert!(stderr.contains("~/repo.feature"), "missing path: {stderr}");
    let upstream = repo
        .git_command()
        .current_dir(&feature_worktree)
        .args(["rev-parse", "--abbrev-ref", "@{upstream}"])
        .run()
        .expect("read feature upstream");
    assert!(upstream.status.success());
    assert_eq!(
        String::from_utf8_lossy(&upstream.stdout).trim(),
        "origin/feature"
    );

    repo.run_git(&["config", "--unset", "branch.feature.pushRemote"]);
    let fallback = repo
        .wt_command()
        .args(["push", "feature", "--dry-run"])
        .output()
        .expect("tracking remote fallback should start");
    let fallback_stderr = String::from_utf8_lossy(&fallback.stderr);
    let fallback_stdout = String::from_utf8_lossy(&fallback.stdout);
    assert!(
        fallback.status.success(),
        "wt push failed: {fallback_stderr}"
    );
    assert!(
        fallback_stdout.contains("origin"),
        "tracking remote was not selected: {fallback_stdout}"
    );
}
