//! Integration tests for `wt step diff`

use crate::common::{TestRepo, make_snapshot_cmd, repo, setup_snapshot_settings};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::fs;
use std::path::Path;

/// Helper: create a feature worktree with a commit ahead of main
fn setup_feature_with_commit(repo: &mut TestRepo) -> std::path::PathBuf {
    let feature_path = repo.add_worktree("feature");
    fs::write(feature_path.join("feature.txt"), "feature content").unwrap();
    repo.run_git_in(&feature_path, &["add", "feature.txt"]);
    repo.run_git_in(&feature_path, &["commit", "-m", "Add feature file"]);
    feature_path
}

/// No changes: worktree identical to merge base
#[rstest]
fn test_step_diff_no_changes(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");
    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["diff"],
        Some(&feature_path),
    ));
}

/// Committed changes show full diff by default
#[rstest]
fn test_step_diff_committed_changes(mut repo: TestRepo) {
    let feature_path = setup_feature_with_commit(&mut repo);
    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["diff"],
        Some(&feature_path),
    ));
}

/// Untracked files appear in diff
#[rstest]
fn test_step_diff_untracked_files(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");
    fs::write(feature_path.join("untracked.txt"), "untracked content").unwrap();
    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["diff"],
        Some(&feature_path),
    ));
}

/// All change types: committed + staged + unstaged + untracked
#[rstest]
fn test_step_diff_all_change_types(mut repo: TestRepo) {
    let feature_path = setup_feature_with_commit(&mut repo);

    // Staged change
    fs::write(feature_path.join("staged.txt"), "staged content").unwrap();
    repo.run_git_in(&feature_path, &["add", "staged.txt"]);

    // Unstaged change (modify a tracked file)
    fs::write(feature_path.join("feature.txt"), "modified content").unwrap();

    // Untracked file
    fs::write(feature_path.join("new.txt"), "new content").unwrap();

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["diff"],
        Some(&feature_path),
    ));
}

/// Extra args are forwarded to git diff
#[rstest]
fn test_step_diff_extra_args(mut repo: TestRepo) {
    let feature_path = setup_feature_with_commit(&mut repo);
    fs::write(feature_path.join("untracked.txt"), "untracked content").unwrap();
    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["diff", "--", "--stat"],
        Some(&feature_path),
    ));
}

/// Real index is unchanged after running diff
#[rstest]
fn test_step_diff_index_unchanged(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");
    fs::write(feature_path.join("untracked.txt"), "content").unwrap();

    // Get index state before
    let index_before = git_status(&repo, &feature_path);

    // Run step diff
    let mut cmd = repo.wt_command();
    cmd.args(["step", "diff"]).current_dir(&feature_path);
    let output = cmd.output().unwrap();
    assert!(output.status.success(), "step diff failed");

    // Get index state after
    let index_after = git_status(&repo, &feature_path);

    assert_eq!(
        index_before, index_after,
        "Real git index was modified by step diff"
    );
}

/// Cleanup is wired into index creation in both storage locations.
#[cfg(unix)]
#[rstest]
#[case(false)]
#[case(true)]
fn test_step_diff_sweeps_stale_indexes(repo: TestRepo, #[case] fallback: bool) {
    let scratch = worktrunk::testing::test_tempdir();
    let temp_dir = if fallback {
        scratch.path().join("missing")
    } else {
        scratch.path().to_path_buf()
    };
    let index_dir = if fallback {
        repo.root_path().join(".git")
    } else {
        temp_dir.clone()
    };
    let orphan = index_dir.join("worktrunk-temp-index-AbC123");
    let file = fs::File::create(&orphan).unwrap();
    file.set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::UNIX_EPOCH))
        .unwrap();
    drop(file);
    // A matching directory cannot be removed as a file; cleanup must continue.
    let obstacle = index_dir.join("worktrunk-temp-index-directory");
    fs::create_dir(&obstacle).unwrap();
    fs::File::open(&obstacle)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::UNIX_EPOCH))
        .unwrap();
    let real_index = repo.root_path().join(".git/index");
    let before = fs::read(&real_index).unwrap();
    fs::write(
        repo.root_path().join("untracked.txt"),
        "untracked content\n",
    )
    .unwrap();

    let output = repo
        .wt_command()
        .args(["step", "diff", "--", "--name-only"])
        .env("TMPDIR", &temp_dir)
        .env("RUST_LOG", "debug")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(&format!(
            "Failed to remove stale temporary index {}:",
            obstacle.display()
        )),
        "cleanup failure should be visible in debug logging"
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "untracked.txt"
    );
    assert!(!orphan.exists(), "stale index must be reclaimed");
    assert!(obstacle.is_dir(), "cleanup must not remove directories");
    assert_eq!(fs::read(real_index).unwrap(), before);
    assert!(
        fs::read_dir(index_dir).unwrap().all(|entry| {
            let entry = entry.unwrap();
            entry.path() == obstacle
                || !entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("worktrunk-temp-index-")
        }),
        "normal exit must remove the new index too"
    );
}

/// When neither storage location is writable, diff fails without touching the index.
#[cfg(unix)]
#[rstest]
fn test_step_diff_preserves_index_when_temp_index_creation_fails(repo: TestRepo) {
    use std::os::unix::fs::PermissionsExt;

    struct RestorePermissions<'a>(&'a Path, fs::Permissions);
    impl Drop for RestorePermissions<'_> {
        fn drop(&mut self) {
            let _ = fs::set_permissions(self.0, self.1.clone());
        }
    }

    let git_dir = repo.root_path().join(".git");
    let real_index = git_dir.join("index");
    let before = fs::read(&real_index).unwrap();
    fs::write(
        repo.root_path().join("untracked.txt"),
        "untracked content\n",
    )
    .unwrap();
    let permissions = fs::metadata(&git_dir).unwrap().permissions();
    let read_only = fs::Permissions::from_mode(permissions.mode() & !0o222);
    let restore = RestorePermissions(&git_dir, permissions);
    fs::set_permissions(&git_dir, read_only).unwrap();

    let scratch = worktrunk::testing::test_tempdir();
    let output = repo
        .wt_command()
        .args(["step", "diff"])
        .env("TMPDIR", scratch.path().join("missing"))
        .output()
        .unwrap();
    drop(restore);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "diff unexpectedly succeeded");
    assert!(
        stderr.contains(
            "Failed to create temporary index in the system temp directory or Git directory"
        ),
        "{stderr}"
    );
    assert_eq!(fs::read(real_index).unwrap(), before);
}

/// Explicit target branch
#[rstest]
fn test_step_diff_explicit_target(mut repo: TestRepo) {
    let feature_path = setup_feature_with_commit(&mut repo);
    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["diff", "main"],
        Some(&feature_path),
    ));
}

/// `--branch` diffs another worktree's branch from a different worktree
#[rstest]
fn test_step_diff_branch_arg(mut repo: TestRepo) {
    setup_feature_with_commit(&mut repo);
    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    // Run from the main worktree (repo root, cwd = None), targeting `feature`.
    // Output should match `test_step_diff_committed_changes`, which runs the
    // same diff from inside the feature worktree.
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["diff", "--branch", "feature"],
        None,
    ));
}

/// `--branch` for a branch without a worktree errors
#[rstest]
fn test_step_diff_branch_no_worktree(repo: TestRepo) {
    repo.run_git(&["branch", "lonely"]);
    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["diff", "--branch", "lonely"],
        None,
    ));
}

/// `--branch=` (empty value) is rejected at the parse boundary with a clear
/// usage error, not surfaced as a garbled `Branch  has no worktree`.
#[rstest]
fn test_step_diff_branch_empty(repo: TestRepo) {
    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["diff", "--branch="],
        None,
    ));
}

/// A branch checked out in two worktrees (as `git worktree add --force`
/// produces) resolves to the first, warning once and naming every path so the
/// otherwise-silent choice is visible.
#[rstest]
fn test_step_diff_duplicate_branch_warns(mut repo: TestRepo) {
    repo.add_worktree("feature");
    let dup_path = repo.root_path().parent().unwrap().join("repo.feature-dup");
    repo.run_git(&[
        "worktree",
        "add",
        "--force",
        dup_path.to_str().unwrap(),
        "feature",
    ]);

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["diff", "--branch=feature"],
        None,
    ));
}

fn git_status(repo: &TestRepo, dir: &Path) -> String {
    let output = repo
        .git_command()
        .args(["status", "--porcelain"])
        .current_dir(dir)
        .run()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// `--branch` and the target both take a selector: a worktree's path names the
/// worktree to diff, and names the branch to diff against.
#[rstest]
fn step_diff_accepts_worktree_paths(mut repo: TestRepo) {
    let feature_path = setup_feature_with_commit(&mut repo);
    let other_path = repo.add_worktree("other");

    let by_branch = repo
        .wt_command()
        .args(["step", "diff", "--branch", "feature"])
        .output()
        .unwrap();
    let by_path = repo
        .wt_command()
        .args(["step", "diff", "--branch", feature_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(by_branch.status.success() && by_path.status.success());
    assert_eq!(
        by_branch.stdout, by_path.stdout,
        "the branch and its worktree's path should name the same worktree"
    );

    // And the positional target, which is a ref rather than a worktree.
    let target_by_path = repo
        .wt_command()
        .current_dir(&feature_path)
        .args(["step", "diff", other_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        target_by_path.status.success(),
        "a worktree path should name the branch checked out there: {}",
        String::from_utf8_lossy(&target_by_path.stderr)
    );
}

/// `@` resolves to the current branch, the same as everywhere else in wt.
#[rstest]
fn step_diff_branch_flag_accepts_shortcut(mut repo: TestRepo) {
    let feature_path = setup_feature_with_commit(&mut repo);

    let output = repo
        .wt_command()
        .current_dir(&feature_path)
        .args(["step", "diff", "--branch", "@"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "--branch @ should resolve to the current worktree: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("feature.txt"),
        "the diff should be the current worktree's"
    );
}
