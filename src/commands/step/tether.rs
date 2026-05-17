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
//! supervisor watches its own worktree directory (the cwd at launch, which is
//! the worktree root for a `post-start` hook). worktrunk removes a worktree by
//! renaming it into a trash directory; `git worktree remove` and `rm -rf`
//! delete it outright. A kqueue `EVFILT_VNODE` (macOS) or inotify (Linux)
//! watch fires on the rename or the delete either way, so the orphaned server
//! dies with its worktree without any hook beyond the single `post-start`
//! line. An open fd does not pin a Unix directory entry, so the watch must be
//! explicit; nothing else signals the removal.
//!
//! State lives only in this supervisor process, which dies with CMD.
//!
//! Unix only: process groups and `killpg` have no Windows equivalent here; the
//! Windows stub errors out. The worktree watch covers macOS and Linux; on
//! other Unix the command is still supervised, but only its own exit tears the
//! group down.

#[cfg(unix)]
mod imp {
    use std::os::unix::process::CommandExt;
    use std::path::Path;

    use anyhow::{Context, Result};

    /// Run `command` supervised; tear its whole process group down when the
    /// command exits or its worktree is removed.
    pub(crate) fn step_tether(command: &[String]) -> Result<()> {
        // post-start hooks run with cwd at the worktree root; capture it now
        // so the reaper can notice the worktree being removed. `None` (or a
        // path the watch can't arm on) degrades to "tear down only on the
        // command's own exit", never a false teardown.
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

        // Reaper: blocks until the worktree is removed, then signals the group
        // so the supervised child exits and `wait` returns. Only spawned when
        // there is a worktree to watch; otherwise teardown relies solely on
        // the command's own exit.
        if let Some(dir) = worktree {
            std::thread::spawn(move || {
                await_worktree_gone(&dir);
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

        // A reaper still blocked on the watch (the command self-exited) dies
        // with this process when it returns.
        Ok(())
    }

    /// Never returns. The reaper calls this only when it genuinely cannot
    /// watch (kqueue/inotify instance creation failed), so an inability to
    /// arm is never mistaken for a removal and a live server is never falsely
    /// torn down. A *missing* worktree is not a setup failure: it means the
    /// removal already happened, so those paths return instead of parking.
    fn park_forever() -> ! {
        loop {
            std::thread::park();
        }
    }

    /// The worktree path no longer exists. The reaper arms its watch
    /// asynchronously; if the worktree is removed before (or during) arming,
    /// `open`/`add_watch` see `ENOENT` and the post-arm re-check sees this —
    /// the removal already happened, so the reaper must tear down rather than
    /// block on a watch that will never fire.
    fn worktree_gone(worktree: &Path) -> bool {
        matches!(
            std::fs::symlink_metadata(worktree),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound
        )
    }

    /// Block until `worktree` is deleted or renamed (worktrunk renames a
    /// removed worktree into trash, so `wt rm`, `git worktree remove`, and
    /// `rm -rf` all fire). A single non-recursive vnode watch, not an
    /// FSEvents hierarchy stream, so it does not reintroduce the watcher
    /// proliferation this command exists to avoid.
    #[cfg(target_os = "macos")]
    fn await_worktree_gone(worktree: &Path) {
        use std::os::fd::AsRawFd;

        use nix::errno::Errno;
        use nix::sys::event::{EvFlags, EventFilter, FilterFlag, KEvent, Kqueue};

        let dir = match std::fs::File::open(worktree) {
            Ok(dir) => dir,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(_) => park_forever(),
        };
        let Ok(kq) = Kqueue::new() else {
            park_forever()
        };
        let change = KEvent::new(
            dir.as_raw_fd() as usize,
            EventFilter::EVFILT_VNODE,
            EvFlags::EV_ADD | EvFlags::EV_CLEAR,
            FilterFlag::NOTE_DELETE | FilterFlag::NOTE_RENAME | FilterFlag::NOTE_REVOKE,
            0,
            0,
        );
        if kq.kevent(&[change], &mut [], None).is_err() {
            park_forever();
        }
        // kqueue only reports events that occur after EV_ADD registers, so a
        // removal between `open` and here would be missed and the wait would
        // block forever. Re-check now that the watch is armed.
        if worktree_gone(worktree) {
            return;
        }
        // `dir` stays in scope so the watched fd is open across the wait.
        // Return only on a real vnode event: a bare signal wakes `kevent`
        // with `EINTR`, and returning then would tear down a live server.
        let mut ev = [KEvent::new(
            0,
            EventFilter::EVFILT_VNODE,
            EvFlags::empty(),
            FilterFlag::empty(),
            0,
            0,
        )];
        loop {
            match kq.kevent(&[], &mut ev, None) {
                Ok(n) if n > 0 => return,
                Ok(_) | Err(Errno::EINTR) => continue,
                Err(_) => park_forever(),
            }
        }
    }

    /// Block until `worktree` is deleted or renamed. `IN_MOVE_SELF` covers
    /// worktrunk's rename-into-trash; `IN_DELETE_SELF` covers `rm -rf` and
    /// `git worktree remove`.
    #[cfg(target_os = "linux")]
    fn await_worktree_gone(worktree: &Path) {
        use nix::errno::Errno;
        use nix::sys::inotify::{AddWatchFlags, InitFlags, Inotify};

        let Ok(ino) = Inotify::init(InitFlags::empty()) else {
            park_forever()
        };
        match ino.add_watch(
            worktree,
            AddWatchFlags::IN_DELETE_SELF | AddWatchFlags::IN_MOVE_SELF,
        ) {
            Ok(_) => {}
            Err(Errno::ENOENT) => return,
            Err(_) => park_forever(),
        }
        // inotify queues events from the moment the watch is added, so a
        // removal after this point is captured; but one racing the add could
        // slip between the ENOENT check and here. Re-check now.
        if worktree_gone(worktree) {
            return;
        }
        // Return only on a real event: `read_events` is a raw `read` that
        // returns `EINTR` on signal interruption, and returning then would
        // tear down a live server.
        loop {
            match ino.read_events() {
                Ok(events) if !events.is_empty() => return,
                Ok(_) | Err(Errno::EINTR) => continue,
                Err(_) => park_forever(),
            }
        }
    }

    /// No worktree-removal watch on this platform; the command is still
    /// supervised, but only its own exit tears the group down.
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    fn await_worktree_gone(_worktree: &Path) {
        park_forever()
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
