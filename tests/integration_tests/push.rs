use crate::common::{
    TestRepo, make_snapshot_cmd, repo, repo_with_feature_worktree, repo_with_remote,
    setup_snapshot_settings,
};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;

/// Helper to create snapshot with normalized paths
fn snapshot_push(test_name: &str, repo: &TestRepo, args: &[&str], cwd: Option<&std::path::Path>) {
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        // Prepend "push" to args for `wt step push` command
        let mut step_args = vec!["push"];
        step_args.extend_from_slice(args);
        let mut cmd = make_snapshot_cmd(repo, "step", &step_args, cwd);
        assert_cmd_snapshot!(test_name, cmd);
    });
}

#[rstest]
fn test_push_fast_forward(mut repo: TestRepo) {
    // Create a worktree for main
    repo.add_main_worktree();

    // Make a commit in a feature worktree
    let feature_wt =
        repo.add_worktree_with_commit("feature", "test.txt", "test content", "Add test file");

    // Push from feature to main
    snapshot_push("push_fast_forward", &repo, &["main"], Some(&feature_wt));
}

/// With no detectable width (piped output, no COLUMNS), the diffstat omits
/// `--stat-width` and lets git pick its default. Back when "no width" was a
/// `usize::MAX` sentinel, it reached git as a real `--stat-width` and made
/// git truncate filenames to ~10 chars.
#[rstest]
fn test_push_diffstat_without_detectable_width(mut repo: TestRepo) {
    repo.add_main_worktree();
    let feature_wt = repo.add_worktree_with_commit(
        "feature",
        "a-reasonably-long-filename.txt",
        "test content",
        "Add test file",
    );

    let mut cmd = repo.wt_command();
    cmd.args(["step", "push", "main"])
        .current_dir(&feature_wt)
        .env_remove("COLUMNS");
    let output = cmd.output().expect("failed to run command");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "push failed: {stderr}");
    assert!(
        stderr.contains("a-reasonably-long-filename.txt"),
        "diffstat should show the full filename, got: {stderr}"
    );
}

#[rstest]
fn test_push_to_default_branch(#[from(repo_with_feature_worktree)] repo: TestRepo) {
    let feature_wt = repo.worktree_path("feature");

    // Push without specifying target (should use default branch)
    snapshot_push("push_to_default", &repo, &[], Some(feature_wt));
}

#[rstest]
fn test_push_with_dirty_target(mut repo: TestRepo) {
    // Make main worktree (repo root) dirty with a conflicting file
    std::fs::write(repo.root_path().join("conflict.txt"), "old content").unwrap();

    let feature_wt = repo.add_worktree_with_commit(
        "feature",
        "conflict.txt",
        "new content",
        "Add conflict file",
    );

    // Try to push (should fail due to conflicting changes)
    snapshot_push(
        "push_dirty_target_overlap",
        &repo,
        &["main"],
        Some(&feature_wt),
    );

    // Ensure target worktree still has original file content and no stash was created
    let main_contents = std::fs::read_to_string(repo.root_path().join("conflict.txt")).unwrap();
    assert_eq!(main_contents, "old content");

    let stash_list = repo.git_command().args(["stash", "list"]).run().unwrap();
    assert!(
        String::from_utf8_lossy(&stash_list.stdout)
            .trim()
            .is_empty()
    );
}

/// Uncommitted changes in the target worktree that don't overlap the push
/// range never move: the two-tree merge carries them in place — an unstaged
/// edit stays unstaged, a staged entry stays staged, an untracked file stays
/// untracked. The stash list stays empty throughout, so nothing `wt` does can
/// collide with another process's stash operations (#3683's whole class).
#[rstest]
fn test_push_dirty_target_changes_stay_in_place(mut repo: TestRepo) {
    // A tracked file committed to main before the feature branches, so the
    // push range doesn't touch it.
    let root = repo.root_path().to_path_buf();
    std::fs::write(root.join("tracked.txt"), "base").unwrap();
    repo.run_git(&["add", "tracked.txt"]);
    repo.run_git(&["commit", "-m", "Add tracked file"]);

    // Non-overlapping uncommitted state in the target worktree (repo root).
    std::fs::write(root.join("tracked.txt"), "unstaged-edit").unwrap();
    std::fs::write(root.join("staged.txt"), "staged").unwrap();
    repo.run_git(&["add", "staged.txt"]);
    std::fs::write(root.join("untracked.txt"), "untracked").unwrap();

    let feature_wt = repo.add_feature();

    snapshot_push(
        "push_dirty_target_carried",
        &repo,
        &["main"],
        Some(&feature_wt),
    );

    // The push landed and every uncommitted change sits exactly where it was.
    assert_eq!(
        repo.git_output(&["rev-parse", "main"]),
        repo.git_output(&["rev-parse", "feature"])
    );
    assert_eq!(
        repo.git_output(&["diff", "--name-only"]),
        "tracked.txt",
        "the unstaged edit must stay unstaged"
    );
    assert_eq!(
        repo.git_output(&["diff", "--cached", "--name-only"]),
        "staged.txt",
        "the staged entry must stay staged"
    );
    assert_eq!(
        repo.git_output(&["ls-files", "--others", "--exclude-standard"]),
        "untracked.txt",
        "the untracked file must stay untracked"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("tracked.txt")).unwrap(),
        "unstaged-edit"
    );

    // Nothing wt does touches the stash list.
    let stash_list = repo.git_command().args(["stash", "list"]).run().unwrap();
    assert!(
        String::from_utf8_lossy(&stash_list.stdout)
            .trim()
            .is_empty()
    );
}

#[rstest]
fn test_push_dirty_target_overlap_renamed_file(mut repo: TestRepo) {
    // Regression test: overlap detection must detect conflicts when a file is renamed
    // in the source branch but has uncommitted changes under the old name in the target.
    //
    // Setup:
    // 1. main has file.txt (committed)
    // 2. main (target) has uncommitted modifications to file.txt
    // 3. feature renames file.txt -> renamed.txt (committed)
    // 4. Push from feature to main should FAIL (conflict on the same file)

    // Create initial file in main
    repo.commit_in_worktree(
        repo.root_path(),
        "file.txt",
        "original content",
        "Initial file",
    );

    // Create feature branch from main
    let feature_wt = repo.add_worktree("feature");

    // Make uncommitted changes to file.txt in main (target worktree)
    std::fs::write(repo.root_path().join("file.txt"), "modified in target").unwrap();

    // In feature worktree, rename file.txt to renamed.txt and commit
    repo.run_git_in(&feature_wt, &["mv", "file.txt", "renamed.txt"]);
    repo.run_git_in(
        &feature_wt,
        &["commit", "-m", "Rename file.txt to renamed.txt"],
    );

    // Try to push from feature to main (should fail due to conflicting changes)
    // The renamed file.txt (now renamed.txt) conflicts with uncommitted file.txt changes
    snapshot_push(
        "push_dirty_target_overlap_renamed_file",
        &repo,
        &["main"],
        Some(&feature_wt),
    );

    // Ensure target worktree still has the modified file.txt and no stash was created
    let main_contents = std::fs::read_to_string(repo.root_path().join("file.txt")).unwrap();
    assert_eq!(main_contents, "modified in target");

    let stash_list = repo.git_command().args(["stash", "list"]).run().unwrap();
    assert!(
        String::from_utf8_lossy(&stash_list.stdout)
            .trim()
            .is_empty()
    );
}

#[rstest]
fn test_push_error_not_fast_forward(#[from(repo_with_remote)] mut repo: TestRepo) {
    // Create feature branch from initial commit
    let feature_wt = repo.add_worktree("feature");

    // Make a commit in the main worktree (repo root) and push it
    // Note: Must match original file layout for snapshot consistency
    repo.commit_in_worktree(
        repo.root_path(),
        "main-file.txt",
        "main content",
        "Main commit",
    );
    repo.push_branch("main");

    // Make a commit in feature (which doesn't have main's commit)
    repo.commit_in_worktree(
        &feature_wt,
        "feature.txt",
        "feature content",
        "Feature commit",
    );

    // Try to push feature to main (should fail - main has commits not in feature)
    snapshot_push(
        "push_error_not_fast_forward",
        &repo,
        &["main"],
        Some(&feature_wt),
    );
}

#[rstest]
fn test_push_with_merge_commits(mut repo: TestRepo) {
    // Create feature branch with initial commit
    let feature_wt = repo.add_worktree_with_commit("feature", "file1.txt", "content1", "Commit 1");

    // Create another branch for merging
    repo.run_git_in(&feature_wt, &["checkout", "-b", "temp"]);

    repo.commit_in_worktree(&feature_wt, "file2.txt", "content2", "Commit 2");

    // Switch back to feature and merge temp (creating merge commit)
    repo.run_git_in(&feature_wt, &["checkout", "feature"]);
    repo.run_git_in(
        &feature_wt,
        &["merge", "temp", "--no-ff", "-m", "Merge temp"],
    );

    // Push to main (should succeed - merge commits are allowed)
    snapshot_push(
        "push_with_merge_commits",
        &repo,
        &["main"],
        Some(&feature_wt),
    );
}

#[rstest]
fn test_push_no_ff(mut repo: TestRepo) {
    repo.add_main_worktree();

    let feature_wt =
        repo.add_worktree_with_commit("feature", "test.txt", "test content", "Add test file");

    // Push with --no-ff should create a merge commit on main
    snapshot_push("push_no_ff", &repo, &["--no-ff", "main"], Some(&feature_wt));

    // Verify a merge commit was created (HEAD on main should have 2 parents)
    let cat_file = repo.git_output(&["cat-file", "-p", "main"]);
    let parents: Vec<&str> = cat_file
        .lines()
        .filter(|l| l.starts_with("parent "))
        .collect();
    assert_eq!(
        parents.len(),
        2,
        "Merge commit should have exactly 2 parents"
    );

    // Verify the merge commit message
    let commit_msg = repo.git_output(&["log", "-1", "--format=%s", "main"]);
    assert_eq!(commit_msg, "Merge branch 'feature' into main");
}

#[rstest]
fn test_push_with_submodule_recurse_config(mut repo: TestRepo) {
    // Regression test for https://github.com/max-sixty/worktrunk/issues/1604:
    // submodule.recurse=true once broke the push stage. The update is now
    // plumbing (`update-ref` + `read-tree`) the setting can't reach; this
    // pins that hostile config stays harmless.
    repo.run_git(&["config", "submodule.recurse", "true"]);

    repo.add_main_worktree();

    let feature_wt =
        repo.add_worktree_with_commit("feature", "test.txt", "test content", "Add test file");

    // Fast-forward push should succeed despite submodule.recurse=true
    snapshot_push(
        "push_with_submodule_recurse",
        &repo,
        &["main"],
        Some(&feature_wt),
    );
}

#[rstest]
fn test_push_no_ff_with_submodule_recurse_config(mut repo: TestRepo) {
    // Regression test for https://github.com/max-sixty/worktrunk/issues/1604
    repo.run_git(&["config", "submodule.recurse", "true"]);

    repo.add_main_worktree();

    let feature_wt =
        repo.add_worktree_with_commit("feature", "test.txt", "test content", "Add test file");

    // No-ff push should succeed despite submodule.recurse=true
    snapshot_push(
        "push_no_ff_with_submodule_recurse",
        &repo,
        &["--no-ff", "main"],
        Some(&feature_wt),
    );
}

/// An ignored file at a path the push range tracks is silently overwritten —
/// the documented Data Safety carve-out (module spec: the same split a
/// `git merge` run in the target would produce). Pinned here because
/// read-tree's deprecated `--exclude-per-directory` help text suggests the
/// opposite; modern git applies the standard excludes during the sync by
/// default.
#[rstest]
fn test_push_overwrites_ignored_file_at_tracked_path(mut repo: TestRepo) {
    let root = repo.root_path().to_path_buf();
    std::fs::write(root.join(".gitignore"), "dist.txt\n").unwrap();
    repo.run_git(&["add", ".gitignore"]);
    repo.run_git(&["commit", "-m", "Add gitignore"]);

    let feature_wt = repo.add_worktree("feature");
    std::fs::write(feature_wt.join("dist.txt"), "tracked").unwrap();
    repo.run_git_in(&feature_wt, &["add", "-f", "dist.txt"]);
    repo.run_git_in(&feature_wt, &["commit", "-m", "Track dist.txt"]);

    // Ignored file in the target worktree at the path the push now tracks.
    std::fs::write(root.join("dist.txt"), "IGNORED-LOCAL").unwrap();

    let output = repo
        .wt_command()
        .args(["step", "push", "main"])
        .current_dir(&feature_wt)
        .output()
        .expect("failed to run push");
    assert!(
        output.status.success(),
        "push must overwrite the ignored file, matching git: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(root.join("dist.txt")).unwrap(),
        "tracked"
    );
}

/// An untracked file nested in an untracked directory still gets the upfront
/// named refusal. The overlap check lists untracked files individually
/// (`-uall`); default porcelain collapses the directory to a single `dir/`
/// entry, which can never match a file path in the push range — the refusal
/// would then come from the sync backstop, without naming the file.
#[rstest]
fn test_push_dirty_target_overlap_in_untracked_dir(mut repo: TestRepo) {
    let root = repo.root_path().to_path_buf();
    let feature_wt = repo.add_worktree("feature");
    std::fs::create_dir_all(feature_wt.join("docs/api")).unwrap();
    std::fs::write(feature_wt.join("docs/api/index.md"), "tracked").unwrap();
    repo.run_git_in(&feature_wt, &["add", "docs"]);
    repo.run_git_in(&feature_wt, &["commit", "-m", "Add docs"]);

    // The same path, untracked inside an untracked directory, in the target.
    std::fs::create_dir_all(root.join("docs/api")).unwrap();
    std::fs::write(root.join("docs/api/index.md"), "LOCAL").unwrap();

    let target_before = repo.git_output(&["rev-parse", "main"]);
    let output = repo
        .wt_command()
        .args(["step", "push", "main"])
        .current_dir(&feature_wt)
        .output()
        .expect("failed to run push");

    assert!(
        !output.status.success(),
        "an overlapping untracked file must refuse the push: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("docs/api/index.md"),
        "the refusal must name the conflicting file: {stderr}"
    );
    assert_eq!(
        repo.git_output(&["rev-parse", "main"]),
        target_before,
        "the ref must not move"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("docs/api/index.md")).unwrap(),
        "LOCAL"
    );
}

/// A target worktree parked mid-operation refuses the push, whether the
/// operation left HEAD on the branch or detached it.
///
/// `advance_target`'s `read-tree -m -u` refuses an unmerged index, but a
/// stopped operation whose conflict has been staged has a merged one — so
/// without the upfront gate the sync writes the push range into a worktree
/// partway through, and the user's `--continue` commits the synced tree as the
/// operation's result. The conflict is staged at a path the push range never
/// touches, so `ensure_no_target_conflicts` passes and only the operation gate
/// can catch this.
///
/// Both shapes are pinned because they reach the gate differently. A stopped
/// cherry-pick keeps HEAD on the branch, so `worktree_for_branch` finds the
/// worktree directly; a rebase detaches HEAD, and `git worktree list
/// --porcelain` reports that worktree with no branch at all — the lookup
/// succeeds only because `finalize_worktree` backfills the branch from
/// `rebase-merge/head-name`. Were that backfill to stop covering the rebase
/// case, `target_worktree_path` would go `None`, the gate would be skipped, and
/// `advance_target` would return right after its compare-and-swap: the ref
/// moves, the worktree is never synced, and `git rebase --continue` then resets
/// the branch to the rebase result with the pushed commits off it. The
/// fast-forward path this replaced got that refusal from git itself, so the
/// rebase arm is the one place the guarantee rests on a helper of ours.
#[rstest]
#[case::cherry_pick("cherry-pick", ".git/CHERRY_PICK_HEAD")]
#[case::rebase("rebase", ".git/rebase-merge")]
fn test_push_refuses_target_mid_operation(
    mut repo: TestRepo,
    #[case] operation: &str,
    #[case] state_marker: &str,
) {
    let root = repo.root_path().to_path_buf();

    // A side commit that conflicts with main at `conflict.txt`.
    repo.run_git(&["checkout", "-b", "side"]);
    std::fs::write(root.join("conflict.txt"), "side\n").unwrap();
    repo.run_git(&["add", "conflict.txt"]);
    repo.run_git(&["commit", "-m", "Side conflict"]);
    repo.run_git(&["checkout", "main"]);
    std::fs::write(root.join("conflict.txt"), "main\n").unwrap();
    repo.run_git(&["add", "conflict.txt"]);
    repo.run_git(&["commit", "-m", "Main conflict"]);

    // feature descends from main and touches only `feature.txt`.
    let feature_wt = repo.add_feature();

    // Stop the operation in main's worktree, then stage its resolution: the
    // operation stays open while the index goes clean.
    let _ = repo.git_command().args([operation, "side"]).run();
    repo.run_git(&["checkout", "--ours", "conflict.txt"]);
    repo.run_git(&["add", "conflict.txt"]);
    assert!(
        root.join(state_marker).exists(),
        "the fixture must leave a {operation} open in the target worktree"
    );
    // The rebase arm proves nothing about the backfill unless its worktree is
    // really detached, so assert the shape rather than assuming it.
    let head_detached = repo.git_output(&["rev-parse", "--abbrev-ref", "HEAD"]) == "HEAD";
    assert_eq!(
        head_detached,
        operation == "rebase",
        "unexpected HEAD shape after stopping a {operation} in the target worktree"
    );

    let target_before = repo.git_output(&["rev-parse", "main"]);
    let output = repo
        .wt_command()
        .args(["step", "push", "main"])
        .current_dir(&feature_wt)
        .output()
        .expect("failed to run push");

    assert!(
        !output.status.success(),
        "a target worktree mid-operation must refuse the push: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("already in progress") && stderr.contains("main"),
        "the refusal must name the target worktree holding the operation: {stderr}"
    );
    assert_eq!(
        repo.git_output(&["rev-parse", "main"]),
        target_before,
        "the ref must not move"
    );
    assert!(
        !root.join("feature.txt").exists(),
        "the worktree sync must not have run"
    );
}

/// A fast-forward whose target-worktree sync fails is rolled back whole.
///
/// A locked index in the target worktree stands in for a sync failure in the
/// window after the upfront conflict check. The ref update is rolled back and
/// the command exits non-zero — the fast-forward path's version of the
/// rollback `test_merge_no_ff_sync_failure_rolls_back` proves for `--no-ff`.
#[rstest]
fn test_push_sync_failure_rolls_back(mut repo: TestRepo) {
    // The repo root is main's worktree in this fixture; its `.git` is a real
    // directory, so the index lock can be planted directly.
    let feature_wt = repo.add_feature();

    let target_before = repo.git_output(&["rev-parse", "main"]);
    let index_lock = repo.root_path().join(".git/index.lock");
    std::fs::write(&index_lock, "").unwrap();

    let output = repo
        .wt_command()
        .args(["step", "push", "main"])
        .current_dir(&feature_wt)
        .output()
        .expect("failed to run push");

    assert!(
        !output.status.success(),
        "a push whose worktree sync fails must fail whole: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("rolled back"),
        "the error must say the ref change was rolled back: {stderr}"
    );
    assert_eq!(
        repo.git_output(&["rev-parse", "main"]),
        target_before,
        "the ref update must be rolled back"
    );

    let _ = std::fs::remove_file(&index_lock);
}

/// A registered target worktree whose directory is gone stops both strategies.
///
/// Nothing can be synced into a missing directory — git dies trying to cd
/// into it — so `MergeContext::prepare` refuses upfront with the ref
/// untouched, naming the `git worktree prune` that clears the registration.
#[rstest]
fn test_push_target_worktree_missing(mut repo: TestRepo) {
    let main_wt = repo.add_main_worktree();
    let feature_wt =
        repo.add_worktree_with_commit("feature", "test.txt", "test content", "Add test file");
    let main_before = main_sha(&repo);

    std::fs::remove_dir_all(&main_wt).unwrap();

    snapshot_push(
        "push_target_worktree_missing",
        &repo,
        &["main"],
        Some(&feature_wt),
    );
    snapshot_push(
        "push_target_worktree_missing_no_ff",
        &repo,
        &["--no-ff", "main"],
        Some(&feature_wt),
    );
    assert_eq!(
        main_sha(&repo),
        main_before,
        "neither strategy may move the ref it cannot sync"
    );
}

/// The commit `main` resolves to, read from the repo root.
fn main_sha(repo: &TestRepo) -> String {
    let output = repo
        .git_command()
        .current_dir(repo.root_path())
        .args(["rev-parse", "main"])
        .run()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A push target can be named by the worktree it is checked out in, the same as
/// a rebase or diff target — `require_target_branch`'s path fallback, where
/// `require_target_ref`'s is covered by `wt step diff`.
#[rstest]
fn push_target_accepts_worktree_path(mut repo: TestRepo) {
    let main_wt = repo.add_main_worktree();
    let feature_wt =
        repo.add_worktree_with_commit("feature", "test.txt", "test content", "Add test file");

    let output = repo
        .wt_command()
        .current_dir(&feature_wt)
        .args(["step", "push", main_wt.to_str().unwrap()])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "a worktree path should name the branch checked out there: {stderr}"
    );
    assert_eq!(
        repo.git_output(&["rev-parse", "main"]),
        repo.git_output(&["rev-parse", "feature"]),
        "main should have fast-forwarded to feature"
    );
}
