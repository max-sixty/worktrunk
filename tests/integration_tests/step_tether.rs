//! Integration tests for `wt step tether`.
//!
//! `tether` runs a command in its own process group and tears the whole group
//! down (a) when its worktree is removed and (b) when the command exits on its
//! own. The supervised snippet records its process-group id (the leader's pid,
//! since `tether` puts the child in a new group) and backgrounds a second
//! sleep, so the assertions prove the whole group dies, including the
//! reparented sidecar a parent-walk would miss.
#![cfg(unix)]

use std::path::Path;
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use crate::common::{TestRepo, repo, wait_for_file_content};
use nix::errno::Errno;
use nix::sys::signal::kill;
use nix::unistd::Pid;
use rstest::rstest;

/// True while any process in group `pgid` still exists. `kill(-pgid, 0)` is the
/// portable group-liveness probe: `ESRCH` once the group is empty.
fn group_alive(pgid: i32) -> bool {
    !matches!(kill(Pid::from_raw(-pgid), None), Err(Errno::ESRCH))
}

/// Poll `cond` with a generous cap and fast interval (per tests/CLAUDE.md:
/// long timeout for slow CI, fast polling so it returns immediately when the
/// event has happened).
fn wait_until(mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    cond()
}

/// Spawn `wt step tether` supervising a snippet that records its pgid to
/// `pidfile`, backgrounds a sidecar sleep, then blocks. Returns the child
/// handle and the pgid once recorded.
fn spawn_tether(repo: &TestRepo, cwd: &Path, pidfile: &Path, tail: &str) -> (Child, i32) {
    let snippet = format!(
        "printf %s \"$$\" > \"{}\"; sleep 600 & {tail}",
        pidfile.display()
    );
    let child = repo
        .wt_command()
        .args(["step", "tether", "--", "sh", "-c", &snippet])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn wt step tether");

    wait_for_file_content(pidfile);
    let pgid: i32 = std::fs::read_to_string(pidfile)
        .unwrap()
        .trim()
        .parse()
        .expect("pgid in pidfile");
    (child, pgid)
}

/// Both ways a worktree disappears tear the whole group down: outright
/// deletion (`rm -rf` / `git worktree remove`, `NOTE_DELETE`/`IN_DELETE_SELF`)
/// and rename into trash (worktrunk's own removal, `NOTE_RENAME`/
/// `IN_MOVE_SELF`).
#[rstest]
fn test_tether_kills_process_group_when_worktree_removed(mut repo: TestRepo) {
    for mechanism in ["delete", "rename"] {
        let worktree = repo.add_worktree(&format!("server-{mechanism}"));
        let pidfile = repo.home_path().join(format!("tether-{mechanism}.pgid"));

        let (mut child, pgid) = spawn_tether(&repo, &worktree, &pidfile, "exec sleep 600");
        assert!(
            group_alive(pgid),
            "[{mechanism}] supervised group should be alive before removal"
        );

        match mechanism {
            "delete" => std::fs::remove_dir_all(&worktree).unwrap(),
            "rename" => std::fs::rename(&worktree, repo.home_path().join("trash")).unwrap(),
            _ => unreachable!(),
        }

        let group_gone = wait_until(|| !group_alive(pgid));
        let supervisor_exited = wait_until(|| child.try_wait().unwrap().is_some());

        // Clean up before asserting so a failure can't leak the sleeps.
        let _ = kill(Pid::from_raw(-pgid), nix::sys::signal::Signal::SIGKILL);
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(repo.home_path().join("trash"));

        assert!(
            group_gone,
            "[{mechanism}] removing the worktree must kill the whole process group"
        );
        assert!(
            supervisor_exited,
            "[{mechanism}] the tether supervisor must exit after teardown"
        );
    }
}

/// When the supervised command exits on its own, the supervisor sweeps the
/// group (so the backgrounded sidecar dies too) and then exits.
#[rstest]
fn test_tether_kills_process_group_when_command_exits(repo: TestRepo) {
    let pidfile = repo.home_path().join("tether-selfexit.pgid");
    // Foreground `true` exits immediately; the sidecar `sleep 600` would
    // outlive it without the group sweep.
    let (mut child, pgid) = spawn_tether(&repo, repo.path(), &pidfile, "true");

    let group_gone = wait_until(|| !group_alive(pgid));
    let supervisor_exited = wait_until(|| child.try_wait().unwrap().is_some());

    let _ = kill(Pid::from_raw(-pgid), nix::sys::signal::Signal::SIGKILL);
    let _ = child.kill();
    let _ = child.wait();

    assert!(
        group_gone,
        "command self-exit must sweep the whole group, including the sidecar"
    );
    assert!(
        supervisor_exited,
        "the tether supervisor must exit after the command exits"
    );
}
