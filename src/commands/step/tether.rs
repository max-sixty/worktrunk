//! `wt step tether` — run a command and guarantee its whole process tree dies.
//!
//! `tether -- CMD…` starts CMD in a new process group (CMD's pid is the group
//! id) and supervises it. The whole group is torn down with a bounded
//! `SIGTERM` → `SIGKILL` escalation when either CMD exits on its own, or the
//! worktree the command runs in is removed.
//!
//! Process-group teardown (not a parent-walk) is the point: when CMD exits on
//! its own, its descendants reparent to PID 1 but keep the original
//! process-group id, so `killpg` still reaches an otherwise-orphaned child
//! (e.g. an esbuild `--service` sidecar of a Vite dev server). A `pgrep -P`
//! parent-walk cannot, because the parent link is gone.
//!
//! Fire and forget: there is no stop command and no rendezvous file. The
//! supervisor polls its own worktree directory (the cwd at launch, which is
//! the worktree root for a `post-start` hook). worktrunk removes a worktree by
//! renaming it into a trash directory; `git worktree remove` and `rm -rf`
//! delete it outright. `symlink_metadata` stops resolving in every case, so
//! the orphaned server dies with its worktree without any hook beyond the
//! single `post-start` line.
//!
//! Polling, not a kqueue/inotify watch: a stat of one path is portable, has no
//! arm-time race, behaves identically on every platform, and registers no
//! filesystem watcher — the proliferation of which is the very problem this
//! command exists to avoid. The poll interval bounds teardown latency for an
//! already-orphaned server, which no one observes.
//!
//! State lives only in this supervisor process, which dies with CMD.
//!
//! Unix only: process groups and `killpg` have no Windows equivalent here; the
//! Windows stub errors out.

#[cfg(unix)]
mod imp {
    use std::os::unix::process::CommandExt;
    use std::path::Path;
    use std::time::Duration;

    use anyhow::{Context, Result};

    /// How often the reaper re-checks whether the worktree still exists.
    /// Teardown of an already-orphaned server within this bound is
    /// imperceptible next to a dev server's own startup.
    const POLL_INTERVAL: Duration = Duration::from_millis(250);

    /// Run `command` supervised; tear its whole process group down when the
    /// command exits or its worktree is removed.
    pub(crate) fn step_tether(command: &[String]) -> Result<()> {
        // post-start hooks run with cwd at the worktree root; capture it now
        // so the reaper can notice the worktree being removed. `None` (cwd
        // unavailable) degrades to "tear down only on the command's own
        // exit", never a false teardown.
        let worktree = std::env::current_dir().ok();

        let mut cmd = std::process::Command::new(&command[0]);
        cmd.args(&command[1..]);
        // Direct exec, no implicit shell, same as `wt step for-each`; scrub
        // the directive env vars so a long-lived child can't write to the
        // parent shell's cd/exec directive files.
        worktrunk::shell_exec::scrub_directive_env_vars(&mut cmd);
        // `process_group(0)` => the child leads a new process group whose id
        // is its own pid (POSIX `setpgid(0, 0)` semantics), so `child.id()` is
        // the pgid and `killpg` takes the whole tree. Children that do not
        // re-`setpgid` (node, Vite, the esbuild sidecar) stay in this group.
        // Same invariant the foreground runner relies on (`shell_exec.rs`).
        cmd.process_group(0);
        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawn tethered command: {}", command[0]))?;
        let pgid = child.id() as i32;

        // Reaper: polls until the worktree is gone, then signals the group so
        // the supervised child exits and `wait` returns. Only spawned when
        // there is a worktree to watch; otherwise teardown relies solely on
        // the command's own exit.
        if let Some(dir) = worktree {
            std::thread::spawn(move || {
                while !worktree_gone(&dir) {
                    std::thread::sleep(POLL_INTERVAL);
                }
                let _ = nix::sys::signal::killpg(
                    nix::unistd::Pid::from_raw(pgid),
                    nix::sys::signal::Signal::SIGTERM,
                );
            });
        }

        // Block until the child exits — killed by the reaper, or on its own.
        let _ = child.wait();

        // Sweep the group with the shared bounded TERM → KILL escalation. On a
        // self-exit the children have reparented to PID 1 but kept this pgid,
        // so this still reaches them.
        worktrunk::shell_exec::forward_signal_with_escalation(pgid, signal_hook::consts::SIGTERM);

        // A reaper still sleeping between polls (the command self-exited) dies
        // with this process when it returns.
        Ok(())
    }

    /// The worktree path no longer resolves. Only `NotFound` counts as gone;
    /// a transient stat error (EACCES, EIO) is not a removal, so the reaper
    /// keeps waiting rather than tearing down a live server.
    fn worktree_gone(worktree: &Path) -> bool {
        matches!(
            std::fs::symlink_metadata(worktree),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound
        )
    }
}

#[cfg(not(unix))]
mod imp {
    use anyhow::{Result, bail};

    pub(crate) fn step_tether(_command: &[String]) -> Result<()> {
        bail!("`wt step tether` is only supported on Unix");
    }
}

pub(crate) use imp::step_tether;
