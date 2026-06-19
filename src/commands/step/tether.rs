//! `wt step tether` — run a command and guarantee its whole process tree dies.
//!
//! `tether -- CMD…` starts CMD in a new process group and supervises it. The
//! whole tree is torn down when CMD exits on its own, or the worktree the
//! command runs in is removed.
//!
//! The process group is the point. `npm run dev` spawns an esbuild
//! `--service` sidecar that does not listen on the dev-server port; a port- or
//! parent-based kill misses it. Putting CMD in its own group at spawn keeps
//! every descendant reachable for one teardown call.
//!
//! Removal detection is a poll, not a kqueue/inotify watch: a `symlink_metadata`
//! of one path is portable, has no arm-time race, behaves identically on every
//! platform, and registers no filesystem watcher — the proliferation of which
//! is the very problem this command exists to avoid. The 250ms interval bounds
//! teardown latency for an already-orphaned server, which no one observes.
//!
//! Fire and forget: no stop command, no rendezvous file, no `pre-remove` hook.
//! State lives only in this supervisor process, which dies with CMD.
//!
//! Platform teardown differs. Unix sweeps the process group with a bounded
//! `SIGTERM` → `SIGKILL` escalation (`killpg`), which still reaches a child
//! that reparented to PID 1 after CMD exited. Windows has no killable process
//! group, so `taskkill /T /F` terminates CMD's process tree; it reaches every
//! descendant while CMD is alive (the dominant "worktree removed while the
//! server runs" case), but a child that detaches *after* CMD self-exits can
//! outlive it — Windows neither reparents to a single init nor offers a pgid.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use worktrunk::trace::CommandTrace;

/// How often the reaper re-checks whether the worktree still exists. Teardown
/// of an already-orphaned server within this bound is imperceptible next to a
/// dev server's own startup.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Run `command` supervised; tear its whole process tree down when the command
/// exits or its worktree is removed.
pub(crate) fn step_tether(command: &[String]) -> Result<()> {
    // post-start hooks run with cwd at the worktree root; capture it now so
    // the reaper can notice the worktree being removed. `None` (cwd
    // unavailable) degrades to "tear down only on the command's own exit",
    // never a false teardown.
    let worktree = std::env::current_dir().ok();

    let mut cmd = std::process::Command::new(&command[0]);
    cmd.args(&command[1..]);
    // Direct exec, no implicit shell, same as `wt step for-each`; scrub the
    // directive env vars so a long-lived child can't write to the parent
    // shell's cd/exec directive files.
    worktrunk::shell_exec::scrub_directive_env_vars(&mut cmd);
    set_new_process_group(&mut cmd);
    let mut trace = CommandTrace::new(None, &command.join(" "));
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            trace.fail(&e);
            return Err(e).with_context(|| format!("spawn tethered command: {}", command[0]));
        }
    };
    let id = child.id();

    // Reaper: polls until the worktree is gone, then kills the tree so the
    // supervised child exits and `wait` returns. Only spawned when there is a
    // worktree to watch; otherwise teardown relies solely on the command's
    // own exit.
    if let Some(dir) = worktree {
        std::thread::spawn(move || {
            while !worktree_gone(&dir) {
                std::thread::sleep(POLL_INTERVAL);
            }
            kill_process_tree(id);
        });
    }

    // Block until the child exits — killed by the reaper, or on its own.
    match child.wait() {
        Ok(status) => trace.complete(status.success()),
        Err(e) => trace.fail(&e),
    }

    // Final sweep: on a self-exit, descendants may still be running (Unix:
    // reparented but in this pgid; Windows: while the tree is intact). On a
    // reaper-driven teardown the tree is already gone and this is a no-op.
    kill_process_tree(id);

    // A reaper still sleeping between polls (the command self-exited) dies
    // with this process when it returns.
    Ok(())
}

/// The worktree path no longer resolves. Only `NotFound` counts as gone; a
/// transient stat error (`EACCES`, `EIO`) is not a removal, so the reaper
/// keeps waiting rather than tearing down a live server.
fn worktree_gone(worktree: &Path) -> bool {
    matches!(
        std::fs::symlink_metadata(worktree),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound
    )
}

/// Put the child at the head of a new process group so a single teardown call
/// reaches every descendant, including ones that reparent away.
#[cfg(unix)]
fn set_new_process_group(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;

    // POSIX `setpgid(0, 0)`: the child's pid is the new group's id, so
    // `child.id()` is the pgid. Children that do not re-`setpgid` (node,
    // Vite, the esbuild sidecar) stay in the group. Same invariant the
    // foreground runner relies on (`shell_exec.rs`).
    cmd.process_group(0);
}

#[cfg(windows)]
fn set_new_process_group(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;

    // CREATE_NEW_PROCESS_GROUP (0x00000200): the child leads a new process
    // group. Windows has no killable group like a Unix pgid, but the new
    // group still detaches CMD from the parent's console so its tree can be
    // terminated as a unit. Same flag `commands/process.rs` uses.
    cmd.creation_flags(0x0000_0200);
}

/// Tear down the whole tree rooted at the supervised child.
#[cfg(unix)]
fn kill_process_tree(pid: u32) {
    // `process_group(0)` made the child its own group leader, so the pid is
    // the pgid. The bounded TERM → KILL escalation reaches every member,
    // including a child that reparented to PID 1 after the leader exited.
    worktrunk::shell_exec::forward_signal_with_escalation(pid as i32, signal_hook::consts::SIGTERM);
}

#[cfg(windows)]
fn kill_process_tree(pid: u32) {
    // No killable process group on Windows; `taskkill /T` terminates the
    // process and its child tree, `/F` forces. Best-effort: the pid may
    // already be gone (self-exit) or have left detached children.
    let _ = std::process::Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}
