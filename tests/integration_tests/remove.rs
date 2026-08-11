use crate::common::{
    BareRepoTest, SLEEP_FOR_ABSENCE_CHECK, TestRepo, TestRepoBase, configure_directive_files,
    directive_files, make_snapshot_cmd, repo, repo_with_remote, setup_snapshot_settings,
    setup_temp_snapshot_settings, wt_command,
};
use ansi_str::AnsiStr;
use insta::assert_snapshot;
use insta_cmd::assert_cmd_snapshot;
use path_slash::PathExt as _;
use rstest::rstest;

#[rstest]
fn test_remove_from_worktree(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature-wt");

    // Run remove from within the worktree
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &[],
        Some(&worktree_path)
    ));
}

// `--reap` (experimental) with no processes running under the worktree: the
// reap phase reports it found nothing, then removal proceeds normally. A fresh
// worktree has no processes with a cwd under it, so this is deterministic
// (and identical whether or not `lsof` is installed on the runner).
#[cfg(unix)]
#[rstest]
fn test_remove_reap_no_processes(mut repo: TestRepo) {
    repo.add_worktree("feature-reap");

    // Remove by name from the primary worktree so the reap scans the removed
    // worktree's path, not the current one.
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["--reap", "feature-reap"],
        None
    ));
}

// `--reap` (experimental) with a real process running under the worktree: the
// detached child is discovered and terminated before removal. Whether the
// controlling-terminal guard reaps it depends on the test's own terminal
// (none in CI → reaped; a TTY on a dev box → spared), so the assertion
// branches on that terminal — read directly from the session (`/dev/tty`),
// the property the child inherits at spawn, not probed via a second
// `lsof`/`ps` snapshot, which can transiently fail under suite load and
// predict the branch `wt`'s own probe then contradicts.
#[cfg(unix)]
#[rstest]
fn test_remove_reap_kills_process(mut repo: TestRepo) {
    use crate::common::{wait_for, wait_for_worktree_removed};
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use worktrunk::git::reap;

    let worktree_path = repo.add_worktree("feature-reapkill");
    let canonical = std::fs::canonicalize(&worktree_path).unwrap();

    // A detached child whose cwd is the worktree — the shape `--reap` targets.
    let mut child = Command::new("sleep")
        .arg("60")
        .current_dir(&canonical)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .unwrap();
    let pid = child.id();

    // Wait until lsof reports the child's cwd — fast when idle, but under
    // suite load a single probe can burn its whole 5s in-process timeout;
    // `wait_for` carries the suite's generous presence-poll deadline.
    wait_for(
        &format!("child {pid} in cwd discovery — is lsof installed and able to read process cwds?"),
        || {
            reap::processes_under(&canonical)
                .iter()
                .any(|p| p.pid == pid)
        },
    );

    // The guard reaps the child iff it holds no controlling terminal — which
    // it inherited from this process's session, so read the session directly:
    // `/dev/tty` opens iff the session has a controlling terminal.
    let will_reap = std::fs::File::open("/dev/tty").is_err();

    let run_remove = |repo: &TestRepo| {
        let output = repo
            .wt_command()
            .args(["remove", "--reap", "feature-reapkill"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stderr).into_owned()
    };

    if will_reap {
        // `sleep` is a child of *this* test process, so once `wt` signals it,
        // its parent (us) must `wait()` to reap the zombie — otherwise it
        // lingers and `wt`'s liveness check (`kill(pid, 0)`) still sees it.
        // Real reap targets aren't `wt`'s children, so they simply vanish. A
        // thread already blocked in `wait()` reaps it the instant it exits.
        let reaper = std::thread::spawn(move || child.wait().unwrap());
        let stderr = run_remove(&repo);
        let status = reaper.join().unwrap();

        assert!(
            stderr.contains("Reaping 1 process under") && stderr.contains("Reaped 1 process"),
            "expected reap output, got:\n{stderr}"
        );
        // Terminated by a signal (SIGTERM, or SIGKILL if it ignored SIGTERM).
        assert!(!status.success());
    } else {
        let stderr = run_remove(&repo);
        assert!(
            stderr.contains("No processes to reap"),
            "expected no-reap output, got:\n{stderr}"
        );
        let _ = child.kill();
        let _ = child.wait();
    }

    // The reap phase prints before the removal runs, so neither branch's
    // output proves the removal itself completed.
    wait_for_worktree_removed(&worktree_path);
}

// `--reap` (experimental) with a process *holding* a controlling terminal
// under the worktree: the guard spares it. Unlike
// `test_remove_reap_kills_process` this doesn't branch on the suite's own
// terminal — the child gets a fresh PTY as its controlling terminal at spawn
// (portable_pty runs it as a session leader on the slave side), so the spared
// branch runs deterministically in every environment, CI included.
//
// From the outside a spared process is indistinguishable from an undiscovered
// one (both print "No processes to reap"), so the test first proves discovery
// sees the child, then pins the guard's verdict in-process. The load-bearing
// assertion is the child surviving removal — the data-safety contract — which
// a guard regression fails whenever discovery succeeds.
#[cfg(unix)]
#[rstest]
fn test_remove_reap_spares_terminal_process(mut repo: TestRepo) {
    use crate::common::{open_pty_with_size, wait_for, wait_for_worktree_removed};
    use portable_pty::CommandBuilder;
    use worktrunk::git::reap;

    let worktree_path = repo.add_worktree("feature-reapspare");
    let canonical = std::fs::canonicalize(&worktree_path).unwrap();

    // The "keep-me" shape: a terminal-holding process cwd'd in the worktree.
    // `sleep` writes nothing, so the never-read master can't fill and block it;
    // 600s bounds the leak if an assertion panics before cleanup.
    let pty = open_pty_with_size(24, 80);
    let mut cmd = CommandBuilder::new("sleep");
    cmd.arg("600");
    cmd.cwd(&canonical);
    let mut child = pty.slave.spawn_command(cmd).unwrap();
    let pid = child.process_id().unwrap();

    // Same presence poll as the kill test: prove discovery sees the child
    // before asking what the guard makes of it.
    wait_for(
        &format!("child {pid} in cwd discovery — is lsof installed and able to read process cwds?"),
        || {
            reap::processes_under(&canonical)
                .iter()
                .any(|p| p.pid == pid)
        },
    );

    // The guard's verdict, pinned in-process: discovered but not reapable.
    // (A transiently failed `ps` probe also yields "not reapable" — the
    // fail-safe points the same way as the contract, so this can't flake.)
    assert!(
        !reap::collect_reapable(&canonical)
            .iter()
            .any(|p| p.pid == pid),
        "terminal-holding child {pid} was classified reapable"
    );

    let output = repo
        .wt_command()
        .args(["remove", "--reap", "feature-reapspare"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("No processes to reap"),
        "expected no-reap output, got:\n{stderr}"
    );
    // The reap phase prints before the removal runs, so confirm the removal
    // actually happened — otherwise the survival assertion below would hold
    // just as well for a `wt` that gave up before touching the worktree.
    assert!(output.status.success(), "remove failed; stderr:\n{stderr}");
    wait_for_worktree_removed(&worktree_path);
    assert!(
        child.try_wait().unwrap().is_none(),
        "spared child {pid} was killed during removal"
    );

    let _ = child.kill();
    let _ = child.wait();
}

#[rstest]
fn test_remove_internal_mode(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature-internal");

    // Directive file guards must live through command execution
    let (cd_path, exec_path, _guard) = directive_files();
    assert_cmd_snapshot!({
        let mut cmd = make_snapshot_cmd(&repo, "remove", &[], Some(&worktree_path));
        configure_directive_files(&mut cmd, &cd_path, &exec_path);
        cmd
    });
}

///
/// When git runs a subcommand, it sets `GIT_EXEC_PATH` in the environment.
/// Shell integration cannot work in this case because cd directives cannot
/// propagate through git's subprocess to the parent shell.
#[rstest]
fn test_remove_as_git_subcommand(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature-git-subcmd");

    // Remove with GIT_EXEC_PATH set (simulating `git wt remove ...`)
    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "remove", &[], Some(&worktree_path));
        cmd.env("GIT_EXEC_PATH", "/usr/lib/git-core");
        assert_cmd_snapshot!("remove_as_git_subcommand", cmd);
    });
}

#[rstest]
fn test_remove_locked_worktree(mut repo: TestRepo) {
    // Create a worktree and lock it
    let _worktree_path = repo.add_worktree("locked-feature");
    repo.lock_worktree("locked-feature", Some("Testing lock"));

    // Try to remove the locked worktree - should fail with helpful error
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["locked-feature"],
        None
    ));
}

#[rstest]
fn test_remove_locked_worktree_no_reason(mut repo: TestRepo) {
    // Create a worktree and lock it without a reason
    let _worktree_path = repo.add_worktree("locked-no-reason");
    repo.lock_worktree("locked-no-reason", None);

    // Try to remove - should show error without parenthesized reason
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["locked-no-reason"],
        None
    ));
}

#[rstest]
fn test_remove_locked_current_worktree(mut repo: TestRepo) {
    // Create a worktree, switch to it, and lock it
    let worktree_path = repo.add_worktree("locked-current");
    repo.lock_worktree("locked-current", Some("Do not remove"));

    // Try to remove current (locked) worktree - should fail
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &[],
        Some(&worktree_path)
    ));
}

#[rstest]
fn test_remove_locked_detached_worktree(mut repo: TestRepo) {
    // Create a worktree, detach HEAD, and lock it
    let worktree_path = repo.add_worktree("locked-detached");
    repo.detach_head_in_worktree("locked-detached");
    repo.lock_worktree("locked-detached", Some("Detached and locked"));

    // Try to remove from within the locked detached worktree - should fail
    // This exercises exact-path removal of the current locked worktree.
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &[],
        Some(&worktree_path)
    ));
}

#[rstest]
fn test_remove_locked_detached_multi(mut repo: TestRepo) {
    // Test multi-remove where current worktree (@ target) is locked and detached
    let _other_worktree = repo.add_worktree("other");
    let _locked_worktree = repo.add_worktree("locked-detached");
    repo.detach_head_in_worktree("locked-detached");
    repo.lock_worktree("locked-detached", Some("Locked detached"));

    // From the locked detached worktree, try to remove @ and other
    // The @ resolves to current (locked-detached) which is locked
    let locked_path = repo.worktree_path("locked-detached");
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["@", "other"],
        Some(locked_path)
    ));
}

/// Regression test for #3645: a locked worktree whose directory is currently
/// absent must still honor the lock on removal. The missing-directory fallback
/// used to run before the lock guard, so `wt remove` pruned the registration
/// and fell through to branch deletion — bypassing the lock entirely. A lock
/// says "don't remove this", and a temporarily-unreachable directory (removable
/// media, a network mount, a dropped VPN) is exactly the case it exists for.
#[rstest]
fn test_remove_locked_worktree_directory_missing(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("locked-missing");
    repo.lock_worktree("locked-missing", Some("Detachable media"));

    // Simulate the directory becoming unreachable (unmounted media, etc.).
    std::fs::remove_dir_all(&worktree_path).expect("Failed to remove worktree directory");

    // `wt remove` must refuse: the lock guards the branch and registration even
    // though the directory is currently absent.
    let output = repo
        .wt_command()
        .args(["remove", "locked-missing"])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "wt remove should fail on a locked worktree even when its directory is missing.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // The branch must survive.
    let branch_exists = repo
        .git_command()
        .args(["branch", "--list", "locked-missing"])
        .run()
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&branch_exists.stdout)
            .trim()
            .is_empty(),
        "Branch should NOT be deleted for a locked worktree",
    );

    // The (stale) registration must NOT be pruned — the lock still guards it.
    let list_after = repo
        .git_command()
        .args(["worktree", "list", "--porcelain"])
        .run()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&list_after.stdout).contains("locked-missing"),
        "Locked worktree registration should NOT be pruned",
    );
}

#[rstest]
fn test_remove_by_name_from_main(mut repo: TestRepo) {
    // Create a worktree
    let _worktree_path = repo.add_worktree("feature-a");

    // Remove it by name from main repo
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "remove", &["feature-a"], None));
}

#[rstest]
fn test_remove_by_name_from_other_worktree(mut repo: TestRepo) {
    // Create two worktrees
    let worktree_a = repo.add_worktree("feature-a");
    let _worktree_b = repo.add_worktree("feature-b");

    // From worktree A, remove worktree B by name
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["feature-b"],
        Some(&worktree_a)
    ));
}

#[rstest]
fn test_remove_current_by_name(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature-current");

    // Remove current worktree by specifying its name
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["feature-current"],
        Some(&worktree_path)
    ));
}

///
/// Regression test for bug where `wt remove npm` would show "Cannot create worktree for npm"
/// when the expected path was occupied. Resolution skips the path occupation check entirely,
/// correctly treating this as a branch-only removal.
///
/// Setup:
/// - Branch `npm` exists but has no worktree
/// - The expected path for `npm` (repo.npm) is occupied by a different branch's worktree
///
/// Expected behavior:
/// - Warning: "No worktree found for branch npm"
/// - Success: Branch deleted (same commit as main)
#[rstest]
fn test_remove_branch_no_worktree_path_occupied(mut repo: TestRepo) {
    // Create branch `npm` without a worktree
    repo.git_command().args(["branch", "npm"]).run().unwrap();

    // Create a worktree for a different branch at the path where `npm` worktree would be
    // (the path template puts worktrees at ../repo.branch, so ../repo.npm would be npm's path)
    let _other_worktree = repo.add_worktree("other");

    // Manually move the worktree to occupy npm's expected path
    // First, get the expected path for npm
    let npm_expected_path = repo.root_path().parent().unwrap().join(format!(
        "{}.npm",
        repo.root_path().file_name().unwrap().to_str().unwrap()
    ));
    let other_path = repo.root_path().parent().unwrap().join(format!(
        "{}.other",
        repo.root_path().file_name().unwrap().to_str().unwrap()
    ));

    // Remove the worktree metadata and move the directory
    repo.git_command()
        .args([
            "worktree",
            "remove",
            "--force",
            other_path.to_str().unwrap(),
        ])
        .run()
        .unwrap();

    // Create worktree at npm's expected path but for the "other" branch
    repo.git_command()
        .args([
            "worktree",
            "add",
            npm_expected_path.to_str().unwrap(),
            "other",
        ])
        .run()
        .unwrap();

    // Now: branch `npm` exists, no worktree for it, but npm's expected path has `other` branch
    // Running `wt remove npm` should show "No worktree found" NOT "Cannot create worktree"
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "remove", &["npm"], None));
}

#[rstest]
fn test_remove_multiple_nonexistent_force(repo: TestRepo) {
    // Try to force-remove multiple branches that don't exist
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["-D", "foo", "bar", "baz"],
        None
    ));
}

#[rstest]
fn test_remove_remote_only_branch(#[from(repo_with_remote)] repo: TestRepo) {
    // Create a remote-only branch by pushing a branch then deleting it locally
    repo.run_git(&["branch", "remote-feature"]);
    repo.run_git(&["push", "origin", "remote-feature"]);
    repo.run_git(&["branch", "-D", "remote-feature"]);
    repo.run_git(&["fetch", "origin"]);

    // Try to remove a branch that only exists on remote - should get helpful error
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["remote-feature"],
        None
    ));
}

#[rstest]
fn test_remove_nonexistent_branch(repo: TestRepo) {
    // Try to remove a branch that doesn't exist at all
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "remove", &["nonexistent"], None));
}

/// A directory holding no worktree — a skeleton left behind by an interrupted
/// create — is reported as the directory it is. Resolution falls through to a
/// branch name, and reporting that would send the user to a branch listing the
/// path could never appear in.
///
/// Spelled `../<sibling>`, which is both worktrunk's own layout and the shape
/// that resolves against the cwd rather than against `-C`; the absolute form is
/// covered at the unit boundary.
#[rstest]
fn test_remove_path_holding_no_worktree(repo: TestRepo) {
    let leftover = repo.root_path().parent().unwrap().join("repo.leftover");
    std::fs::create_dir_all(&leftover).unwrap();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["../repo.leftover"],
        None
    ));
}

/// A worktree directory deleted and *recreated* is reported, not carried into
/// git's own validation failure.
///
/// It is a stale registration either way, but only the absent spelling can be
/// cleaned up here: `prune_worktree_entry` unregisters with `git worktree
/// remove`, which skips its validation only while the directory is gone. With
/// a directory sitting there git refuses — `--force` included — so the answer
/// is the message naming the repo-wide `git worktree prune` that does clear
/// it. `test_remove_pruned_worktree_directory_missing` covers the absent half,
/// which still removes without a prompt.
#[rstest]
fn test_remove_worktree_directory_recreated(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature");
    std::fs::remove_dir_all(&worktree_path).unwrap();
    std::fs::create_dir_all(&worktree_path).unwrap();

    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "remove", &["feature"], None));
}

/// A registration whose directory now holds a *different* repository is not
/// this repository's worktree, and removing it would destroy that one — its
/// uncommitted work and, for a repo that was never pushed, the only copy of
/// its objects.
///
/// `--force` is the case that matters. It is the user waiving their own
/// uncommitted changes, never a claim about who owns the directory, and it is
/// where the dirty-worktree gate stops applying — so it must not carry the
/// removal through. `git worktree remove` refuses this same removal with
/// `--force`; worktrunk's fast path renames the directory itself instead of
/// asking git to, so it has to make the check for itself.
#[rstest]
fn test_remove_refuses_foreign_repository_at_worktree_path(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature");
    let parent = worktree_path.parent().unwrap().to_path_buf();
    let dir_name = worktree_path.file_name().unwrap().to_str().unwrap();

    // Replace the worktree with an unrelated repository at the same path — the
    // registration still resolves, and the occupant's own `.git` keeps git
    // from calling it prunable.
    std::fs::remove_dir_all(&worktree_path).unwrap();
    repo.run_git_in(&parent, &["init", "-b", "main", dir_name]);
    std::fs::write(worktree_path.join("precious.txt"), "unpushed work").unwrap();

    let output = repo
        .wt_command()
        .args(["remove", "--force", "feature", "--yes"])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "wt remove --force must refuse a foreign repository at a registered path.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    // The refusal is worth nothing if the directory went anyway: removal
    // stages by rename and deletes in a detached process, so the exit code
    // alone would not catch a staged-then-deleted tree.
    assert!(
        worktree_path.join("precious.txt").exists(),
        "the foreign repository's uncommitted file must survive",
    );
    assert!(
        worktree_path.join(".git").is_dir(),
        "the foreign repository's object store must survive",
    );
}

#[rstest]
fn test_remove_partial_success(mut repo: TestRepo) {
    // Create one valid worktree
    let _feature_path = repo.add_worktree("feature");

    // Try to remove both the valid worktree and a nonexistent one
    // The valid one should be removed; error for nonexistent; exit with failure
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["feature", "nonexistent"],
        None
    ));

    // Verify the valid worktree was actually removed
    let worktrees_dir = repo.root_path().parent().unwrap();
    assert!(
        !worktrees_dir.join("feature").exists(),
        "feature worktree should have been removed despite partial failure"
    );
}

#[rstest]
fn test_remove_by_name_dirty_target(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature-dirty");

    // Create a dirty file in the target worktree
    std::fs::write(worktree_path.join("dirty.txt"), "uncommitted changes").unwrap();

    // Try to remove it by name from main repo
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "remove", &["feature-dirty"], None));
}

/// --force allows removal of dirty worktrees (issue #658)
/// This test: untracked files, branch at same commit as main
#[rstest]
fn test_remove_force_with_untracked_files(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature-untracked");

    // Create an untracked file (like devbox.lock, .env, build artifacts)
    std::fs::write(worktree_path.join("devbox.lock"), "untracked content").unwrap();

    // Verify git sees it as untracked only
    let status = repo
        .git_command()
        .args(["status", "--porcelain"])
        .current_dir(&worktree_path)
        .run()
        .unwrap();
    let status_output = String::from_utf8_lossy(&status.stdout);
    assert!(
        status_output.contains("?? devbox.lock"),
        "File should be untracked"
    );

    // Remove with --force should succeed
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["--force", "feature-untracked"],
        None
    ));
}

/// --force allows removal of dirty worktrees (issue #658)
/// This test: modified tracked file, branch ahead of main (unmerged)
#[rstest]
fn test_remove_force_with_modified_files(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature-modified");

    // Add a file to the worktree and commit it first
    std::fs::write(worktree_path.join("tracked.txt"), "original content").unwrap();
    repo.git_command()
        .args(["add", "tracked.txt"])
        .current_dir(&worktree_path)
        .run()
        .unwrap();
    repo.git_command()
        .args(["commit", "-m", "Add tracked file"])
        .current_dir(&worktree_path)
        .run()
        .unwrap();

    // Now modify the tracked file
    std::fs::write(worktree_path.join("tracked.txt"), "modified content").unwrap();

    // --force passes through to git, which allows this
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["--force", "feature-modified"],
        None
    ));
}

/// --force allows removal of dirty worktrees (issue #658)
/// This test: staged (uncommitted) file, branch at same commit as main
#[rstest]
fn test_remove_force_with_staged_files(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature-staged");

    // Create and stage a new file (but don't commit)
    std::fs::write(worktree_path.join("staged.txt"), "staged content").unwrap();
    repo.git_command()
        .args(["add", "staged.txt"])
        .current_dir(&worktree_path)
        .run()
        .unwrap();

    // --force passes through to git, which allows this
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["--force", "feature-staged"],
        None
    ));
}

/// --force + -D: dirty worktree AND unmerged branch
#[rstest]
fn test_remove_force_with_force_delete(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature-dirty-unmerged");

    // Make a commit so the branch is ahead of main (unmerged)
    repo.git_command()
        .args(["commit", "--allow-empty", "-m", "feature commit"])
        .current_dir(&worktree_path)
        .run()
        .unwrap();

    // Add untracked file to make the worktree dirty
    std::fs::write(worktree_path.join("untracked.txt"), "dirty").unwrap();

    // --force (dirty worktree) + -D (force delete unmerged branch)
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["--force", "-D", "feature-dirty-unmerged"],
        None
    ));
}

/// Regression test for issue #839: untracked files not deleted on Windows.
/// Verifies the worktree directory is actually removed, not just that the command succeeds.
#[rstest]
fn test_remove_force_actually_deletes_directory_with_untracked(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature-untracked-delete");

    // Make a commit so the branch is ahead of main (unmerged)
    repo.git_command()
        .args(["commit", "--allow-empty", "-m", "feature commit"])
        .current_dir(&worktree_path)
        .run()
        .unwrap();

    // Create untracked files (the scenario from issue #839)
    std::fs::write(worktree_path.join("untracked.txt"), "untracked content").unwrap();
    std::fs::create_dir_all(worktree_path.join("untracked_dir")).unwrap();
    std::fs::write(
        worktree_path.join("untracked_dir/nested.txt"),
        "nested untracked",
    )
    .unwrap();

    // Verify worktree exists before removal
    assert!(
        worktree_path.exists(),
        "Worktree should exist before removal"
    );

    // Remove with --force -D (the flags from issue #839)
    let output = repo
        .wt_command()
        .args([
            "remove",
            "--force",
            "-D",
            "--foreground",
            "feature-untracked-delete",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "wt remove --force -D should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The critical assertion: directory must actually be gone
    assert!(
        !worktree_path.exists(),
        "Worktree directory should be deleted after `wt remove --force -D`, but it still exists"
    );

    // Verify branch is also deleted
    let branch_list = repo
        .git_command()
        .args(["branch", "--list", "feature-untracked-delete"])
        .run()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&branch_list.stdout)
            .trim()
            .is_empty(),
        "Branch should be deleted with -D flag"
    );
}

#[rstest]
fn test_remove_multiple_worktrees(mut repo: TestRepo) {
    // Create three worktrees
    let _worktree_a = repo.add_worktree("feature-a");
    let _worktree_b = repo.add_worktree("feature-b");
    let _worktree_c = repo.add_worktree("feature-c");

    // Remove all three at once from main repo
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["feature-a", "feature-b", "feature-c"],
        None
    ));
}

#[rstest]
fn test_remove_multiple_including_current(mut repo: TestRepo) {
    // Create three worktrees
    let worktree_a = repo.add_worktree("feature-a");
    let _worktree_b = repo.add_worktree("feature-b");
    let _worktree_c = repo.add_worktree("feature-c");

    // From worktree A, remove all three (including current)
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["feature-a", "feature-b", "feature-c"],
        Some(&worktree_a)
    ));
}

#[rstest]
fn test_remove_branch_not_fully_merged(mut repo: TestRepo) {
    // Create a worktree with an unmerged commit
    let worktree_path = repo.add_worktree("feature-unmerged");

    // Add a commit to the feature branch that's not in main
    std::fs::write(worktree_path.join("feature.txt"), "new feature").unwrap();
    repo.git_command()
        .args(["add", "feature.txt"])
        .current_dir(&worktree_path)
        .run()
        .unwrap();
    repo.git_command()
        .args(["commit", "-m", "Add feature"])
        .current_dir(&worktree_path)
        .run()
        .unwrap();

    // Try to remove it from the main repo
    // Branch deletion should fail but worktree removal should succeed
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["feature-unmerged"],
        None
    ));
}

#[rstest]
fn test_remove_foreground(mut repo: TestRepo) {
    // Create a worktree
    let _worktree_path = repo.add_worktree("feature-fg");

    // Remove it with --foreground flag from main repo
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["--foreground", "feature-fg"],
        None
    ));
}

/// Tests that --force-delete and --no-delete-branch are mutually exclusive
#[rstest]
fn test_remove_conflicting_branch_flags(repo: TestRepo) {
    // Try to use both --force-delete (-D) and --no-delete-branch together
    // This should fail with an error
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["-D", "--no-delete-branch", "nonexistent"],
        None
    ));
}

#[rstest]
fn test_remove_foreground_unmerged(mut repo: TestRepo) {
    // Create a worktree with an unmerged commit
    let worktree_path = repo.add_worktree("feature-unmerged-fg");

    // Add a commit to the feature branch that's not in main
    std::fs::write(worktree_path.join("feature.txt"), "new feature").unwrap();
    repo.git_command()
        .args(["add", "feature.txt"])
        .current_dir(&worktree_path)
        .run()
        .unwrap();
    repo.git_command()
        .args(["commit", "-m", "Add feature"])
        .current_dir(&worktree_path)
        .run()
        .unwrap();

    // Remove it with --foreground flag from main repo
    // Branch deletion should fail but worktree removal should succeed
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["--foreground", "feature-unmerged-fg"],
        None
    ));
}

/// Tests foreground removal with --no-delete-branch on an integrated branch.
/// The hint should show "Branch integrated (reason); retained with --no-delete-branch"
#[rstest]
fn test_remove_foreground_no_delete_branch(mut repo: TestRepo) {
    // Create a worktree (integrated - same commit as main)
    let _worktree_path = repo.add_worktree("feature-fg-keep");

    // Remove with both --foreground and --no-delete-branch
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["--foreground", "--no-delete-branch", "feature-fg-keep"],
        None
    ));
}

/// Tests foreground removal with --no-delete-branch on an unmerged branch.
/// No hint needed since the flag had no effect (branch wouldn't be deleted anyway).
#[rstest]
fn test_remove_foreground_no_delete_branch_unmerged(mut repo: TestRepo) {
    // Create a worktree with an unmerged commit
    let worktree_path = repo.add_worktree("feature-fg-unmerged-keep");

    // Add a commit to the feature branch that's not in main
    std::fs::write(worktree_path.join("feature.txt"), "new feature").unwrap();
    repo.git_command()
        .args(["add", "feature.txt"])
        .current_dir(&worktree_path)
        .run()
        .unwrap();
    repo.git_command()
        .args(["commit", "-m", "Add feature"])
        .current_dir(&worktree_path)
        .run()
        .unwrap();

    // Go back to main
    repo.git_command().args(["checkout", "main"]).run().unwrap();

    // Remove with both --foreground and --no-delete-branch
    // No hint because:
    // - Branch is unmerged (wouldn't be deleted anyway)
    // - --no-delete-branch had no effect
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &[
            "--foreground",
            "--no-delete-branch",
            "feature-fg-unmerged-keep",
        ],
        None
    ));
}

#[rstest]
fn test_remove_no_delete_branch(mut repo: TestRepo) {
    // Create a worktree (integrated - same commit as main)
    let _worktree_path = repo.add_worktree("feature-keep");

    // Remove worktree but keep the branch using --no-delete-branch flag
    // Since branch is integrated, the flag has an effect - hint explains this
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["--no-delete-branch", "feature-keep"],
        None
    ));
}

/// `[remove] delete-branch = false` in user config makes `wt remove` keep the
/// branch by default — equivalent to passing `--no-delete-branch` every time.
#[rstest]
fn test_remove_config_delete_branch_false_keeps_branch(mut repo: TestRepo) {
    repo.write_test_config(
        r#"[remove]
delete-branch = false
"#,
    );
    let _worktree_path = repo.add_worktree("feature-config-keep");

    let output = repo
        .wt_command()
        .args(["remove", "--foreground", "feature-config-keep"])
        .output()
        .expect("wt remove should run");
    assert!(
        output.status.success(),
        "wt remove failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Branch should still exist because config kept it.
    let branches = repo
        .git_command()
        .args(["branch", "--list", "feature-config-keep"])
        .run()
        .unwrap()
        .stdout;
    assert!(
        String::from_utf8_lossy(&branches).contains("feature-config-keep"),
        "branch should be retained when [remove] delete-branch = false; got: {}",
        String::from_utf8_lossy(&branches),
    );
}

/// `--delete-branch` on the command line overrides
/// `[remove] delete-branch = false` from config.
#[rstest]
fn test_remove_cli_delete_branch_overrides_config(mut repo: TestRepo) {
    repo.write_test_config(
        r#"[remove]
delete-branch = false
"#,
    );
    let _worktree_path = repo.add_worktree("feature-cli-override");

    let output = repo
        .wt_command()
        .args([
            "remove",
            "--foreground",
            "--delete-branch",
            "feature-cli-override",
        ])
        .output()
        .expect("wt remove should run");
    assert!(
        output.status.success(),
        "wt remove failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let branches = repo
        .git_command()
        .args(["branch", "--list", "feature-cli-override"])
        .run()
        .unwrap()
        .stdout;
    assert!(
        !String::from_utf8_lossy(&branches).contains("feature-cli-override"),
        "branch should be deleted when --delete-branch overrides config; got: {}",
        String::from_utf8_lossy(&branches),
    );
}

#[rstest]
fn test_remove_no_delete_branch_unmerged(mut repo: TestRepo) {
    // Create a worktree with an unmerged commit
    let worktree_path = repo.add_worktree("feature-unmerged-keep");

    // Add a commit to the feature branch that's not in main
    std::fs::write(worktree_path.join("feature.txt"), "new feature").unwrap();
    repo.git_command()
        .args(["add", "feature.txt"])
        .current_dir(&worktree_path)
        .run()
        .unwrap();
    repo.git_command()
        .args(["commit", "-m", "Add feature"])
        .current_dir(&worktree_path)
        .run()
        .unwrap();

    // Go back to main before removing
    repo.git_command().args(["checkout", "main"]).run().unwrap();

    // Remove worktree with --no-delete-branch flag
    // Since branch is unmerged, the flag has no effect - no hint shown
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["--no-delete-branch", "feature-unmerged-keep"],
        None
    ));
}

#[rstest]
fn test_remove_branch_only_merged(repo: TestRepo) {
    // Create a branch from main without a worktree (already merged)
    repo.git_command()
        .args(["branch", "feature-merged"])
        .run()
        .unwrap();

    // Remove the branch (no worktree exists)
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["feature-merged"],
        None
    ));
}

#[rstest]
fn test_remove_branch_only_unmerged(repo: TestRepo) {
    // Create a branch with a unique commit (not in main)
    repo.git_command()
        .args(["branch", "feature-unmerged"])
        .run()
        .unwrap();

    // Add a commit to the branch that's not in main
    repo.git_command()
        .args(["checkout", "feature-unmerged"])
        .run()
        .unwrap();
    std::fs::write(repo.root_path().join("feature.txt"), "new feature").unwrap();
    repo.git_command()
        .args(["add", "feature.txt"])
        .run()
        .unwrap();
    repo.git_command()
        .args(["commit", "-m", "Add feature"])
        .run()
        .unwrap();
    repo.git_command().args(["checkout", "main"]).run().unwrap();

    // Try to remove the branch (no worktree exists, branch not merged)
    // Branch deletion should fail but not error
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["feature-unmerged"],
        None
    ));
}

#[rstest]
fn test_remove_branch_only_force_delete(repo: TestRepo) {
    // Create a branch with a unique commit (not in main)
    repo.git_command()
        .args(["branch", "feature-force"])
        .run()
        .unwrap();

    // Add a commit to the branch that's not in main
    repo.git_command()
        .args(["checkout", "feature-force"])
        .run()
        .unwrap();
    std::fs::write(repo.root_path().join("feature.txt"), "new feature").unwrap();
    repo.git_command()
        .args(["add", "feature.txt"])
        .run()
        .unwrap();
    repo.git_command()
        .args(["commit", "-m", "Add feature"])
        .run()
        .unwrap();
    repo.git_command().args(["checkout", "main"]).run().unwrap();

    // Force delete the branch (no worktree exists)
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["--force-delete", "feature-force"],
        None
    ));
}

///
/// When in detached HEAD, we should still be able to remove the current worktree
/// using path-based removal (no branch deletion).
#[rstest]
fn test_remove_from_detached_head_in_worktree(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature-detached");

    // Detach HEAD in the worktree
    repo.detach_head_in_worktree("feature-detached");

    // Run remove from within the detached worktree (should still work)
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &[],
        Some(&worktree_path)
    ));
}

///
/// Covers the foreground detached HEAD code path in handlers.rs.
/// The output should be "✓ Removed worktree (detached HEAD, no branch to delete)".
///
/// Ignored on Windows: subprocess tests stay in the worktree, causing file locking errors.
#[rstest]
#[cfg_attr(windows, ignore)]
fn test_remove_foreground_detached_head(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature-detached-fg");

    // Detach HEAD in the worktree
    repo.detach_head_in_worktree("feature-detached-fg");

    // Run foreground remove from within the detached worktree
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["--foreground"],
        Some(&worktree_path)
    ));
}

///
/// This should behave identically to `wt remove` (no args) - path-based removal
/// without branch deletion. The `@` symbol refers to the current worktree.
#[rstest]
fn test_remove_at_from_detached_head_in_worktree(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature-detached-at");

    // Detach HEAD in the worktree
    repo.detach_head_in_worktree("feature-detached-at");

    // Run `wt remove @` from within the detached worktree (should behave same as no args)
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["@"],
        Some(&worktree_path)
    ));
}

///
/// This simulates a squash merge workflow where:
/// - Feature branch has commits ahead of main
/// - Main is updated (e.g., via squash merge on GitHub) with the same content
/// - Branch is NOT an ancestor of main, but tree SHAs match
/// - Branch should be deleted because content is integrated
#[rstest]
fn test_remove_branch_matching_tree_content(repo: TestRepo) {
    // Create a feature branch from main
    repo.git_command()
        .args(["branch", "feature-squashed"])
        .run()
        .unwrap();

    // On feature branch: add a file
    repo.git_command()
        .args(["checkout", "feature-squashed"])
        .run()
        .unwrap();
    std::fs::write(repo.root_path().join("feature.txt"), "squash content").unwrap();
    repo.git_command()
        .args(["add", "feature.txt"])
        .run()
        .unwrap();
    repo.git_command()
        .args(["commit", "-m", "Add feature (on feature branch)"])
        .run()
        .unwrap();

    // On main: add the same file with same content (simulates squash merge result)
    repo.git_command().args(["checkout", "main"]).run().unwrap();
    std::fs::write(repo.root_path().join("feature.txt"), "squash content").unwrap();
    repo.git_command()
        .args(["add", "feature.txt"])
        .run()
        .unwrap();
    repo.git_command()
        .args(["commit", "-m", "Add feature (squash merged)"])
        .run()
        .unwrap();

    // Verify the setup: feature-squashed is NOT an ancestor of main (different commits)
    let is_ancestor = repo
        .git_command()
        .args(["merge-base", "--is-ancestor", "feature-squashed", "main"])
        .run()
        .unwrap();
    assert!(
        !is_ancestor.status.success(),
        "feature-squashed should NOT be an ancestor of main"
    );

    // Verify: tree SHAs should match
    let feature_tree = String::from_utf8(
        repo.git_command()
            .args(["rev-parse", "feature-squashed^{tree}"])
            .run()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let main_tree = String::from_utf8(
        repo.git_command()
            .args(["rev-parse", "main^{tree}"])
            .run()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(
        feature_tree.trim(),
        main_tree.trim(),
        "Tree SHAs should match (same content)"
    );

    // Remove the branch - should succeed because tree content matches main
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["feature-squashed"],
        None
    ));
}
///
/// This test documents the expected behavior:
/// 1. Linked worktrees can be removed (whether from within them or from elsewhere)
/// 2. The main worktree cannot be removed under any circumstances
/// 3. This is true regardless of which branch is checked out in the main worktree
///
/// Skipped on Windows: Tests run as subprocesses which can't change directory via shell
/// integration. Real users are fine - shell integration cds to main before removing.
/// But subprocess tests stay in the worktree, causing Windows file locking errors.
#[rstest]
#[cfg_attr(windows, ignore)]
fn test_remove_main_worktree_vs_linked_worktree(mut repo: TestRepo) {
    // Create a linked worktree
    let linked_wt_path = repo.add_worktree("feature");

    // Part 1: Verify linked worktree CAN be removed (from within it)
    // Use --foreground to ensure removal completes before creating next worktree
    assert_cmd_snapshot!(
        "remove_main_vs_linked__from_linked_succeeds",
        make_snapshot_cmd(&repo, "remove", &["--foreground"], Some(&linked_wt_path))
    );

    // Part 2: Recreate the linked worktree for the next test
    let _linked_wt_path = repo.add_worktree("feature2");

    // Part 3: Verify linked worktree CAN be removed (from main, by name)
    assert_cmd_snapshot!(
        "remove_main_vs_linked__from_main_by_name_succeeds",
        make_snapshot_cmd(&repo, "remove", &["feature2"], None)
    );

    // Part 4: Verify main worktree CANNOT be removed (from main, on default branch)
    assert_cmd_snapshot!(
        "remove_main_vs_linked__main_on_default_fails",
        make_snapshot_cmd(&repo, "remove", &[], None)
    );

    // Part 5: Create a feature branch IN the main worktree, verify STILL cannot remove
    repo.run_git(&["switch", "-c", "feature-in-main"]);

    assert_cmd_snapshot!(
        "remove_main_vs_linked__main_on_feature_fails",
        make_snapshot_cmd(&repo, "remove", &[], None)
    );

    // Part 6: Verify main worktree CANNOT be removed by name from a linked worktree
    // Switch back to main branch in main worktree, then create a new linked worktree
    repo.run_git(&["switch", "main"]);

    let linked_for_test = repo.add_worktree("test-from-linked");
    assert_cmd_snapshot!(
        "remove_main_vs_linked__main_by_name_from_linked_fails",
        make_snapshot_cmd(&repo, "remove", &["main"], Some(&linked_for_test))
    );
}

/// Removing the default branch worktree should be refused — the default branch
/// is the integration target, not something to remove.
///
/// This requires a bare repo setup since you can't have a linked worktree for the default
/// branch in a normal repo (the main worktree already has it checked out).
#[test]
fn test_remove_default_branch_refused() {
    let test = BareRepoTest::new();

    // Create worktrees for main and feature branches
    let main_worktree = test.create_worktree("main", "main");
    test.commit_in(&main_worktree, "Initial commit on main");
    let feature_worktree = test.create_worktree("feature", "feature");

    let settings = setup_temp_snapshot_settings(test.temp_path());

    // Without -D: should fail
    settings.bind(|| {
        let mut cmd = test.wt_command();
        cmd.args(["remove", "--foreground", "main"])
            .current_dir(&feature_worktree);

        assert_cmd_snapshot!("remove_default_branch_refused", cmd);
    });

    // With -D: should succeed (user explicitly force-deletes)
    settings.bind(|| {
        let mut cmd = test.wt_command();
        cmd.args(["remove", "--foreground", "-D", "main"])
            .current_dir(&feature_worktree);

        assert_cmd_snapshot!("remove_default_branch_force_delete", cmd);
    });
}

/// BranchOnly path: when the default branch has no worktree (directory deleted),
/// removal should be refused without -D, and allowed with -D.
#[test]
fn test_remove_default_branch_branch_only() {
    let test = BareRepoTest::new();

    let main_worktree = test.create_worktree("main", "main");
    test.commit_in(&main_worktree, "Initial commit on main");
    let feature_worktree = test.create_worktree("feature", "feature");

    // Delete main worktree directory so it becomes a BranchOnly removal
    std::fs::remove_dir_all(&main_worktree).unwrap();

    let settings = setup_temp_snapshot_settings(test.temp_path());

    // Without -D: should be refused
    settings.bind(|| {
        let mut cmd = test.wt_command();
        cmd.args(["remove", "main"]).current_dir(&feature_worktree);

        assert_cmd_snapshot!("remove_default_branch_branch_only_refused", cmd);
    });

    // With -D: should succeed (force-delete the default branch)
    settings.bind(|| {
        let mut cmd = test.wt_command();
        cmd.args(["remove", "-D", "main"])
            .current_dir(&feature_worktree);

        assert_cmd_snapshot!("remove_default_branch_branch_only_force_delete", cmd);
    });
}

///
/// This tests the scenario:
/// 1. Create feature branch from main and make changes (file A)
/// 2. Squash-merge feature into main (main now has A via squash commit)
/// 3. Main advances with more commits (file B)
/// 4. Try to remove feature
///
/// The branch should be detected as integrated because its content (A) is
/// already in main, even though main has additional content (B).
///
/// This is detected via merge simulation: `git merge-tree --write-tree main feature`
/// produces the same tree as main, meaning merging feature would add nothing.
#[rstest]
fn test_remove_squash_merged_then_main_advanced(repo: TestRepo) {
    // Create feature branch
    repo.git_command()
        .args(["checkout", "-b", "feature-squash"])
        .run()
        .unwrap();

    // Make changes on feature branch (file A)
    std::fs::write(repo.root_path().join("feature-a.txt"), "feature content").unwrap();
    repo.git_command()
        .args(["add", "feature-a.txt"])
        .run()
        .unwrap();
    repo.git_command()
        .args(["commit", "-m", "Add feature A"])
        .run()
        .unwrap();

    // Go back to main
    repo.git_command().args(["checkout", "main"]).run().unwrap();

    // Squash merge feature into main (simulating GitHub squash merge)
    // This creates a NEW commit on main with the same content changes
    std::fs::write(repo.root_path().join("feature-a.txt"), "feature content").unwrap();
    repo.git_command()
        .args(["add", "feature-a.txt"])
        .run()
        .unwrap();
    repo.git_command()
        .args(["commit", "-m", "Add feature A (squash merged)"])
        .run()
        .unwrap();

    // Main advances with another commit (file B)
    std::fs::write(repo.root_path().join("main-b.txt"), "main content").unwrap();
    repo.git_command()
        .args(["add", "main-b.txt"])
        .run()
        .unwrap();
    repo.git_command()
        .args(["commit", "-m", "Main advances with B"])
        .run()
        .unwrap();

    // Verify setup: feature-squash is NOT an ancestor of main (squash creates different SHAs)
    let is_ancestor = repo
        .git_command()
        .args(["merge-base", "--is-ancestor", "feature-squash", "main"])
        .run()
        .unwrap();
    assert!(
        !is_ancestor.status.success(),
        "feature-squash should NOT be an ancestor of main (squash merge)"
    );

    // Verify setup: trees don't match (main has file B that feature doesn't)
    let feature_tree = String::from_utf8(
        repo.git_command()
            .args(["rev-parse", "feature-squash^{tree}"])
            .run()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let main_tree = String::from_utf8(
        repo.git_command()
            .args(["rev-parse", "main^{tree}"])
            .run()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_ne!(
        feature_tree.trim(),
        main_tree.trim(),
        "Tree SHAs should differ (main has file B that feature doesn't)"
    );

    // Remove the feature branch - should succeed because content is integrated
    // (detected via merge simulation using git merge-tree)
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["feature-squash"],
        None
    ));
}

/// Squash merge where target later modifies the SAME files (#1818).
///
/// This is the scenario from issue #1818:
///   1. Branch modifies file A
///   2. Squash-merge lands on main (file A matches branch content)
///   3. Main later modifies file A again (advancing past the squash merge)
///   4. `wt remove` should still detect integration
///
/// Previous behavior: `git merge-tree --write-tree` conflicts on file A because
/// both sides changed it, and the code conservatively treats conflicts as
/// "not integrated". The fix uses patch-id matching as a fallback.
#[rstest]
fn test_remove_squash_merged_then_same_files_modified(repo: TestRepo) {
    // Create feature branch
    repo.git_command()
        .args(["checkout", "-b", "feature-squash-conflict"])
        .run()
        .unwrap();

    // Make changes on feature branch (file A)
    std::fs::write(repo.root_path().join("feature-a.txt"), "feature content").unwrap();
    repo.git_command()
        .args(["add", "feature-a.txt"])
        .run()
        .unwrap();
    repo.git_command()
        .args(["commit", "-m", "Add feature A"])
        .run()
        .unwrap();

    // Go back to main
    repo.git_command().args(["checkout", "main"]).run().unwrap();

    // Squash merge feature into main (simulating GitHub squash merge)
    std::fs::write(repo.root_path().join("feature-a.txt"), "feature content").unwrap();
    repo.git_command()
        .args(["add", "feature-a.txt"])
        .run()
        .unwrap();
    repo.git_command()
        .args(["commit", "-m", "Add feature A (squash merged)"])
        .run()
        .unwrap();

    // Main advances by modifying the SAME file (the key difference from the previous test)
    std::fs::write(
        repo.root_path().join("feature-a.txt"),
        "feature content\nplus more changes on main",
    )
    .unwrap();
    repo.git_command()
        .args(["add", "feature-a.txt"])
        .run()
        .unwrap();
    repo.git_command()
        .args(["commit", "-m", "Main advances same file"])
        .run()
        .unwrap();

    // Verify setup: merge-tree would conflict (this is the scenario from #1818)
    let merge_tree_result = repo
        .git_command()
        .args([
            "merge-tree",
            "--write-tree",
            "main",
            "feature-squash-conflict",
        ])
        .run()
        .unwrap();
    assert!(
        !merge_tree_result.status.success(),
        "merge-tree should report conflicts (both sides modified feature-a.txt)"
    );

    // Remove the feature branch - should succeed because content is integrated
    // (detected via patch-id fallback when merge-tree conflicts)
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["feature-squash-conflict"],
        None
    ));
}

/// Simulate a GitHub squash merge: push feature to origin, squash-merge on
/// the remote side (via a temporary clone), fetch locally, then `wt remove`.
///
/// This is the exact workflow that users hit:
///   1. Create worktree, commit, push, open PR
///   2. Squash-merge on GitHub
///   3. `git fetch` locally
///   4. `wt remove <branch>` — should detect integration via origin/main
///
/// `integration_reason` ORs over local `main` and `origin/main` so the squash
/// merge on `origin/main` (ahead of local `main` after fetch) is detected.
#[rstest]
fn test_remove_squash_merged_on_remote(#[from(repo_with_remote)] repo: TestRepo) {
    let remote_path = repo.remote_path().unwrap();

    // Create a feature branch with multiple commits (realistic PR)
    repo.run_git(&["checkout", "-b", "feature-remote-squash"]);
    std::fs::write(repo.root_path().join("feature.txt"), "initial").unwrap();
    repo.run_git(&["add", "feature.txt"]);
    repo.run_git(&["commit", "-m", "Add feature file"]);
    std::fs::write(repo.root_path().join("feature.txt"), "revised").unwrap();
    repo.run_git(&["add", "feature.txt"]);
    repo.run_git(&["commit", "-m", "Revise feature"]);
    std::fs::write(repo.root_path().join("feature.txt"), "final version").unwrap();
    repo.run_git(&["add", "feature.txt"]);
    repo.run_git(&["commit", "-m", "Finalize feature"]);
    repo.run_git(&["push", "-u", "origin", "feature-remote-squash"]);

    // Go back to main locally (don't pull — local main stays behind)
    repo.run_git(&["checkout", "main"]);

    // Simulate GitHub squash merge: clone the bare remote into a temp dir,
    // squash-merge there, push back to the bare remote
    let github_sim = repo.home_path().join("github-sim");
    repo.run_git_in(
        repo.home_path(),
        &["clone", remote_path.to_str().unwrap(), "github-sim"],
    );
    // Squash merge feature into main (like GitHub's "Squash and merge" button)
    repo.run_git_in(
        &github_sim,
        &["merge", "--squash", "origin/feature-remote-squash"],
    );
    repo.run_git_in(&github_sim, &["commit", "-m", "Add feature (#1)"]);
    // Push the squash merge back to the bare remote
    repo.run_git_in(&github_sim, &["push", "origin", "main"]);

    // Fetch locally — origin/main now has the squash merge, local main does not
    repo.run_git(&["fetch", "origin"]);

    // Verify setup: local main is behind origin/main
    let local_main = repo.git_output(&["rev-parse", "main"]);
    let origin_main = repo.git_output(&["rev-parse", "origin/main"]);
    assert_ne!(
        local_main, origin_main,
        "local main should be behind origin/main"
    );

    // Remove the feature branch — should detect as integrated via origin/main
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["feature-remote-squash"],
        None
    ));
}

/// Like `test_remove_squash_merged_on_remote`, but local `main` also advances
/// with a local-only commit after the fetch. Integration should still be
/// detected via `origin/main`.
#[rstest]
fn test_remove_squash_merged_on_remote_when_local_main_diverged(
    #[from(repo_with_remote)] repo: TestRepo,
) {
    let remote_path = repo.remote_path().unwrap();

    repo.run_git(&["checkout", "-b", "feature-remote-squash-diverged"]);
    std::fs::write(repo.root_path().join("feature-diverged.txt"), "initial").unwrap();
    repo.run_git(&["add", "feature-diverged.txt"]);
    repo.run_git(&["commit", "-m", "Add diverged feature"]);
    std::fs::write(
        repo.root_path().join("feature-diverged.txt"),
        "final version",
    )
    .unwrap();
    repo.run_git(&["add", "feature-diverged.txt"]);
    repo.run_git(&["commit", "-m", "Finalize diverged feature"]);
    repo.run_git(&["push", "-u", "origin", "feature-remote-squash-diverged"]);
    repo.run_git(&["checkout", "main"]);

    // Simulate a remote squash merge.
    let github_sim = repo.home_path().join("github-sim-diverged");
    repo.run_git_in(
        repo.home_path(),
        &[
            "clone",
            remote_path.to_str().unwrap(),
            "github-sim-diverged",
        ],
    );
    repo.run_git_in(
        &github_sim,
        &["merge", "--squash", "origin/feature-remote-squash-diverged"],
    );
    repo.run_git_in(&github_sim, &["commit", "-m", "Add diverged feature (#3)"]);
    repo.run_git_in(&github_sim, &["push", "origin", "main"]);

    // Fetch the remote squash merge, then create a local-only commit on main so
    // local and upstream diverge.
    repo.run_git(&["fetch", "origin"]);
    std::fs::write(repo.root_path().join("local-only.txt"), "local only").unwrap();
    repo.run_git(&["add", "local-only.txt"]);
    repo.run_git(&["commit", "-m", "Local-only main commit"]);

    let local_main = repo.git_output(&["rev-parse", "main"]);
    let origin_main = repo.git_output(&["rev-parse", "origin/main"]);
    assert_ne!(
        local_main, origin_main,
        "local main should diverge from origin/main"
    );

    let local_behind_remote = repo
        .git_command()
        .args(["merge-base", "--is-ancestor", "main", "origin/main"])
        .run()
        .unwrap();
    assert!(
        !local_behind_remote.status.success(),
        "local main should not be an ancestor of origin/main in diverged state"
    );

    let remote_behind_local = repo
        .git_command()
        .args(["merge-base", "--is-ancestor", "origin/main", "main"])
        .run()
        .unwrap();
    assert!(
        !remote_behind_local.status.success(),
        "origin/main should not be an ancestor of local main in diverged state"
    );

    let output = make_snapshot_cmd(&repo, "remove", &["feature-remote-squash-diverged"], None)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr)
        .ansi_strip()
        .into_owned();

    assert!(
        stderr.contains("Removed branch feature-remote-squash-diverged"),
        "expected branch to be removed once origin/main contains the squash merge\nstderr:\n{stderr}",
    );
    assert!(
        stderr.contains("origin/main"),
        "expected remove output to mention origin/main as the integration target\nstderr:\n{stderr}",
    );

    let branch_still_exists = repo
        .git_command()
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            "refs/heads/feature-remote-squash-diverged",
        ])
        .run()
        .unwrap();
    assert!(
        !branch_still_exists.status.success(),
        "feature branch should be deleted after successful remove"
    );
}

/// Like `test_remove_squash_merged_on_remote`, but main advances on the
/// remote after the squash merge.
/// Tests that `MergeAddsNothing` detection works through origin/main.
#[rstest]
fn test_remove_squash_merged_on_remote_then_advanced(#[from(repo_with_remote)] repo: TestRepo) {
    let remote_path = repo.remote_path().unwrap();

    // Create a feature branch with multiple commits (realistic PR)
    repo.run_git(&["checkout", "-b", "feature-remote-squash2"]);
    std::fs::write(repo.root_path().join("feature2.txt"), "draft").unwrap();
    repo.run_git(&["add", "feature2.txt"]);
    repo.run_git(&["commit", "-m", "WIP: start feature 2"]);
    std::fs::write(repo.root_path().join("feature2.txt"), "done").unwrap();
    repo.run_git(&["add", "feature2.txt"]);
    repo.run_git(&["commit", "-m", "Complete feature 2"]);
    repo.run_git(&["push", "-u", "origin", "feature-remote-squash2"]);

    // Go back to main locally
    repo.run_git(&["checkout", "main"]);

    // Simulate GitHub: squash merge, then main advances with another commit
    let github_sim = repo.home_path().join("github-sim2");
    repo.run_git_in(
        repo.home_path(),
        &["clone", remote_path.to_str().unwrap(), "github-sim2"],
    );
    repo.run_git_in(
        &github_sim,
        &["merge", "--squash", "origin/feature-remote-squash2"],
    );
    repo.run_git_in(&github_sim, &["commit", "-m", "Add feature 2 (#2)"]);
    // Main advances with another commit after the squash merge
    std::fs::write(github_sim.join("other.txt"), "other content").unwrap();
    repo.run_git_in(&github_sim, &["add", "other.txt"]);
    repo.run_git_in(&github_sim, &["commit", "-m", "Unrelated commit"]);
    repo.run_git_in(&github_sim, &["push", "origin", "main"]);

    // Fetch locally
    repo.run_git(&["fetch", "origin"]);

    // Verify setup: local main is behind origin/main
    let local_main = repo.git_output(&["rev-parse", "main"]);
    let origin_main = repo.git_output(&["rev-parse", "origin/main"]);
    assert_ne!(
        local_main, origin_main,
        "local main should be behind origin/main"
    );

    // Remove the feature branch — should detect as integrated via origin/main
    // even though origin/main has advanced past the squash merge
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["feature-remote-squash2"],
        None
    ));
}

/// Like `test_remove_squash_merged_on_remote`, but with a **worktree** (not just
/// a branch). Tests that the worktree-removal path displays the effective target
/// (`origin/main`) rather than the local default branch when upstream is ahead.
#[rstest]
fn test_remove_worktree_squash_merged_on_remote(#[from(repo_with_remote)] mut repo: TestRepo) {
    let remote_path = repo.remote_path().unwrap().to_path_buf();

    // Create a worktree for the feature branch
    let _wt_path = repo.add_worktree("feature-wt-squash");
    let wt_path = repo.worktrees["feature-wt-squash"].clone();
    std::fs::write(wt_path.join("feature-wt.txt"), "feature content").unwrap();
    repo.run_git_in(&wt_path, &["add", "feature-wt.txt"]);
    repo.run_git_in(&wt_path, &["commit", "-m", "Add feature"]);
    repo.run_git_in(&wt_path, &["push", "-u", "origin", "feature-wt-squash"]);

    // Simulate GitHub squash merge on the remote
    let github_sim = repo.home_path().join("github-sim-wt");
    repo.run_git_in(
        repo.home_path(),
        &["clone", remote_path.to_str().unwrap(), "github-sim-wt"],
    );
    repo.run_git_in(
        &github_sim,
        &["merge", "--squash", "origin/feature-wt-squash"],
    );
    repo.run_git_in(&github_sim, &["commit", "-m", "Add feature (#1)"]);
    repo.run_git_in(&github_sim, &["push", "origin", "main"]);

    // Fetch locally — origin/main now has the squash merge, local main does not
    repo.run_git(&["fetch", "origin"]);

    // Remove the worktree — should show origin/main as the integration target
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["feature-wt-squash"],
        None
    ));
}

// ============================================================================
// Pre-Remove Hook Tests
// ============================================================================

#[rstest]
fn test_pre_remove_hook_executes(mut repo: TestRepo) {
    // Create project config with pre-remove hook
    repo.write_project_config(r#"pre-remove = "echo 'About to remove worktree'""#);
    repo.commit("Add config");

    // Pre-approve the command
    repo.write_test_approvals(
        r#"[projects."../origin"]
approved-commands = ["echo 'About to remove worktree'"]
"#,
    );

    // Create a worktree to remove
    let _worktree_path = repo.add_worktree("feature-hook");

    // Remove with --foreground to ensure synchronous execution
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["--foreground", "feature-hook"],
        None
    ));
}

#[rstest]
fn test_pre_remove_hook_template_variables(mut repo: TestRepo) {
    // Create project config with template variables
    repo.write_project_config(
        r#"pre-remove = [
    {branch = "echo 'Branch: {{ branch }}'"},
    {worktree = "echo 'Worktree: {{ worktree_path }}'"},
    {worktree_name = "echo 'Name: {{ worktree_name }}'"},
]
"#,
    );
    repo.commit("Add config with templates");

    // Pre-approve the commands (templates match what's shown in prompts)
    repo.write_test_approvals(
        r#"[projects."../origin"]
approved-commands = [
    "echo 'Branch: {{ branch }}'",
    "echo 'Worktree: {{ worktree_path }}'",
    "echo 'Name: {{ worktree_name }}'",
]
"#,
    );

    // Create a worktree to remove
    let _worktree_path = repo.add_worktree("feature-templates");

    // Remove with --foreground
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["--foreground", "feature-templates"],
        None
    ));
}

#[rstest]
fn test_pre_remove_hook_runs_in_background_mode(mut repo: TestRepo) {
    use crate::common::wait_for_file;

    // Create a marker file that the hook will create
    let marker_file = repo.root_path().join("hook-ran.txt");

    // Create project config with hook that creates a file
    repo.write_project_config(&format!(
        r#"pre-remove = "echo 'hook ran' > {}""#,
        marker_file.to_slash_lossy()
    ));
    repo.commit("Add config");

    // Pre-approve the command
    repo.write_test_config(r#"worktree-path = "../{{ repo }}.{{ branch }}""#);
    repo.write_test_approvals(&format!(
        r#"[projects."../origin"]
approved-commands = ["echo 'hook ran' > {}"]
"#,
        marker_file.to_slash_lossy()
    ));

    // Create a worktree to remove
    let _worktree_path = repo.add_worktree("feature-bg");

    // Remove in background mode (default)
    let mut cmd = repo.wt_command();
    cmd.args(["remove", "feature-bg"]).output().unwrap();

    // Wait for the hook to create the marker file
    wait_for_file(&marker_file);

    // Marker file SHOULD exist - pre-remove hooks run before background removal starts
    assert!(
        marker_file.exists(),
        "Pre-remove hook should run even in background mode"
    );
}

/// The final dirty-worktree gate holds on both execution paths.
///
/// Planning validates cleanliness before `pre-remove` runs, so a hook that
/// dirties the worktree can only be caught by the gate immediately before the
/// mutation — `stage_worktree_removal`'s, the one every path shares. The
/// background case is the load-bearing one: it's the default for `wt remove`,
/// and it stages the worktree by renaming it out from under the user, so a
/// missing gate there destroys the hook's output rather than refusing.
#[rstest]
#[case::foreground(&["--foreground"])]
#[case::background(&[])]
fn test_pre_remove_hook_dirtying_worktree_blocks_remove(
    mut repo: TestRepo,
    #[case] execution_args: &[&str],
) {
    let hook = "echo dirty > hook-created.txt";
    repo.write_project_config(&format!(r#"pre-remove = "{hook}""#));
    repo.commit("Add config");
    repo.write_test_approvals(&format!(
        r#"[projects."../origin"]
approved-commands = ["{hook}"]
"#
    ));

    let worktree_path = repo.add_worktree("feature-hook-dirties");
    let output = repo
        .wt_command()
        .arg("remove")
        .args(execution_args)
        .arg("feature-hook-dirties")
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "remove should fail after pre-remove dirties the worktree; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        worktree_path.exists(),
        "worktree must be preserved when the post-hook clean check fails"
    );
    assert!(
        worktree_path.join("hook-created.txt").exists(),
        "hook-created file should remain recoverable in the worktree"
    );
}

#[rstest]
fn test_pre_remove_hook_new_commit_retains_branch_in_background_remove(mut repo: TestRepo) {
    use crate::common::wait_for_worktree_removed;

    let hook = "echo hook > late-commit.txt && git add late-commit.txt && git commit -m late-hook";
    repo.write_project_config(&format!(r#"pre-remove = "{hook}""#));
    repo.commit("Add config");
    repo.write_test_approvals(&format!(
        r#"[projects."../origin"]
approved-commands = ["{hook}"]
"#
    ));

    let worktree_path = repo.add_worktree("feature-hook-commit");
    let output = repo
        .wt_command()
        .args(["remove", "feature-hook-commit"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "remove should keep the worktree cleanup path successful; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    wait_for_worktree_removed(&worktree_path);

    let branch = repo
        .git_command()
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            "refs/heads/feature-hook-commit",
        ])
        .run()
        .unwrap();
    assert!(
        branch.status.success(),
        "branch must be retained after a pre-remove hook adds an unintegrated commit"
    );
}

/// `pre-remove` resolves `.config/wt.toml` from the invoking worktree — the one
/// `wt remove` ran in — while its template context (`{{ branch }}` etc.) is the
/// removed worktree's. The removed worktree's own config is not consulted.
#[rstest]
fn test_pre_remove_hook_reads_invoking_worktree_config(mut repo: TestRepo) {
    use crate::common::wait_for_file_content;

    let worktree_path = repo.add_worktree("feature-local-hook");
    let marker_file = repo.root_path().join("pre-remove-ran.txt");
    let wrong_marker = repo.root_path().join("wrong-marker.txt");

    // A competing hook in the removed worktree, which must be ignored.
    std::fs::create_dir_all(worktree_path.join(".config")).unwrap();
    std::fs::write(
        worktree_path.join(".config/wt.toml"),
        format!(
            r#"pre-remove = "echo wrong > {}""#,
            wrong_marker.to_slash_lossy()
        ),
    )
    .unwrap();

    // The hook that should run lives in the invoking worktree (primary, cwd).
    // `{{ branch }}` proves it ran with the removed worktree's template context.
    repo.write_project_config(&format!(
        r#"pre-remove = "echo 'removed branch {{{{ branch }}}}' > {}""#,
        marker_file.to_slash_lossy()
    ));

    // `--force` because the removed worktree has an untracked `.config/wt.toml`;
    // `--yes` to skip the approval prompt.
    let output = repo
        .wt_command()
        .args([
            "remove",
            "--foreground",
            "--force",
            "--yes",
            "feature-local-hook",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "wt remove failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    wait_for_file_content(&marker_file);
    assert_eq!(
        std::fs::read_to_string(&marker_file).unwrap().trim(),
        "removed branch feature-local-hook",
        "pre-remove runs from the invoking worktree's config, with the removed worktree's branch"
    );
    assert!(
        !wrong_marker.exists(),
        "the removed worktree's own config must not be consulted"
    );
}

/// `post-remove` reads the invoking worktree's `.config/wt.toml`, snapshotted at
/// the gate — its template context (`{{ branch }}` etc.) is the removed
/// worktree's, gone by the time the hook runs. The removed worktree's own
/// config is not consulted.
#[rstest]
fn test_post_remove_hook_reads_invoking_worktree_config(mut repo: TestRepo) {
    use crate::common::wait_for_file_content;

    let worktree_path = repo.add_worktree("feature-local-hook");
    let marker_file = repo.root_path().join("post-remove-ran.txt");
    let wrong_marker = repo.root_path().join("wrong-marker.txt");

    // A competing hook in the removed worktree, which must be ignored.
    std::fs::create_dir_all(worktree_path.join(".config")).unwrap();
    std::fs::write(
        worktree_path.join(".config/wt.toml"),
        format!(
            r#"post-remove = "echo wrong > {}""#,
            wrong_marker.to_slash_lossy()
        ),
    )
    .unwrap();

    // The hook that should run lives in the invoking worktree (primary, cwd).
    // `{{ branch }}` proves it ran with the removed worktree's template context;
    // the marker outside the removed worktree survives the removal.
    repo.write_project_config(&format!(
        r#"post-remove = "echo 'post-remove of {{{{ branch }}}}' > {}""#,
        marker_file.to_slash_lossy()
    ));

    let output = repo
        .wt_command()
        .args([
            "remove",
            "--foreground",
            "--force",
            "--yes",
            "feature-local-hook",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "wt remove failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    wait_for_file_content(&marker_file);
    assert_eq!(
        std::fs::read_to_string(&marker_file).unwrap().trim(),
        "post-remove of feature-local-hook",
        "post-remove runs from the invoking worktree's config, snapshotted before removal"
    );
    assert!(
        !worktree_path.exists(),
        "feature worktree should be removed"
    );
    assert!(
        !wrong_marker.exists(),
        "the removed worktree's own config must not be consulted"
    );
}

/// A malformed `.config/wt.toml` in the invoking worktree makes `wt remove`
/// abort with the parse error in stderr — no silent fall-through to a different
/// config, and the worktree stays on disk so the user can fix it.
#[rstest]
fn test_remove_aborts_on_malformed_invoking_config(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature-x");
    // Malformed config in the invoking worktree (primary, cwd).
    repo.write_project_config("this is not [ valid toml");

    let output = repo
        .wt_command()
        .args(["remove", "--foreground", "--force", "--yes", "feature-x"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "wt remove should abort on a malformed invoking-worktree config; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("wt.toml"),
        "error should name the offending config file; stderr:\n{stderr}"
    );
    assert!(
        worktree_path.exists(),
        "worktree should stay on disk so the user can fix the broken config"
    );
}

/// Removing a worktree with no project hooks must not touch approval state.
/// A malformed `approvals.toml` aborts only when there is a project command to
/// authorize; with an empty plan the gate never loads approvals (regression:
/// the plan rewrite briefly loaded `Approvals` unconditionally).
#[rstest]
fn test_remove_no_project_hooks_ignores_malformed_approvals(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature-no-hooks");

    // No `.config/wt.toml` anywhere ⇒ the hook plan is empty. A broken
    // approvals file would only matter if there were a command to check.
    repo.write_test_approvals("this is not = = valid toml");

    let output = repo
        .wt_command()
        .args([
            "remove",
            "--foreground",
            "--force",
            "--yes",
            "feature-no-hooks",
        ])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "remove with no project hooks must not parse approvals.toml; stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("Failed to parse approvals"),
        "approvals must not be loaded for an empty plan; stderr:\n{stderr}"
    );
    assert!(
        !worktree_path.exists(),
        "feature worktree should be removed"
    );
}

#[rstest]
fn test_pre_remove_hook_failure_aborts(mut repo: TestRepo) {
    // Create project config with failing hook
    repo.write_project_config(r#"pre-remove = "exit 1""#);
    repo.commit("Add config");

    // Pre-approve the command
    repo.write_test_approvals(
        r#"[projects."../origin"]
approved-commands = ["exit 1"]
"#,
    );

    // Create a worktree to remove
    let worktree_path = repo.add_worktree("feature-fail");

    // Remove - should FAIL due to hook failure
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["--foreground", "feature-fail"],
        None
    ));

    // Verify worktree was NOT removed (hook failure aborted removal)
    assert!(
        worktree_path.exists(),
        "Worktree should NOT be removed when hook fails"
    );
}

/// Pre-remove hook failure should NOT write cd directive.
/// Bug: cd directive was written before pre-remove hooks ran, so if hooks failed,
/// the shell would still cd to main_path even though the worktree wasn't removed.
#[rstest]
fn test_pre_remove_hook_failure_no_cd_directive(mut repo: TestRepo) {
    // Create project config with failing hook
    repo.write_project_config(r#"pre-remove = "exit 1""#);
    repo.commit("Add config");

    // Pre-approve the command
    repo.write_test_approvals(
        r#"[projects."../origin"]
approved-commands = ["exit 1"]
"#,
    );

    // Create a worktree to remove
    let worktree_path = repo.add_worktree("feature-cd-test");

    // Set up directive files
    let (cd_path, exec_path, _guard) = directive_files();

    // Run remove from within the worktree (which would trigger cd to main if it worked)
    let mut cmd = repo.wt_command();
    cmd.args(["remove", "--foreground"]);
    cmd.current_dir(&worktree_path);
    configure_directive_files(&mut cmd, &cd_path, &exec_path);
    let output = cmd.output().unwrap();

    // Command should have failed (hook failure)
    assert!(
        !output.status.success(),
        "Remove should fail when pre-remove hook fails"
    );

    // CD file should be empty (no path written when hook fails)
    let cd_content = std::fs::read_to_string(&cd_path).unwrap_or_default();
    assert!(
        cd_content.trim().is_empty(),
        "CD file should be empty when hook fails, got: {}",
        cd_content
    );

    // Worktree should still exist
    assert!(
        worktree_path.exists(),
        "Worktree should NOT be removed when hook fails"
    );
}

#[rstest]
fn test_pre_remove_hook_not_for_branch_only(repo: TestRepo) {
    // Create a marker file that the hook would create
    let marker_file = repo.root_path().join("branch-only-hook.txt");

    // Create project config with hook
    repo.write_project_config(&format!(
        r#"pre-remove = "echo 'hook ran' > {}""#,
        marker_file.to_slash_lossy()
    ));
    repo.commit("Add config");

    // Pre-approve the command
    repo.write_test_config(r#"worktree-path = "../{{ repo }}.{{ branch }}""#);
    repo.write_test_approvals(&format!(
        r#"[projects."../origin"]
approved-commands = ["echo 'hook ran' > {}"]
"#,
        marker_file.to_slash_lossy()
    ));

    // Create a branch without a worktree
    repo.git_command()
        .args(["branch", "branch-only"])
        .run()
        .unwrap();

    // Remove the branch (no worktree)
    let mut cmd = repo.wt_command();
    cmd.args(["remove", "branch-only"]).output().unwrap();

    // Marker file should NOT exist - pre-remove hooks only run for worktree removal
    assert!(
        !marker_file.exists(),
        "Pre-remove hook should NOT run for branch-only removal"
    );
}

#[rstest]
fn test_pre_remove_hook_skipped_with_no_hooks(mut repo: TestRepo) {
    use std::thread;

    // Create a marker file that the hook would create
    let marker_file = repo.root_path().join("should-not-exist.txt");

    // Create project config with hook that creates a file
    repo.write_project_config(&format!(
        r#"pre-remove = "echo 'hook ran' > {}""#,
        marker_file.to_slash_lossy()
    ));
    repo.commit("Add config");

    // Pre-approve the command (even though it shouldn't run)
    repo.write_test_config(r#"worktree-path = "../{{ repo }}.{{ branch }}""#);
    repo.write_test_approvals(&format!(
        r#"[projects."../origin"]
approved-commands = ["echo 'hook ran' > {}"]
"#,
        marker_file.to_slash_lossy()
    ));

    // Create a worktree to remove
    let worktree_path = repo.add_worktree("feature-skip");

    // Remove with --no-hooks to skip hooks
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["--foreground", "--no-hooks", "feature-skip"],
        None
    ));

    // Give a wrongly-spawned hook time to create its marker before asserting it didn't.
    thread::sleep(SLEEP_FOR_ABSENCE_CHECK);

    // Marker file should NOT exist - --no-hooks skips the hook
    assert!(
        !marker_file.exists(),
        "Pre-remove hook should NOT run with --no-hooks"
    );

    // Worktree should be removed (removal itself succeeds)
    assert!(
        !worktree_path.exists(),
        "Worktree should be removed even with --no-hooks"
    );
}

#[rstest]
fn test_remove_no_verify_deprecated_still_works(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature-no-verify");

    // --no-verify is a deprecated alias for --no-hooks: still works, still
    // emits the shared deprecation warning that points at --no-hooks.
    let output = repo
        .wt_command()
        .args([
            "remove",
            "--foreground",
            "--yes",
            "--no-verify",
            "feature-no-verify",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--no-verify is deprecated"),
        "Expected deprecation warning in stderr: {stderr}"
    );
    assert!(
        stderr.contains("--no-hooks"),
        "Expected --no-hooks suggestion in stderr: {stderr}"
    );
    assert!(
        !worktree_path.exists(),
        "Worktree should be removed with --no-verify"
    );
}

///
/// Even when a worktree is in detached HEAD state (no branch), the pre-remove
/// hook should still execute.
///
/// Skipped on Windows: Tests run as subprocesses which can't change directory via shell
/// integration. Real users are fine - shell integration cds to main before removing.
/// But subprocess tests stay in the worktree, causing Windows file locking errors.
#[rstest]
#[cfg_attr(windows, ignore)]
fn test_pre_remove_hook_runs_for_detached_head(mut repo: TestRepo) {
    // Create marker file path in the repo root
    // Use short filename to avoid terminal line-wrapping differences between platforms
    // (macOS temp paths are ~60 chars vs Linux ~20 chars, affecting wrap points)
    let marker_file = repo.root_path().join("m.txt");
    let marker_path = marker_file.to_slash_lossy();

    // Create project config with pre-remove hook that creates a marker file
    repo.write_project_config(&format!(r#"pre-remove = "touch {marker_path}""#,));
    repo.commit("Add config");

    // Pre-approve the command
    repo.write_test_config(r#"worktree-path = "../{{ repo }}.{{ branch }}""#);
    repo.write_test_approvals(&format!(
        r#"[projects."../origin"]
approved-commands = ["touch {marker_path}"]
"#,
    ));

    // Create a worktree and detach HEAD
    let worktree_path = repo.add_worktree("feature-detached-hook");
    repo.detach_head_in_worktree("feature-detached-hook");

    // Remove with --foreground to ensure synchronous execution
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["--foreground"],
        Some(&worktree_path)
    ));

    // Marker file should exist - hook ran
    assert!(
        marker_file.exists(),
        "Pre-remove hook should run for detached HEAD worktrees"
    );
}

///
/// This complements `test_pre_remove_hook_runs_for_detached_head` by verifying
/// the hook also runs when removal happens in background (the default).
#[rstest]
fn test_pre_remove_hook_runs_for_detached_head_background(mut repo: TestRepo) {
    // Create marker file path in the repo root
    let marker_file = repo.root_path().join("detached-bg-hook-marker.txt");

    // Create project config with pre-remove hook that creates a marker file
    let marker_path = marker_file.to_slash_lossy();
    repo.write_project_config(&format!(r#"pre-remove = "touch {marker_path}""#,));
    repo.commit("Add config");

    // Pre-approve the commands
    repo.write_test_config(r#"worktree-path = "../{{ repo }}.{{ branch }}""#);
    repo.write_test_approvals(&format!(
        r#"[projects."../origin"]
approved-commands = ["touch {marker_path}"]
"#,
    ));

    // Create a worktree and detach HEAD
    let worktree_path = repo.add_worktree("feature-detached-bg");
    repo.detach_head_in_worktree("feature-detached-bg");

    // Remove in background mode (default)
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &[],
        Some(&worktree_path)
    ));

    // Marker file should exist - hook ran before background spawn
    assert!(
        marker_file.exists(),
        "Pre-remove hook should run for detached HEAD worktrees in background mode"
    );
}

///
/// This is a non-snapshot test to avoid cross-platform line-wrapping differences
/// (macOS temp paths are ~60 chars vs Linux ~20 chars). The snapshot version
/// of this test (`test_pre_remove_hook_runs_for_detached_head`) verifies the hook runs;
/// this test verifies the specific template expansion behavior.
///
/// Skipped on Windows: Tests run as subprocesses which can't change directory via shell
/// integration. Real users are fine - shell integration cds to main before removing.
/// But subprocess tests stay in the worktree, causing Windows file locking errors.
#[rstest]
#[cfg_attr(windows, ignore)]
fn test_pre_remove_hook_branch_expansion_detached_head(mut repo: TestRepo) {
    // Create a file where the hook will write the branch template expansion
    let branch_file = repo.root_path().join("branch-expansion.txt");
    let branch_path = branch_file.to_slash_lossy();

    // Create project config with hook that writes {{ branch }} to file
    repo.write_project_config(&format!(
        r#"pre-remove = "echo 'branch={{{{ branch }}}}' > {branch_path}""#,
    ));
    repo.commit("Add config");

    // Pre-approve the command
    repo.write_test_config(r#"worktree-path = "../{{ repo }}.{{ branch }}""#);
    repo.write_test_approvals(&format!(
        r#"[projects."../origin"]
approved-commands = ["echo 'branch={{{{ branch }}}}' > {branch_path}"]
"#,
    ));

    // Create a worktree and detach HEAD
    let worktree_path = repo.add_worktree("feature-branch-test");
    repo.detach_head_in_worktree("feature-branch-test");

    // Run wt remove (not a snapshot test - just verify behavior)
    let output = wt_command()
        .args(["remove", "--foreground"])
        .current_dir(&worktree_path)
        .env("WORKTRUNK_CONFIG_PATH", repo.test_config_path())
        .env("WORKTRUNK_APPROVALS_PATH", repo.test_approvals_path())
        .output()
        .expect("Failed to execute wt remove");

    assert!(
        output.status.success(),
        "wt remove should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify {{ branch }} expanded to "HEAD" (fallback for detached HEAD state)
    let content =
        std::fs::read_to_string(&branch_file).expect("Hook should have created the branch file");
    assert_eq!(
        content.trim(),
        "branch=HEAD",
        "{{ branch }} should expand to 'HEAD' for detached HEAD worktrees"
    );
}

///
/// When a worktree is created at a path that doesn't match the config template,
/// `wt remove` proceeds with no mismatch notice (the state is informational,
/// surfaced only by the `wt list` glyph).
#[rstest]
fn test_remove_path_mismatch(repo: TestRepo) {
    // Create a worktree at a non-standard path using raw git
    // (wt switch --create would put it at the expected path)
    let unexpected_path = repo
        .root_path()
        .parent()
        .unwrap()
        .join("weird-path-for-feature");

    repo.git_command()
        .args([
            "worktree",
            "add",
            unexpected_path.to_str().unwrap(),
            "-b",
            "feature",
        ])
        .run()
        .unwrap();

    // Remove the worktree - no mismatch notice
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "remove", &["feature"], None));
}

#[rstest]
fn test_remove_path_mismatch_foreground(repo: TestRepo) {
    // Create a worktree at a non-standard path using raw git
    let unexpected_path = repo
        .root_path()
        .parent()
        .unwrap()
        .join("another-weird-path");

    repo.git_command()
        .args([
            "worktree",
            "add",
            unexpected_path.to_str().unwrap(),
            "-b",
            "feature-fg",
        ])
        .run()
        .unwrap();

    // Remove in foreground mode - no mismatch notice
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["--foreground", "feature-fg"],
        None
    ));
}

#[rstest]
fn test_remove_detached_worktree_in_multi(mut repo: TestRepo) {
    // Create two worktrees
    let _feature_a = repo.add_worktree("feature-a");
    let _feature_b = repo.add_worktree("feature-b");

    // Detach HEAD in feature-b
    repo.detach_head_in_worktree("feature-b");

    // From main, try to multi-remove both
    // feature-a should succeed, feature-b should fail (detached HEAD)
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["feature-a", "feature-b"],
        None
    ));
}

/// Reproduces #1661: "(detached)" is not a valid branch name — verify it fails.
#[rstest]
fn test_remove_detached_by_name_fails(mut repo: TestRepo) {
    repo.add_worktree("feature-detached");
    repo.detach_head_in_worktree("feature-detached");

    // "(detached)" is not a branch name — this should fail
    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "remove", &["(detached)"], None));
}

/// Verify that detached worktrees can be removed by absolute path (#1661).
/// This ensures the CLI supports the same operation the picker uses.
#[rstest]
fn test_remove_detached_worktree_by_path(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature-detached");
    repo.detach_head_in_worktree("feature-detached");

    assert!(worktree_path.exists());

    let worktree_str = worktree_path.to_string_lossy().to_string();
    let output = repo
        .wt_command()
        .args(["remove", &worktree_str, "--foreground", "--yes"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "wt remove should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !worktree_path.exists(),
        "Worktree directory should be removed"
    );
}

/// Verify that detached worktrees can be removed by relative path.
/// This tests `Repository::resolve_worktree`'s path resolution, which here runs
/// from a cwd inside the repo.
#[rstest]
fn test_remove_detached_worktree_by_relative_path(mut repo: TestRepo) {
    repo.add_worktree("feature-detached");
    repo.detach_head_in_worktree("feature-detached");

    // From the main worktree (repo/), the detached worktree is at ../repo.feature-detached
    let relative_path = "../repo.feature-detached";
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &[relative_path, "--foreground", "--yes"],
        None,
    ));
}

/// A relative path resolves against `-C`, not the process cwd — git's own rule
/// for path arguments under `-C`.
///
/// The test above reaches the worktree from a cwd inside the repo, the route
/// that works under either rule. Running from outside the repo is what tells
/// the two resolution bases apart: `../repo.feature-detached` names the
/// worktree only when it is resolved from the `-C` directory.
#[rstest]
fn test_remove_detached_worktree_by_relative_path_honors_directory_flag(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature-detached");
    repo.detach_head_in_worktree("feature-detached");

    let outside = repo.root_path().parent().unwrap().to_path_buf();
    let root = repo.root_path().to_string_lossy().to_string();
    let output = repo
        .wt_command()
        .current_dir(&outside)
        .args([
            "-C",
            &root,
            "remove",
            "../repo.feature-detached",
            "--foreground",
            "--yes",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "remove should resolve the relative path against -C:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !worktree_path.exists(),
        "the worktree the path names relative to -C should be gone"
    );
}

/// Test that resolve_worktree("@") works when the worktree is accessed via a symlink.
///
/// This tests the path normalization fix where:
/// - `root()` returns a canonicalized path (symlinks resolved)
/// - `wt.path` from git is the raw path (symlinks not resolved)
///
/// Without proper canonicalization, comparison fails on systems with symlinks
/// (e.g., macOS /var -> /private/var).
#[cfg(unix)]
#[rstest]
fn test_remove_at_symbol_via_symlink(mut repo: TestRepo) {
    use std::os::unix::fs::symlink;

    let worktree_path = repo.add_worktree("feature-symlink");

    // Create a symlink pointing to the worktree
    let symlink_path = repo
        .root_path()
        .parent()
        .unwrap()
        .join("symlink-to-feature");
    symlink(&worktree_path, &symlink_path).expect("Failed to create symlink");

    // Verify symlink was created
    assert!(
        symlink_path.is_symlink(),
        "Symlink should exist at {:?}",
        symlink_path
    );

    // Run `wt remove @` from the symlinked path
    // This tests that resolve_worktree("@") properly handles symlinked paths
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["@"],
        Some(&symlink_path)
    ));
}

// ============================================================================
// Pruned Worktree Tests
// ============================================================================

/// When a worktree's directory is deleted externally (e.g., `rm -rf`), the git
/// metadata becomes stale. `wt remove` should prune this stale metadata and
/// proceed with branch deletion, rather than erroring.
///
/// This makes `wt remove` more idempotent - it puts the repository into the
/// correct end state regardless of whether the directory exists.
#[rstest]
fn test_remove_pruned_worktree_directory_missing(mut repo: TestRepo) {
    // Create a worktree
    let worktree_path = repo.add_worktree("feature-pruned");

    // Verify the worktree exists
    assert!(worktree_path.exists(), "Worktree should exist initially");

    // Externally delete the worktree directory (simulating user running `rm -rf`)
    std::fs::remove_dir_all(&worktree_path).expect("Failed to remove worktree directory");
    assert!(
        !worktree_path.exists(),
        "Worktree directory should be deleted"
    );

    // Verify git still thinks the worktree exists (stale metadata)
    let list_output = repo
        .git_command()
        .args(["worktree", "list", "--porcelain"])
        .run()
        .unwrap();
    let list_str = String::from_utf8_lossy(&list_output.stdout);
    assert!(
        list_str.contains("feature-pruned"),
        "Git should still list the stale worktree"
    );

    // `wt remove feature-pruned` should prune the stale metadata and delete the branch
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["feature-pruned"],
        None
    ));

    // Verify the stale worktree metadata is cleaned up
    let list_after = repo
        .git_command()
        .args(["worktree", "list", "--porcelain"])
        .run()
        .unwrap();
    let list_after_str = String::from_utf8_lossy(&list_after.stdout);
    assert!(
        !list_after_str.contains("feature-pruned"),
        "Stale worktree should be pruned"
    );

    // Verify the branch is deleted
    let branch_exists = repo
        .git_command()
        .args(["branch", "--list", "feature-pruned"])
        .run()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&branch_exists.stdout)
            .trim()
            .is_empty(),
        "Branch should be deleted"
    );
}

/// Test pruning with --no-delete-branch: should prune metadata but keep the branch
#[rstest]
fn test_remove_pruned_worktree_keep_branch(mut repo: TestRepo) {
    // Create a worktree
    let worktree_path = repo.add_worktree("feature-pruned-keep");

    // Delete the worktree directory externally
    std::fs::remove_dir_all(&worktree_path).expect("Failed to remove worktree directory");

    // Remove with --no-delete-branch
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["--no-delete-branch", "feature-pruned-keep"],
        None
    ));

    // Verify the branch still exists
    let branch_exists = repo
        .git_command()
        .args(["branch", "--list", "feature-pruned-keep"])
        .run()
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&branch_exists.stdout)
            .trim()
            .is_empty(),
        "Branch should still exist"
    );
}

/// Test pruning a stale worktree with an unmerged branch: should prune metadata,
/// retain branch, and show hint to force-delete
#[rstest]
fn test_remove_pruned_worktree_unmerged_branch(mut repo: TestRepo) {
    // Create a worktree with a real change (unmerged with main)
    let worktree_path = repo.add_worktree("feature-pruned-unmerged");
    std::fs::write(worktree_path.join("unmerged.txt"), "unmerged work\n").unwrap();
    repo.git_command()
        .args(["add", "unmerged.txt"])
        .current_dir(&worktree_path)
        .run()
        .unwrap();
    repo.git_command()
        .args(["commit", "-m", "unmerged work"])
        .current_dir(&worktree_path)
        .run()
        .unwrap();

    // Delete the worktree directory externally (simulating user running `rm -rf`)
    std::fs::remove_dir_all(&worktree_path).expect("Failed to remove worktree directory");

    // Remove: should prune stale metadata but retain the unmerged branch
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "remove",
        &["feature-pruned-unmerged"],
        None
    ));

    // Verify the branch still exists (retained because unmerged)
    let branch_exists = repo
        .git_command()
        .args(["branch", "--list", "feature-pruned-unmerged"])
        .run()
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&branch_exists.stdout)
            .trim()
            .is_empty(),
        "Unmerged branch should be retained"
    );
}

// ============================================================================
// Instant Removal Tests (move-then-delete optimization)
// ============================================================================

/// Background removal should make the original worktree path unavailable immediately.
///
/// This tests the move-then-delete optimization: the worktree directory is renamed
/// to a staging path synchronously, so the original path is gone before wt returns.
/// The actual deletion (rm -rf) happens in the background.
#[rstest]
fn test_remove_background_path_gone_immediately(mut repo: TestRepo) {
    // Create a worktree
    let worktree_path = repo.add_worktree("feature-instant");

    // Verify the worktree exists
    assert!(worktree_path.exists(), "Worktree should exist initially");

    // Remove in background mode (default) - NOT using snapshot since we need to check state after
    let output = repo
        .wt_command()
        .args(["remove", "feature-instant"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "wt remove should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The worktree contents should be gone IMMEDIATELY (moved to .git/wt/trash/).
    // No placeholder created because this is a non-current worktree removal.
    assert!(!worktree_path.exists(), "Worktree should be fully removed");
}

/// Background removal should prune git worktree metadata synchronously.
///
/// After removal, `git worktree list` should NOT show the removed worktree,
/// even before the background rm -rf completes.
#[rstest]
fn test_remove_background_git_metadata_pruned(mut repo: TestRepo) {
    // Create a worktree
    let _worktree_path = repo.add_worktree("feature-prune-test");

    // Verify git knows about the worktree
    let list_before = repo
        .git_command()
        .args(["worktree", "list", "--porcelain"])
        .run()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&list_before.stdout).contains("feature-prune-test"),
        "Git should list the worktree before removal"
    );

    // Remove in background mode
    let output = repo
        .wt_command()
        .args(["remove", "feature-prune-test"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "wt remove should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Git worktree metadata should be pruned IMMEDIATELY
    let list_after = repo
        .git_command()
        .args(["worktree", "list", "--porcelain"])
        .run()
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&list_after.stdout).contains("feature-prune-test"),
        "Git should NOT list the worktree after removal (metadata should be pruned)"
    );
}

/// Background removal should delete the branch synchronously when it's merged.
///
/// On the fast path (rename-then-prune), the branch is deleted synchronously
/// after pruning git metadata, before the background `rm -rf` runs.
/// This prevents races where the user creates a new worktree with the same
/// branch name before the background process completes.
#[rstest]
fn test_remove_background_deletes_merged_branch(mut repo: TestRepo) {
    // Create a worktree with the branch already merged to main (same commit)
    let _worktree_path = repo.add_worktree("feature-merged");

    // Verify branch exists before removal
    let branches_before = repo
        .git_command()
        .args(["branch", "--list", "feature-merged"])
        .run()
        .unwrap();
    assert!(
        !String::from_utf8_lossy(&branches_before.stdout)
            .trim()
            .is_empty(),
        "Branch should exist before removal"
    );

    // Remove in background mode (default)
    let output = repo
        .wt_command()
        .args(["remove", "feature-merged"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "wt remove should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Branch should be deleted IMMEDIATELY (synchronously, not in background)
    let branches_after = repo
        .git_command()
        .args(["branch", "--list", "feature-merged"])
        .run()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&branches_after.stdout)
            .trim()
            .is_empty(),
        "Branch should be deleted synchronously after wt remove returns"
    );
}

/// Test that worktree paths containing special characters are handled correctly.
///
/// This tests that the `rm -rf -- <path>` command correctly handles paths
/// that might be misinterpreted as options.
#[rstest]
fn test_remove_worktree_with_special_path_chars(mut repo: TestRepo) {
    // Create a worktree with special characters in the branch name
    // (which becomes part of the path)
    let _worktree_path = repo.add_worktree("feature--double-dash");

    // Verify worktree exists
    let list_before = repo
        .git_command()
        .args(["worktree", "list", "--porcelain"])
        .run()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&list_before.stdout).contains("feature--double-dash"),
        "Worktree should exist before removal"
    );

    // Remove the worktree
    let output = repo
        .wt_command()
        .args(["remove", "feature--double-dash"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "wt remove should succeed for path with special chars: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Poll for background worktree removal
    crate::common::wait_for("worktree with special chars removed", || {
        let list = repo
            .git_command()
            .args(["worktree", "list", "--porcelain"])
            .run()
            .unwrap();
        !String::from_utf8_lossy(&list.stdout).contains("feature--double-dash")
    });
}

/// Test that background removal falls back to legacy git worktree remove
/// when the instant rename fails.
///
/// This tests the fallback path: when std::fs::rename() fails (e.g., cross-filesystem,
/// permissions, or in this case a blocking file), we fall back to the legacy
/// `git worktree remove` command which handles cleanup properly.
#[rstest]
fn test_remove_background_fallback_on_rename_failure(mut repo: TestRepo) {
    // Create a worktree
    let worktree_path = repo.add_worktree("feature-fallback");

    // Calculate the expected staged path that the rename would use.
    // The path is: <git-common-dir>/wt/trash/<name>-<TEST_EPOCH>
    // Since WT_TEST_EPOCH is set by the test harness, the timestamp is deterministic.
    let git_common_dir = crate::common::resolve_git_common_dir(repo.root_path());
    let trash_dir = git_common_dir.join("wt/trash");
    std::fs::create_dir_all(&trash_dir).unwrap();
    let staged_path = trash_dir.join(format!(
        "{}-{}",
        worktree_path.file_name().unwrap().to_string_lossy(),
        crate::common::TEST_EPOCH
    ));

    // Create a regular file at the staged path to block the rename.
    // On POSIX systems, you cannot rename a directory to an existing file.
    std::fs::write(&staged_path, "blocking file").unwrap();

    // Verify worktree exists before removal
    assert!(
        worktree_path.exists(),
        "Worktree should exist before removal"
    );

    // Remove in background mode - should fall back to legacy removal
    let output = repo
        .wt_command()
        .args(["remove", "feature-fallback"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "wt remove should succeed even when instant rename fails: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Poll for legacy background removal (includes 1-second sleep before git worktree remove)
    crate::common::wait_for("worktree removed by legacy fallback", || {
        !worktree_path.exists()
    });

    // Poll for branch deletion (happens after worktree removal in background command)
    crate::common::wait_for("branch deleted by legacy fallback", || {
        let branches = repo
            .git_command()
            .args(["branch", "--list", "feature-fallback"])
            .run()
            .unwrap();
        String::from_utf8_lossy(&branches.stdout).trim().is_empty()
    });

    // Clean up the blocking file
    let _ = std::fs::remove_file(&staged_path);
}

/// Block the rename-into-trash fast path for `worktree_path` by pre-creating a
/// regular file at its deterministic staged path. Returns that path so the
/// caller can clean it up. (On POSIX a directory cannot be renamed onto an
/// existing file, so `stage_worktree_removal` falls back to legacy removal.)
fn block_staged_rename(repo: &TestRepo, worktree_path: &std::path::Path) -> std::path::PathBuf {
    let trash_dir = crate::common::resolve_git_common_dir(repo.root_path()).join("wt/trash");
    std::fs::create_dir_all(&trash_dir).unwrap();
    let staged_path = trash_dir.join(format!(
        "{}-{}",
        worktree_path.file_name().unwrap().to_string_lossy(),
        crate::common::TEST_EPOCH
    ));
    std::fs::write(&staged_path, "blocking file").unwrap();
    staged_path
}

/// The rename-failure fallback honors `-D`: an unmerged branch is force-deleted
/// in the legacy `git worktree remove && git branch -D` command.
#[rstest]
fn test_remove_background_fallback_force_delete_branch(mut repo: TestRepo) {
    repo.commit("initial");
    // Unmerged branch: a plain `-d` would refuse it, so this exercises the
    // `BranchDeletionMode::ForceDelete` arm of the fallback command builder.
    let worktree_path =
        repo.add_worktree_with_commit("feature-force", "f.txt", "content", "unmerged commit");
    let staged_path = block_staged_rename(&repo, &worktree_path);

    let output = repo
        .wt_command()
        .args(["remove", "--force", "-D", "feature-force"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "wt remove --force -D should succeed via the legacy fallback: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    crate::common::wait_for("worktree removed by legacy fallback", || {
        !worktree_path.exists()
    });
    crate::common::wait_for("unmerged branch force-deleted by legacy fallback", || {
        let branches = repo
            .git_command()
            .args(["branch", "--list", "feature-force"])
            .run()
            .unwrap();
        String::from_utf8_lossy(&branches.stdout).trim().is_empty()
    });

    let _ = std::fs::remove_file(&staged_path);
}

/// The rename-failure fallback removes a detached-HEAD worktree with no branch
/// to delete — the `_` arm of the fallback command builder. `wt remove` resolves
/// the detached worktree by path.
#[rstest]
fn test_remove_background_fallback_detached_worktree(mut repo: TestRepo) {
    repo.commit("initial");
    let worktree_path = repo.add_worktree("feature-detached");
    repo.detach_head_in_worktree("feature-detached");
    let staged_path = block_staged_rename(&repo, &worktree_path);

    let output = repo
        .wt_command()
        .args(["remove", worktree_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "wt remove of a detached worktree should succeed via the legacy fallback: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    crate::common::wait_for("detached worktree removed by legacy fallback", || {
        !worktree_path.exists()
    });

    let _ = std::fs::remove_file(&staged_path);
}

/// Stale staging directories from crashed removals are contained in `.git/wt/trash/`.
///
/// If `wt remove` is killed after `fs::rename()` succeeds but before the background
/// `rm -rf` spawns, the staging directory is left behind inside `.git/wt/trash/`.
/// Unlike the old sibling-path approach, these are hidden from the user's workspace.
/// When the same worktree is re-created and removed again, the new staging path uses
/// a fresh timestamp so there is no collision.
#[rstest]
fn test_remove_stale_staging_dir_from_crashed_removal(mut repo: TestRepo) {
    let worktree_path = repo.add_worktree("feature-crash");

    // Calculate the deterministic staging path (TEST_EPOCH is fixed in tests)
    let git_common_dir = crate::common::resolve_git_common_dir(repo.root_path());
    let trash_dir = git_common_dir.join("wt/trash");
    std::fs::create_dir_all(&trash_dir).unwrap();
    let staged_path = trash_dir.join(format!(
        "{}-{}",
        worktree_path.file_name().unwrap().to_string_lossy(),
        crate::common::TEST_EPOCH
    ));

    // Simulate a crashed removal: rename the worktree to the staging path manually,
    // then prune git metadata — but never run the background rm -rf.
    std::fs::rename(&worktree_path, &staged_path).unwrap();
    repo.run_git(&["worktree", "prune"]);

    // Verify the crash state: original path gone, stale staging dir remains in .git/wt/trash/
    assert!(!worktree_path.exists());
    assert!(staged_path.exists());

    // The stale dir is inside .git/ — invisible to the user, unlike the old
    // sibling-path approach that left confusingly-named dirs in the workspace.
    assert!(
        staged_path.starts_with(&git_common_dir),
        "Stale staging dir should be inside .git/"
    );
}

/// `wt remove` sweeps `.git/wt/trash/` entries older than 24 hours.
///
/// Each run of `wt remove` fires a detached `rm -rf` on trash entries whose
/// encoded timestamp is more than a day in the past. This provides eventual
/// cleanup for directories orphaned when a previous background removal was
/// interrupted. Fresh entries (from recent or in-flight removals) are left
/// alone so concurrent removals don't race.
#[rstest]
fn test_remove_sweeps_stale_trash_entries(mut repo: TestRepo) {
    let git_common_dir = crate::common::resolve_git_common_dir(repo.root_path());
    let trash_dir = git_common_dir.join("wt/trash");
    std::fs::create_dir_all(&trash_dir).unwrap();

    // Pre-populate the trash directory with a stale entry (2 days old) and a
    // fresh entry (just created). The stale entry should be swept; the fresh
    // entry should survive.
    let day = 24 * 60 * 60;
    let stale_timestamp = crate::common::TEST_EPOCH - 2 * day;
    let stale_entry = trash_dir.join(format!("orphan-stale-{stale_timestamp}"));
    let fresh_entry = trash_dir.join(format!("orphan-fresh-{}", crate::common::TEST_EPOCH));
    std::fs::create_dir(&stale_entry).unwrap();
    std::fs::write(stale_entry.join("marker"), "leftover").unwrap();
    std::fs::create_dir(&fresh_entry).unwrap();
    std::fs::write(fresh_entry.join("marker"), "recent").unwrap();

    // Create and remove a real worktree to trigger the sweep, which runs after
    // the primary `wt remove` output has been printed.
    let _ = repo.add_worktree("feature-sweep");
    let output = repo
        .wt_command()
        .args(["remove", "feature-sweep"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "wt remove should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The stale entry is removed by a detached `rm -rf` — poll for absence.
    crate::common::wait_for("stale trash entry swept", || !stale_entry.exists());

    // The fresh entry must survive — only entries older than 24 hours are swept.
    assert!(
        fresh_entry.exists(),
        "fresh trash entry (age 0) must not be swept"
    );
}

/// `wt remove -vv` resolves every fsmonitor daemon in ONE `lsof` spawn and
/// traces the sweep.
///
/// The spawn count is the load-bearing assertion. The candidate set is
/// machine-wide, so a machine that has accumulated a daemon per repo ever
/// touched (100+ is routine) once paid one `lsof` spawn each on every
/// `wt remove`. That fork storm makes macOS assess each new image under
/// contention, inflating per-spawn cost for everything else on the box — so
/// regressing to a per-PID loop is not a linear slowdown, and a duration
/// assertion would not catch it on an idle CI runner. Counting spawns does.
///
/// `pgrep` is mocked to report three bogus PIDs (CI runners have no real
/// daemons) and `lsof` to return batched `-F pn` output covering all three.
#[rstest]
#[cfg(unix)]
fn test_remove_resolves_all_fsmonitor_daemons_in_one_lsof(mut repo: TestRepo) {
    use crate::common::mock_commands::{MockConfig, MockResponse, mock_calls};

    let bin_dir = repo.root_path().join(".bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    MockConfig::new("pgrep")
        .command("_default", MockResponse::output("777001\n777002\n777003\n"))
        .write(&bin_dir);
    // Batched `lsof -F pn` shape: a `p<pid>` line opens each process record.
    // All three sockets resolve under a git-dir that is not this repo's, so
    // the sweep classifies them as "not ours" and signals nothing — the test
    // asserts call shape, and must never depend on killing a real PID.
    MockConfig::new("lsof")
        .command(
            "_default",
            MockResponse::output(concat!(
                "p777001\nf18\nn/elsewhere/a/.git/fsmonitor--daemon.ipc\n",
                "p777002\nf18\nn/elsewhere/b/.git/fsmonitor--daemon.ipc\n",
                "p777003\nf18\nn/elsewhere/c/.git/fsmonitor--daemon.ipc\n",
            )),
        )
        .write(&bin_dir);

    let mut paths: Vec<std::path::PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    paths.insert(0, bin_dir.clone());
    let new_path = std::env::join_paths(&paths).unwrap();

    // Outside the repo: a call log written under `bin_dir` (which lives at
    // `<repo>/.bin`) would leave an untracked file in the working tree the
    // command under test is inspecting.
    let call_log = tempfile::tempdir().unwrap();

    repo.add_worktree("feature-fsmon");
    let output = repo
        .wt_command()
        .args(["remove", "feature-fsmon", "-vv"])
        .env("PATH", &new_path)
        .env("WORKTRUNK_TEST_MOCK_CONFIG_DIR", &bin_dir)
        .env("WORKTRUNK_TEST_MOCK_CALL_LOG_DIR", call_log.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "wt remove should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let calls = mock_calls(call_log.path(), "lsof");
    assert_eq!(
        calls.len(),
        1,
        "3 daemons must cost exactly one lsof spawn, not one per PID. calls: {calls:#?}"
    );
    assert!(
        calls[0].contains("777001,777002,777003"),
        "the single lsof call must pass every PID as one comma-separated list. call: {}",
        calls[0]
    );

    // -vv streams primary output to stderr but writes trace records to the
    // log files; the sweep runs in the foreground process, so its records are
    // complete once the command exits.
    let trace_log =
        crate::common::resolve_git_common_dir(repo.root_path()).join("wt/logs/trace.log");
    let trace = std::fs::read_to_string(&trace_log).unwrap();
    assert!(
        trace.contains("◷ enumerate-fsmonitor-daemons"),
        "sweep span should appear in the -vv trace. trace.log: {trace}"
    );
    assert!(
        trace.contains("resolving sockets for 3 daemon(s) via one lsof"),
        "sweep should surface the daemon count. trace.log: {trace}"
    );
}

/// When `pgrep` succeeds but yields no parseable PID, the sweep returns before
/// spawning `lsof` at all.
///
/// `pgrep` exits 0 with output that isn't a PID (the `parse::<u32>` filter
/// drops every line), so the candidate set is empty. `enumerate_daemons` must
/// take its empty-set early return — never build a `lsof -p` call with an
/// empty PID list, which would resolve every open Unix socket on the machine.
/// Asserted by `lsof` being spawned zero times and the per-daemon "resolving
/// sockets" trace line (emitted only past the guard) being absent.
#[rstest]
#[cfg(unix)]
fn test_remove_sweep_skips_lsof_when_no_daemon_pids(mut repo: TestRepo) {
    use crate::common::mock_commands::{MockConfig, MockResponse, mock_calls};

    let bin_dir = repo.root_path().join(".bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    // Exit 0 (a match) but a non-numeric line, so parsing drops it to empty.
    MockConfig::new("pgrep")
        .command("_default", MockResponse::output("not-a-pid\n"))
        .write(&bin_dir);
    // Present so a stray call would be logged and caught; it must never run.
    MockConfig::new("lsof")
        .command("_default", MockResponse::output(""))
        .write(&bin_dir);

    let mut paths: Vec<std::path::PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    paths.insert(0, bin_dir.clone());
    let new_path = std::env::join_paths(&paths).unwrap();

    let call_log = tempfile::tempdir().unwrap();

    repo.add_worktree("feature-fsmon");
    let output = repo
        .wt_command()
        .args(["remove", "feature-fsmon", "-vv"])
        .env("PATH", &new_path)
        .env("WORKTRUNK_TEST_MOCK_CONFIG_DIR", &bin_dir)
        .env("WORKTRUNK_TEST_MOCK_CALL_LOG_DIR", call_log.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "wt remove should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        mock_calls(call_log.path(), "lsof").is_empty(),
        "an empty PID set must skip the lsof spawn entirely"
    );

    let trace_log =
        crate::common::resolve_git_common_dir(repo.root_path()).join("wt/logs/trace.log");
    let trace = std::fs::read_to_string(&trace_log).unwrap();
    assert!(
        trace.contains("◷ enumerate-fsmonitor-daemons"),
        "sweep span should still appear even when it finds no PIDs. trace.log: {trace}"
    );
    assert!(
        !trace.contains("resolving sockets for"),
        "the per-daemon resolution line must not appear when there are no PIDs. trace.log: {trace}"
    );
}

/// Tests that foreground removal shows remaining directory entries when
/// `git worktree remove` fails because a directory can't be deleted.
///
/// Uses Unix permissions (non-writable directory) to prevent deletion of
/// a gitignored directory with a non-writable subdirectory. The fast path
/// (rename to trash) handles this gracefully — the entire worktree directory
/// is renamed atomically regardless of internal permissions.
#[rstest]
#[cfg(unix)]
fn test_remove_foreground_succeeds_with_stuck_directory(mut repo: TestRepo) {
    use std::fs::{self, Permissions};
    use std::os::unix::fs::PermissionsExt;

    let worktree_path = repo.add_worktree("feature-stuck");

    // Add .gitignore so the stuck directory passes the clean check
    fs::write(worktree_path.join(".gitignore"), "stuck/\n").unwrap();
    repo.run_git_in(&worktree_path, &["add", ".gitignore"]);
    repo.run_git_in(&worktree_path, &["commit", "-m", "Add gitignore"]);

    // Create gitignored directory with a non-writable file inside
    let stuck_dir = worktree_path.join("stuck");
    fs::create_dir_all(&stuck_dir).unwrap();
    fs::write(stuck_dir.join("file.txt"), "content").unwrap();
    fs::set_permissions(&stuck_dir, Permissions::from_mode(0o555)).unwrap();

    // Check if permissions actually restrict us (skip if running as root)
    let test_file = stuck_dir.join("test_write");
    if fs::write(&test_file, "test").is_ok() {
        let _ = fs::remove_file(&test_file);
        fs::set_permissions(&stuck_dir, Permissions::from_mode(0o755)).unwrap();
        eprintln!("Skipping - running with elevated privileges");
        return;
    }

    let output = repo
        .wt_command()
        .args(["remove", "--foreground", "feature-stuck"])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Restore permissions in trash dir so TempDir cleanup works
    let git_dir = repo.root_path().join(".git");
    let trash_dir = git_dir.join("wt").join("trash");
    if trash_dir.exists() {
        for entry in fs::read_dir(&trash_dir).unwrap().flatten() {
            restore_dir_permissions(&entry.path());
        }
    }

    assert!(
        output.status.success(),
        "Remove should succeed via fast path, got: {stderr}"
    );
    assert!(!worktree_path.exists(), "Worktree directory should be gone");
}

/// Same as above but for the detached HEAD code path.
#[rstest]
#[cfg(unix)]
fn test_remove_foreground_succeeds_with_stuck_directory_detached(mut repo: TestRepo) {
    use std::fs::{self, Permissions};
    use std::os::unix::fs::PermissionsExt;

    let worktree_path = repo.add_worktree("feature-stuck-detached");

    // Commit .gitignore, then detach HEAD
    fs::write(worktree_path.join(".gitignore"), "stuck/\n").unwrap();
    repo.run_git_in(&worktree_path, &["add", ".gitignore"]);
    repo.run_git_in(&worktree_path, &["commit", "-m", "Add gitignore"]);
    repo.detach_head_in_worktree("feature-stuck-detached");

    // Create gitignored directory with a non-writable file inside
    let stuck_dir = worktree_path.join("stuck");
    fs::create_dir_all(&stuck_dir).unwrap();
    fs::write(stuck_dir.join("file.txt"), "content").unwrap();
    fs::set_permissions(&stuck_dir, Permissions::from_mode(0o555)).unwrap();

    // Skip if running as root
    let test_file = stuck_dir.join("test_write");
    if fs::write(&test_file, "test").is_ok() {
        let _ = fs::remove_file(&test_file);
        fs::set_permissions(&stuck_dir, Permissions::from_mode(0o755)).unwrap();
        eprintln!("Skipping - running with elevated privileges");
        return;
    }

    let output = repo
        .wt_command()
        .args(["remove", "--foreground"])
        .current_dir(&worktree_path)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Restore permissions in trash dir so TempDir cleanup works
    let git_dir = repo.root_path().join(".git");
    let trash_dir = git_dir.join("wt").join("trash");
    if trash_dir.exists() {
        for entry in fs::read_dir(&trash_dir).unwrap().flatten() {
            restore_dir_permissions(&entry.path());
        }
    }

    assert!(
        output.status.success(),
        "Remove should succeed via fast path, got: {stderr}"
    );
    assert!(!worktree_path.exists(), "Worktree directory should be gone");
}

/// Worktrees with initialized git submodules should be removable.
///
/// Git refuses `git worktree remove` when submodules are initialized,
/// requiring `--force`. This test verifies that `wt remove --foreground`
/// handles this automatically (retries with `--force`).
///
/// Regression test for <https://github.com/max-sixty/worktrunk/issues/1194>.
#[rstest]
fn test_remove_foreground_with_submodules(mut repo: TestRepo) {
    // Create a local repo to use as a submodule source
    let sub_source = repo.root_path().parent().unwrap().join("sub-source");
    std::fs::create_dir_all(&sub_source).unwrap();
    repo.run_git_in(&sub_source, &["init", "-b", "main"]);
    std::fs::write(sub_source.join("sub.txt"), "submodule content").unwrap();
    repo.run_git_in(&sub_source, &["add", "sub.txt"]);
    repo.run_git_in(&sub_source, &["commit", "-m", "sub init"]);

    // Add submodule to the main repo (requires protocol.file.allow=always)
    let output = repo
        .git_command()
        .args([
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            sub_source.to_str().unwrap(),
            "submod",
        ])
        .run()
        .unwrap();
    assert!(
        output.status.success(),
        "Failed to add submodule: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    repo.run_git(&["commit", "-m", "add submodule"]);

    // Create a worktree and initialize submodules in it
    let worktree_path = repo.add_worktree("feature-submod");
    let output = repo
        .git_command()
        .current_dir(&worktree_path)
        .args([
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "update",
            "--init",
        ])
        .run()
        .unwrap();
    assert!(
        output.status.success(),
        "Failed to init submodule: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify the submodule is actually initialized
    assert!(
        worktree_path.join("submod").join("sub.txt").exists(),
        "Submodule should be initialized"
    );

    // Remove the worktree in foreground mode — should succeed despite submodules
    let output = repo
        .wt_command()
        .args(["remove", "--foreground", "feature-submod"])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "Remove should succeed with submodules, got: {stderr}"
    );
    assert!(
        !worktree_path.exists(),
        "Worktree directory should be removed"
    );
}

/// Regression: `Repository::remove_worktree(path, force=false)` synthesizes
/// `git worktree remove --force` for submodule worktrees, which suppresses
/// git's own dirty-file check. The method must re-validate cleanliness
/// itself right before the synthesized-force command so a file dirtied after
/// the caller's planning-time check is not silently destroyed.
#[rstest]
fn test_remove_worktree_submodule_dirty_fails_closed(mut repo: TestRepo) {
    use worktrunk::git::Repository;

    // Submodule source.
    let sub_source = repo.root_path().parent().unwrap().join("sub-source-dirty");
    std::fs::create_dir_all(&sub_source).unwrap();
    repo.run_git_in(&sub_source, &["init", "-b", "main"]);
    std::fs::write(sub_source.join("sub.txt"), "submodule content").unwrap();
    repo.run_git_in(&sub_source, &["add", "sub.txt"]);
    repo.run_git_in(&sub_source, &["commit", "-m", "sub init"]);

    std::fs::write(repo.root_path().join("tracked.txt"), "original\n").unwrap();
    repo.run_git(&["add", "tracked.txt"]);
    repo.run_git(&["commit", "-m", "add tracked file"]);

    let output = repo
        .git_command()
        .args([
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            sub_source.to_str().unwrap(),
            "submod",
        ])
        .run()
        .unwrap();
    assert!(
        output.status.success(),
        "Failed to add submodule: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    repo.run_git(&["commit", "-m", "add submodule"]);

    let worktree_path = repo.add_worktree("feature-submod-dirty");
    let output = repo
        .git_command()
        .current_dir(&worktree_path)
        .args([
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "update",
            "--init",
        ])
        .run()
        .unwrap();
    assert!(
        output.status.success(),
        "Failed to init submodule: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Simulate the TOCTOU window: a tracked file is modified after the
    // caller's clean check but before remove_worktree's destructive step.
    std::fs::write(worktree_path.join("tracked.txt"), "DIRTIED\n").unwrap();

    let repo_api = Repository::at(repo.root_path()).unwrap();
    let result = repo_api.remove_worktree(&worktree_path, /* force */ false);

    assert!(
        result.is_err(),
        "remove_worktree must fail closed when a submodule worktree is dirty \
         (synthesized --force would otherwise destroy the change)"
    );
    assert!(
        worktree_path.exists(),
        "submodule worktree must be preserved, not force-removed"
    );
    assert_eq!(
        std::fs::read_to_string(worktree_path.join("tracked.txt")).unwrap(),
        "DIRTIED\n",
        "the post-check modification must be intact (not destroyed)"
    );
}

/// Restore write permissions recursively so TempDir cleanup succeeds.
#[cfg(unix)]
fn restore_dir_permissions(dir: &std::path::Path) {
    use std::fs::{self, Permissions};
    use std::os::unix::fs::PermissionsExt;

    let _ = fs::set_permissions(dir, Permissions::from_mode(0o755));
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                restore_dir_permissions(&entry.path());
            }
        }
    }
}

// ============================================================================
// Docs-page example snapshot
//
// See tests/integration_tests/merge.rs header comment for the docs-example
// convention — `<!-- wt remove (docs-example) -->` in `src/cli/mod.rs`.
// ============================================================================

/// `wt remove` example for `docs/content/remove.md` — pre-remove hook running
/// `flyctl scale count 0`, background cleanup.
#[rstest]
fn test_docs_remove_pre_remove_hook(mut repo: TestRepo) {
    repo.run_git(&["config", "worktrunk.hints.worktree-path", "true"]);

    let bin_dir = repo.root_path().join(".bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    crate::common::mock_commands::MockConfig::new("flyctl")
        .command(
            "scale",
            crate::common::mock_commands::MockResponse::output("Scaling app to 0 machines\n"),
        )
        .write(&bin_dir);

    repo.write_project_config(
        r#"[[pre-remove]]
cleanup = "flyctl scale count 0"
"#,
    );
    repo.run_git(&["add", ".config", ".bin"]);
    repo.run_git(&["commit", "-m", "Add project config"]);

    let api_wt = repo.add_worktree("api");

    let directive_file = repo
        .root_path()
        .parent()
        .unwrap()
        .join(".wt-directive-docs-remove");
    std::fs::write(&directive_file, "").unwrap();

    let mut paths: Vec<std::path::PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    paths.insert(0, bin_dir.clone());
    let new_path = std::env::join_paths(&paths).unwrap();
    let bin_dir_str = bin_dir.to_string_lossy().into_owned();
    let directive_file_str = directive_file.to_string_lossy().into_owned();

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        assert_cmd_snapshot!("docs_remove_pre_remove_hook", {
            let mut cmd = make_snapshot_cmd(&repo, "remove", &["--yes"], Some(&api_wt));
            cmd.env("PATH", &new_path);
            cmd.env("WORKTRUNK_TEST_MOCK_CONFIG_DIR", &bin_dir_str);
            cmd.env("WORKTRUNK_DIRECTIVE_CD_FILE", &directive_file_str);
            cmd
        });
    });
}

// ============================================================================
// --format=json
// ============================================================================

/// Removing the current worktree by omitting the branch takes a separate
/// single-worktree path from the named removals above, with its own
/// `--format=json` emission. The shape must match: one array, one item.
///
/// Removal stays backgrounded: `wt` runs with the doomed worktree as its cwd,
/// and Windows refuses to delete a directory a live process is sitting in, so
/// `--foreground` would make this a Windows-only failure. The JSON is emitted
/// either way — the execution mode picks who deletes, not who reports.
#[rstest]
fn test_remove_json_current_worktree_no_args(mut repo: TestRepo) {
    use crate::common::wait_for_worktree_removed;

    repo.commit("initial");
    let feature_wt = repo.add_worktree("feature");

    let output = repo
        .wt_command()
        .args(["remove", "--format=json", "--yes"])
        .current_dir(&feature_wt)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr)
        .ansi_strip()
        .into_owned();
    assert!(output.status.success(), "remove should succeed:\n{stderr}");

    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();
    let items = json.as_array().unwrap();
    assert_eq!(items.len(), 1, "one worktree removed, one item:\n{stderr}");
    assert_eq!(items[0]["branch"], "feature");

    wait_for_worktree_removed(&feature_wt);
}

#[rstest]
fn test_remove_json(mut repo: TestRepo) {
    repo.commit("initial");
    repo.add_worktree("feature");

    let output = repo
        .wt_command()
        .args([
            "remove",
            "feature",
            "--format=json",
            "--yes",
            "--foreground",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());

    let mut settings = insta::Settings::clone_current();
    settings.add_filter(r#""path": "[^"]*""#, r#""path": "<PATH>""#);
    settings.bind(|| {
        assert_snapshot!(String::from_utf8_lossy(&output.stdout));
    });
}

#[rstest]
fn test_remove_json_branch_only(repo: TestRepo) {
    repo.commit("initial");
    // Create a branch without a worktree (already merged into main)
    repo.git_command()
        .args(["branch", "orphan-branch"])
        .run()
        .unwrap();

    let output = repo
        .wt_command()
        .args([
            "remove",
            "orphan-branch",
            "--format=json",
            "--yes",
            "--foreground",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());

    assert_snapshot!(String::from_utf8_lossy(&output.stdout));
}

#[cfg(not(target_os = "windows"))]
#[rstest]
fn test_remove_json_multi_with_branch_only(mut repo: TestRepo) {
    repo.commit("initial");
    repo.add_worktree("wt-feature");
    // Create a branch without a worktree
    repo.git_command()
        .args(["branch", "orphan-branch"])
        .run()
        .unwrap();

    let output = repo
        .wt_command()
        .args([
            "remove",
            "wt-feature",
            "orphan-branch",
            "--format=json",
            "--yes",
            "--foreground",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());

    let mut settings = insta::Settings::clone_current();
    settings.add_filter(r#""path": "[^"]*""#, r#""path": "<PATH>""#);
    settings.bind(|| {
        assert_snapshot!(String::from_utf8_lossy(&output.stdout));
    });
}

/// Multi-remove with current worktree in the target list exercises the
/// `plans.current` JSON path (deferred removal, last in output).
#[cfg(not(target_os = "windows"))]
#[rstest]
fn test_remove_json_multi_with_current(mut repo: TestRepo) {
    repo.commit("initial");
    repo.add_worktree("other-feature");
    let current_wt = repo.add_worktree("current-feature");

    let output = repo
        .wt_command()
        .args([
            "remove",
            "other-feature",
            "current-feature",
            "--format=json",
            "--yes",
            "--foreground",
        ])
        .current_dir(&current_wt)
        .output()
        .unwrap();
    assert!(output.status.success());

    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();
    let items = json.as_array().unwrap();
    assert_eq!(items.len(), 2);

    // other-feature removed first (plans.others), current-feature last (plans.current)
    assert_eq!(items[0]["branch"], "other-feature");
    assert_eq!(items[0]["kind"], "worktree");
    assert_eq!(items[1]["branch"], "current-feature");
    assert_eq!(items[1]["kind"], "worktree");
}

/// `branch_outcome` reports what execution did, not what the plan intended,
/// and names *why* the branch survived.
///
/// The planner sees `feature` at main's tip and intends to delete it. A
/// `pre-remove` hook then commits on the branch, so the SafeDelete's re-check
/// against fresh refs finds it unmerged and declines — the worktree goes, the
/// branch stays. Reporting the plan here would tell a script the branch was
/// deleted while it is still on disk, and reporting a bare `false` would leave
/// it unable to tell this from a retention it asked for.
#[rstest]
fn test_remove_json_branch_outcome_reflects_execution(mut repo: TestRepo) {
    use crate::common::wait_for_worktree_removed;

    repo.commit("initial");
    // Same commit as main — integrated, so the plan intends to delete it.
    let feature_wt = repo.add_worktree("feature");

    // Runs in the worktree being removed, after planning, and leaves the
    // worktree clean so the removal itself still succeeds. Resolved from the
    // invoking worktree's config (the repo root), so it needn't be committed.
    repo.write_project_config(
        r#"pre-remove = "printf raced > raced.txt && git add raced.txt && git commit -m raced""#,
    );

    let output = repo
        .wt_command()
        .args(["remove", "feature", "--format=json", "--yes"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr)
        .ansi_strip()
        .into_owned();
    assert!(output.status.success(), "remove should succeed:\n{stderr}");

    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();
    let items = json.as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["branch"], "feature");
    assert_eq!(
        items[0]["branch_outcome"], "retained_unmerged",
        "branch_outcome must report the declined deletion and its reason, not the plan's intent:\n{stderr}",
    );

    wait_for_worktree_removed(&feature_wt);
    repo.run_git(&["rev-parse", "--verify", "refs/heads/feature"]);
    let tip = repo.git_output(&["show", "--format=", "--name-only", "refs/heads/feature"]);
    assert!(
        tip.lines().any(|line| line == "raced.txt"),
        "the hook's commit must be the branch tip, or the divergence never happened:\n{tip}",
    );

    assert!(
        stderr.contains("Removed worktree but kept branch feature (not integrated)"),
        "the surviving branch must be surfaced, not left silent:\n{stderr}",
    );
}

/// A retention the caller asked for reads differently from one a guard forced.
///
/// This is the contrast the old `branch_deleted` boolean could not draw: both
/// this run and the declined deletion above left the branch standing and
/// reported `false`, so an orchestrator could not tell "I asked you to keep
/// it" from "I could not safely delete it". `--no-delete-branch` never
/// attempts a deletion, so nothing is retained — there is no outcome to name.
#[rstest]
fn test_remove_json_branch_outcome_distinguishes_requested_retention(mut repo: TestRepo) {
    repo.commit("initial");
    repo.add_worktree("feature");

    let output = repo
        .wt_command()
        .args([
            "remove",
            "feature",
            "--no-delete-branch",
            "--format=json",
            "--yes",
            "--foreground",
        ])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr)
        .ansi_strip()
        .into_owned();
    assert!(output.status.success(), "remove should succeed:\n{stderr}");

    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();
    let items = json.as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["branch"], "feature");
    assert_eq!(
        items[0]["branch_outcome"], "not_attempted",
        "a retention the caller asked for is not a deletion that was refused:\n{stderr}",
    );

    repo.run_git(&["rev-parse", "--verify", "refs/heads/feature"]);
}

/// The detached legacy fallback also corrects a broken deletion promise.
///
/// When the rename-into-trash fast path fails, the removal falls back to a
/// detached `git worktree remove`, and the branch deletion becomes a CAS
/// shell tail — built in the foreground. A `pre-remove` hook that advances
/// the branch makes the integration re-check decline the tail, so the branch
/// definitively survives while the progress message already promised
/// "worktree & branch". That survival must be warned and reported, exactly as
/// on the fast path.
///
/// The fast path is forced to fail portably by planting a *file* where the
/// trash directory belongs: `stage_worktree_removal`'s `create_dir_all` fails
/// (ignored) and the rename into a non-directory fails on every OS.
#[rstest]
fn test_remove_fallback_warns_when_no_cas_tail(mut repo: TestRepo) {
    repo.commit("initial");
    let feature_wt = repo.add_worktree("feature");

    // Occupy the trash path with a file so the rename-into-trash fast path
    // cannot stage, forcing the detached-fallback arm.
    let wt_dir = repo.root_path().join(".git").join("wt");
    std::fs::create_dir_all(&wt_dir).unwrap();
    std::fs::write(wt_dir.join("trash"), b"not a directory").unwrap();

    // Same divergence as the fast-path test above: planner sees `feature`
    // integrated, the hook then commits on it, the re-check declines.
    repo.write_project_config(
        r#"pre-remove = "printf raced > raced.txt && git add raced.txt && git commit -m raced""#,
    );

    let output = repo
        .wt_command()
        .args(["remove", "feature", "--format=json", "--yes"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr)
        .ansi_strip()
        .into_owned();
    assert!(output.status.success(), "remove should succeed:\n{stderr}");

    assert!(
        stderr.contains("Removed worktree but kept branch feature (not integrated)"),
        "the fallback must correct the deletion promise, not stay silent:\n{stderr}",
    );
    // Discriminates fallback from fast path (which prints the same warning):
    // staging would have replaced the planted file with a real trash
    // directory, so the file surviving proves the rename never staged.
    assert!(
        wt_dir.join("trash").is_file(),
        "the planted trash file should have kept the fast path from staging",
    );
    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();
    assert_eq!(
        json.as_array().unwrap()[0]["branch_outcome"],
        "retained_unmerged",
        "a survival known in the foreground is not a deferral:\n{stderr}",
    );
    // The branch survives with the hook's commit as its tip; the worktree
    // directory itself is the detached process's job, so it isn't asserted.
    repo.run_git(&["rev-parse", "--verify", "refs/heads/feature"]);
    let tip = repo.git_output(&["show", "--format=", "--name-only", "refs/heads/feature"]);
    assert!(
        tip.lines().any(|line| line == "raced.txt"),
        "the hook's commit must be the branch tip, or the divergence never happened:\n{tip}",
    );
    let _ = feature_wt;
}

/// Regression: integration check ORs over local AND upstream. A branch merged
/// into LOCAL `main` must still be detected as integrated when `main` and
/// `origin/main` have diverged — symmetric to
/// `test_remove_squash_merged_on_remote_when_local_main_diverged`, which
/// covers the merged-on-remote side.
#[rstest]
fn test_remove_merged_locally_when_upstream_diverged(#[from(repo_with_remote)] repo: TestRepo) {
    let remote_path = repo.remote_path().unwrap().to_path_buf();

    // Advance origin/main with a remote-only commit so local and upstream diverge.
    let github_sim = repo.home_path().join("github-sim-local-merge");
    repo.run_git_in(
        repo.home_path(),
        &[
            "clone",
            remote_path.to_str().unwrap(),
            "github-sim-local-merge",
        ],
    );
    std::fs::write(github_sim.join("remote-only.txt"), "remote only").unwrap();
    repo.run_git_in(&github_sim, &["add", "remote-only.txt"]);
    repo.run_git_in(&github_sim, &["commit", "-m", "Remote-only main commit"]);
    repo.run_git_in(&github_sim, &["push", "origin", "main"]);

    // Create a feature branch off the original main and merge it locally
    // (no fetch yet, so local main moves while origin/main stays at the
    // original tip in our local view).
    repo.run_git(&["checkout", "-b", "feature-local-merge"]);
    std::fs::write(repo.root_path().join("feature.txt"), "feature").unwrap();
    repo.run_git(&["add", "feature.txt"]);
    repo.run_git(&["commit", "-m", "Add feature"]);
    repo.run_git(&["checkout", "main"]);
    repo.run_git(&["merge", "--ff-only", "feature-local-merge"]);

    // Now fetch — origin/main holds the remote commit, local main holds the
    // feature merge, neither is an ancestor of the other.
    repo.run_git(&["fetch", "origin"]);

    let local_main = repo.git_output(&["rev-parse", "main"]);
    let origin_main = repo.git_output(&["rev-parse", "origin/main"]);
    assert_ne!(
        local_main, origin_main,
        "main and origin/main should differ"
    );
    assert!(
        !repo
            .git_command()
            .args(["merge-base", "--is-ancestor", "main", "origin/main"])
            .run()
            .unwrap()
            .status
            .success(),
        "local main should not be an ancestor of origin/main"
    );
    assert!(
        !repo
            .git_command()
            .args(["merge-base", "--is-ancestor", "origin/main", "main"])
            .run()
            .unwrap()
            .status
            .success(),
        "origin/main should not be an ancestor of local main"
    );

    // Remove the feature branch — should detect integration via local main
    // even though origin/main does not contain the feature commit.
    let output = make_snapshot_cmd(&repo, "remove", &["feature-local-merge"], None)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr)
        .ansi_strip()
        .into_owned();
    assert!(
        output.status.success(),
        "remove should succeed\nstderr:\n{stderr}",
    );
    assert!(
        !stderr.contains("Branch unmerged"),
        "integration check must detect local merges, not just upstream\nstderr:\n{stderr}",
    );

    let branch_still_exists = repo
        .git_command()
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            "refs/heads/feature-local-merge",
        ])
        .run()
        .unwrap();
    assert!(
        !branch_still_exists.status.success(),
        "feature branch should be deleted after detection via local main"
    );
}

// ============================================================================
// Shared-branch retention
//
// A branch reaches two worktrees only through `git worktree add --force`, which
// worktrunk never runs itself. Once it has, deleting the ref orphans whichever
// checkout wt didn't remove: `git update-ref -d` is a compare-and-swap on the
// ref alone and, unlike `git branch -d`, doesn't refuse a ref that's checked
// out. The survivor is left at a null OID with an unresolvable `HEAD`, so every
// test here asserts on the survivor's `HEAD`, not just on the branch.
// ============================================================================

/// Check out `branch` a second time at `<repo>.<suffix>`, the state only
/// `--force` can produce.
fn add_force_duplicate(repo: &TestRepo, branch: &str, suffix: &str) -> std::path::PathBuf {
    let dup = repo
        .root_path()
        .parent()
        .unwrap()
        .join(format!("repo.{suffix}"));
    repo.run_git(&["worktree", "add", "--force", dup.to_str().unwrap(), branch]);
    dup
}

/// Assert `worktree` still resolves `HEAD` to a commit — the corruption a
/// deleted-but-checked-out branch leaves behind.
#[track_caller]
fn assert_not_orphaned(repo: &TestRepo, worktree: &std::path::Path, context: &str) {
    let head = repo
        .git_command()
        .args(["rev-parse", "--verify", "HEAD"])
        .current_dir(worktree)
        .run()
        .unwrap();
    assert!(
        head.status.success(),
        "surviving worktree must resolve HEAD to a commit, not a deleted branch\n{context}",
    );
}

#[track_caller]
fn assert_branch_exists(repo: &TestRepo, branch: &str, expected: bool, context: &str) {
    let found = repo
        .git_command()
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .run()
        .unwrap()
        .status
        .success();
    assert_eq!(found, expected, "branch {branch} presence\n{context}");
}

/// Run `wt remove` with `args`, returning ANSI-stripped stderr.
fn run_remove(repo: &TestRepo, args: &[&str]) -> String {
    let output = repo.wt_command().arg("remove").args(args).output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr)
        .ansi_strip()
        .into_owned();
    assert!(
        output.status.success(),
        "wt remove {args:?} should succeed\nstderr:\n{stderr}",
    );
    stderr
}

/// Naming a duplicate by its path removes exactly that worktree — resolving the
/// path back to a branch would target git's first-listed checkout instead — and
/// retains the branch the survivor still holds.
#[rstest]
fn test_remove_duplicate_checkout_by_path_retains_survivor(mut repo: TestRepo) {
    use crate::common::wait_for_worktree_removed;

    let survivor = repo.add_worktree("feature");
    let dup = add_force_duplicate(&repo, "feature", "feature-dup");

    let stderr = run_remove(&repo, &[dup.to_str().unwrap()]);

    wait_for_worktree_removed(&dup);
    assert!(
        survivor.exists(),
        "only the named worktree should be removed\nstderr:\n{stderr}",
    );
    assert_branch_exists(&repo, "feature", true, &stderr);
    assert_not_orphaned(&repo, &survivor, &stderr);
    assert!(
        stderr.contains("retained") && stderr.contains("still checked out"),
        "output should explain the branch was retained\nstderr:\n{stderr}",
    );
}

/// Removing a duplicated branch *by name* resolves to git's first-listed
/// worktree and removes that one, still retaining the shared branch rather than
/// orphaning the other checkout.
#[rstest]
fn test_remove_duplicate_checkout_by_name_retains_branch(mut repo: TestRepo) {
    use crate::common::wait_for_worktree_removed;

    let first = repo.add_worktree("feature");
    let dup = add_force_duplicate(&repo, "feature", "feature-dup");

    let stderr = run_remove(&repo, &["feature"]);

    wait_for_worktree_removed(&first);
    assert_branch_exists(&repo, "feature", true, &stderr);
    assert_not_orphaned(&repo, &dup, &stderr);
}

/// `-D` overrides every other retention wt has, but it can't override this one:
/// the ref is live in another worktree, so honoring it would corrupt that
/// worktree. The refusal warns rather than passing silently.
///
/// `--foreground` so the retention is also exercised on the synchronous
/// removal path; the other tests here take the background one.
#[rstest]
fn test_remove_force_delete_refused_while_branch_is_shared(mut repo: TestRepo) {
    let survivor = repo.add_worktree("feature");
    let dup = add_force_duplicate(&repo, "feature", "feature-dup");

    let stderr = run_remove(&repo, &[dup.to_str().unwrap(), "-D", "--foreground"]);

    assert!(
        !dup.exists(),
        "foreground removal should finish before returning\nstderr:\n{stderr}",
    );
    assert_branch_exists(&repo, "feature", true, &stderr);
    assert_not_orphaned(&repo, &survivor, &stderr);
    assert!(
        stderr.contains("retained despite -D"),
        "a refused -D must say so, not retain silently\nstderr:\n{stderr}",
    );
}

/// The missing-directory fallback reaches the same ref deletion, so it needs the
/// same guard: the target's own entry is stale, but the sibling's checkout is
/// live and would be orphaned.
#[rstest]
fn test_remove_pruned_dir_with_sibling_checkout_retains_branch(mut repo: TestRepo) {
    // `feature` has no commits beyond main, so it's integrated and would be
    // deleted by the branch-only fallback absent the sibling guard.
    let survivor = repo.add_worktree("feature");
    let dup = add_force_duplicate(&repo, "feature", "feature-dup");
    std::fs::remove_dir_all(&dup).unwrap();

    let stderr = run_remove(&repo, &[dup.to_str().unwrap()]);

    assert_branch_exists(&repo, "feature", true, &stderr);
    assert_not_orphaned(&repo, &survivor, &stderr);
    assert!(
        stderr.contains("pruned")
            && stderr.contains("retained")
            && stderr.contains("still checked out"),
        "output should report the prune and explain the branch was retained\nstderr:\n{stderr}",
    );
}

/// The mirror image: a *stale* duplicate entry alongside one live checkout. The
/// live checkout is the one being removed, so nothing survives to be orphaned
/// and the branch is deleted as usual. Retaining here would strand the branch
/// and name a directory that no longer exists.
#[rstest]
fn test_remove_last_live_checkout_deletes_branch(mut repo: TestRepo) {
    use crate::common::wait_for_worktree_removed;

    let live = repo.add_worktree("feature");
    let stale = add_force_duplicate(&repo, "feature", "feature-dup");
    std::fs::remove_dir_all(&stale).unwrap();

    let stderr = run_remove(&repo, &[live.to_str().unwrap()]);

    wait_for_worktree_removed(&live);
    assert_branch_exists(&repo, "feature", false, &stderr);
    assert!(
        !stderr.contains("still checked out"),
        "a stale entry is not a checkout to retain the branch for\nstderr:\n{stderr}",
    );
}

/// Planning sees one checkout, then an approved `pre-remove` hook creates a
/// duplicate. The final topology guard must retain the branch and report the
/// actual checkout race, not mislabel it as ref movement or failed integration.
#[rstest]
fn test_pre_remove_hook_new_checkout_retains_branch(mut repo: TestRepo) {
    let survivor = repo
        .root_path()
        .parent()
        .unwrap()
        .join("repo.feature-hook-survivor");
    let hook = "git worktree add --force ../repo.feature-hook-survivor feature-hook-checkout";
    repo.write_project_config(&format!("pre-remove = {hook:?}"));
    repo.commit("Add config");
    repo.write_test_approvals(&format!(
        r#"[projects."../origin"]
approved-commands = [{hook:?}]
"#
    ));
    let removed = repo.add_worktree("feature-hook-checkout");

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        assert_cmd_snapshot!(
            "remove_pre_remove_hook_new_checkout_retains_branch",
            make_snapshot_cmd(
                &repo,
                "remove",
                &["--foreground", "feature-hook-checkout"],
                None,
            )
        );
    });

    assert!(
        !removed.exists(),
        "the originally planned worktree is removed"
    );
    assert_branch_exists(&repo, "feature-hook-checkout", true, "snapshot above");
    assert_not_orphaned(&repo, &survivor, "snapshot above");
}

/// Assert git still tracks `branch`'s worktree — that its `.git/worktrees/<id>`
/// admin dir survived a neighbouring removal.
fn assert_still_registered(repo: &TestRepo, branch: &str) {
    let listed = repo
        .git_command()
        .args(["worktree", "list", "--porcelain"])
        .run()
        .unwrap();
    let listed = String::from_utf8_lossy(&listed.stdout);
    assert!(
        listed.contains(&format!("branch refs/heads/{branch}")),
        "worktree for `{branch}` must still be registered\n{listed}"
    );
}

/// Displace a worktree's directory, as an unmounted volume or a half-finished
/// `mv` would, and return the path it was parked at.
fn displace(worktree: &std::path::Path) -> std::path::PathBuf {
    let parked = worktree.parent().unwrap().join("displaced-elsewhere");
    std::fs::rename(worktree, &parked).unwrap();
    parked
}

/// A removal clears the metadata for the worktree it was asked to remove, and
/// for no other.
///
/// `git worktree prune` takes no path filter: it unregisters every entry whose
/// directory it cannot find at that instant. A sibling that is merely absent is
/// indistinguishable from a deleted one, so a repo-wide sweep would take its
/// `.git/worktrees/<id>` admin dir too — discarding the index, `ORIG_HEAD`, the
/// per-worktree refs, and any in-progress rebase. `git worktree repair` cannot
/// rebuild those, so the removal has to name its target.
#[rstest]
fn test_remove_spares_absent_sibling(mut repo: TestRepo) {
    let victim = repo.add_worktree("victim");
    let bystander = repo.add_worktree("bystander");
    let parked = displace(&bystander);

    run_remove(&repo, &["--foreground", "victim"]);
    assert!(!victim.exists(), "the named worktree should be gone");

    // The bystander's volume comes back.
    std::fs::rename(&parked, &bystander).unwrap();
    assert_still_registered(&repo, "bystander");
}

/// The same guarantee on the missing-directory route: `wt remove` on a worktree
/// whose directory is already gone degrades to a branch-only deletion, and the
/// stale-metadata cleanup that precedes it must also spare a displaced sibling.
#[rstest]
fn test_remove_stale_entry_spares_absent_sibling(mut repo: TestRepo) {
    let victim = repo.add_worktree("victim");
    let bystander = repo.add_worktree("bystander");
    std::fs::remove_dir_all(&victim).unwrap();
    let parked = displace(&bystander);

    run_remove(&repo, &["--foreground", "victim"]);

    std::fs::rename(&parked, &bystander).unwrap();
    assert_still_registered(&repo, "bystander");

    // The entry that was actually targeted is the one that went.
    let listed = repo
        .git_command()
        .args(["worktree", "list", "--porcelain"])
        .run()
        .unwrap();
    let listed = String::from_utf8_lossy(&listed.stdout);
    assert!(
        !listed.contains("branch refs/heads/victim"),
        "the stale entry should have been pruned\n{listed}"
    );
}
