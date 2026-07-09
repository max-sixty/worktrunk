//! Discover and terminate processes whose working directory is under a
//! worktree, for `wt remove --reap` (experimental).
//!
//! # Purpose
//!
//! Removing a worktree leaves behind any long-running process started inside
//! it — a `post-start` dev server, a watcher, a language server — still
//! holding ports and file handles. `--reap` opts into terminating those
//! processes as part of removal.
//!
//! # Discovery
//!
//! Processes are discovered by working directory: any process whose `cwd` is
//! at or under the worktree path. `lsof -d cwd` reports every visible
//! process's cwd in one call; [`parse_lsof_cwd`] parses it and the caller
//! filters by path prefix. This is deliberately scoped to processes the
//! invoking user can see (no root), matching the `lsof` reliance already
//! established by [`super::fsmonitor`].
//!
//! # Data-safety contract
//!
//! Killing a process the user did not mean to kill — an editor with unsaved
//! buffers, an interactive shell — is exactly the silent loss-of-work the
//! project refuses without explicit consent. Two guards keep `--reap`
//! conservative:
//!
//! - **Controlling-terminal exclusion.** A process holding a controlling
//!   terminal is an interactive shell (including the one `wt remove` was run
//!   from) or a terminal editor (`vim`, `nvim`, `emacs -nw`). These are the
//!   "keep-me" set; `without_controlling_terminal` drops them via `ps -o
//!   tty=`, so only detached processes (dev servers, watchers, daemons) remain
//!   candidates.
//! - **Self-exclusion.** The current `wt` process is never a candidate.
//!
//! cwd-based discovery is also **under-inclusive by design**: a daemon that
//! forked and `chdir`'d away, or reparented to pid 1, no longer reports a cwd
//! under the path and is not found. Those are what [`wt step tether`] is built
//! to reap (it kills the whole process group). `--reap` and `tether` cover
//! different gaps and are not substitutes.
//!
//! # Platform
//!
//! Unix only. Windows has no cheap per-process cwd, so the whole module is
//! `#[cfg(unix)]` and the `wt remove` command rejects `--reap` there.
//!
//! [`wt step tether`]: https://worktrunk.dev/step/#wt-step-tether

#![cfg(unix)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::shell_exec::Cmd;

use super::fsmonitor::{NixSignaller, REAP_KILL_DEADLINE, escalate_terminate};

/// One process discovered under a worktree: its PID, short command name, and
/// the working directory `lsof` reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CwdProcess {
    pub pid: u32,
    /// Short command name from `lsof` (truncated to ~9 chars by `lsof`).
    pub command: String,
    pub cwd: PathBuf,
}

/// Timeout for the `lsof` / `ps` probes. Discovery is opt-in and off the hot
/// path, but a hung probe should still not stall removal indefinitely.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Parse `lsof -d cwd -F pcn` field output into one [`CwdProcess`] per process.
///
/// `lsof` field output emits one record per line, prefixed by a field id:
/// `p<pid>` opens a new process, `c<command>` names it, and `n<path>` gives the
/// cwd (there is exactly one cwd file per process, selected by `-d cwd`). A
/// process contributes a [`CwdProcess`] only once its `n` (cwd path) line is
/// seen; records missing a path are skipped.
pub fn parse_lsof_cwd(stdout: &str) -> Vec<CwdProcess> {
    let mut out = Vec::new();
    let mut pid: Option<u32> = None;
    let mut command = String::new();

    for line in stdout.lines() {
        let Some((tag, rest)) = line.split_at_checked(1) else {
            continue;
        };
        match tag {
            "p" => {
                pid = rest.trim().parse::<u32>().ok();
                command = String::new();
            }
            "c" => command = rest.to_string(),
            "n" => {
                if let Some(pid) = pid {
                    out.push(CwdProcess {
                        pid,
                        command: command.clone(),
                        cwd: PathBuf::from(rest),
                    });
                }
            }
            _ => {}
        }
    }
    out
}

/// Parse `ps -o pid=,tty=` output into `(pid, has_controlling_terminal)` pairs.
///
/// `ps` prints one line per PID: the PID, then the controlling terminal or a
/// "none" marker (`?` / `??` on Linux/macOS, `-` on some platforms). A terminal
/// name that starts with `?` or equals `-` means no controlling terminal.
pub fn parse_ps_tty(stdout: &str) -> Vec<(u32, bool)> {
    stdout
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse::<u32>().ok()?;
            let tty = fields.next().unwrap_or("?");
            let has_tty = !(tty.starts_with('?') || tty == "-");
            Some((pid, has_tty))
        })
        .collect()
}

/// Return the subset of `pids` that hold **no** controlling terminal.
///
/// Runs `ps -o pid=,tty=` over the candidate PIDs and keeps those whose
/// terminal is a "none" marker — the detached processes safe to reap. Any PID
/// `ps` does not report (it exited between discovery and this probe) is
/// dropped. A `ps` spawn or non-zero exit yields an empty set: without a
/// trustworthy terminal reading, nothing is reaped (fail-safe).
fn without_controlling_terminal(pids: &[u32]) -> HashSet<u32> {
    if pids.is_empty() {
        return HashSet::new();
    }
    let pid_list = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let Ok(output) = Cmd::new("ps")
        .args(["-o", "pid=,tty=", "-p", &pid_list])
        .timeout(PROBE_TIMEOUT)
        .run()
    else {
        return HashSet::new();
    };
    if !output.status.success() {
        return HashSet::new();
    }
    parse_ps_tty(&String::from_utf8_lossy(&output.stdout))
        .into_iter()
        .filter_map(|(pid, has_tty)| (!has_tty).then_some(pid))
        .collect()
}

/// Discover processes whose cwd is at or under `worktree_path`, excluding the
/// current `wt` process. This is the raw discovery step, *before* the
/// controlling-terminal guard — [`collect_reapable`] layers that on top.
///
/// Best-effort: an `lsof` spawn failure yields an empty list. `lsof -d cwd` is
/// run system-wide rather than with `+D <path>` so it never walks the
/// worktree's file tree; the prefix filter is applied here instead. Results
/// are sorted by PID for stable output.
pub fn processes_under(worktree_path: &Path) -> Vec<CwdProcess> {
    let canonical =
        dunce::canonicalize(worktree_path).unwrap_or_else(|_| worktree_path.to_path_buf());

    // `lsof -d cwd` lists every visible process's cwd. It exits non-zero when
    // some processes are inaccessible (other users), but still prints the
    // accessible ones — so parse whatever stdout we got rather than gating on
    // exit status. Only a spawn failure (lsof missing) means "no data".
    let Ok(output) = Cmd::new("lsof")
        .args(["-d", "cwd", "-F", "pcn"])
        .timeout(PROBE_TIMEOUT)
        .run()
    else {
        return Vec::new();
    };

    let self_pid = std::process::id();
    let mut candidates: Vec<CwdProcess> = parse_lsof_cwd(&String::from_utf8_lossy(&output.stdout))
        .into_iter()
        .filter(|p| p.pid != self_pid)
        .filter(|p| p.cwd.starts_with(&canonical))
        .collect();
    candidates.sort_by_key(|p| p.pid);
    candidates
}

/// Discover the processes eligible for reaping under `worktree_path`.
///
/// Applies the full data-safety contract: [`processes_under`] (cwd prefix,
/// self-exclusion) then drops any process holding a controlling terminal.
/// Returns candidates sorted by PID for stable output.
pub fn collect_reapable(worktree_path: &Path) -> Vec<CwdProcess> {
    let mut candidates = processes_under(worktree_path);
    if candidates.is_empty() {
        return candidates;
    }

    let pids: Vec<u32> = candidates.iter().map(|p| p.pid).collect();
    let reapable = without_controlling_terminal(&pids);
    candidates.retain(|p| reapable.contains(&p.pid));
    candidates
}

/// `SIGTERM`→wait→`SIGKILL` each PID, returning the count confirmed gone.
///
/// Thin wrapper over `escalate_terminate` with the production
/// `NixSignaller` and the shared `REAP_KILL_DEADLINE`, so `--reap` uses the
/// same bounded escalation as the fsmonitor sweep.
pub fn reap_pids(pids: &[u32]) -> usize {
    escalate_terminate(&NixSignaller, pids, REAP_KILL_DEADLINE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_lsof_cwd_extracts_pid_command_and_path() {
        let stdout = "\
p101
cnode
fcwd
n/home/user/repo.feature
p202
cesbuild
fcwd
n/home/user/repo.feature/node_modules/.bin
";
        let procs = parse_lsof_cwd(stdout);
        assert_eq!(
            procs,
            vec![
                CwdProcess {
                    pid: 101,
                    command: "node".into(),
                    cwd: PathBuf::from("/home/user/repo.feature"),
                },
                CwdProcess {
                    pid: 202,
                    command: "esbuild".into(),
                    cwd: PathBuf::from("/home/user/repo.feature/node_modules/.bin"),
                },
            ]
        );
    }

    #[test]
    fn parse_lsof_cwd_skips_process_without_cwd_line() {
        // A process whose cwd lsof could not read (no `n` line) is dropped.
        let stdout = "\
p101
cbash
p202
czsh
fcwd
n/home/user/repo.feature
";
        let procs = parse_lsof_cwd(stdout);
        assert_eq!(procs.len(), 1);
        assert_eq!(procs[0].pid, 202);
    }

    #[test]
    fn parse_ps_tty_classifies_terminal_presence() {
        // Linux `?`, macOS `??`, and `-` all mean "no controlling terminal";
        // `pts/2` / `s001` are real terminals.
        let stdout = "\
  101 ?
  202 pts/2
  303 ??
  404 s001
  505 -
";
        let ttys = parse_ps_tty(stdout);
        assert_eq!(
            ttys,
            vec![
                (101, false),
                (202, true),
                (303, false),
                (404, true),
                (505, false),
            ]
        );
    }

    #[test]
    fn parse_ps_tty_ignores_unparsable_lines() {
        let stdout = "\
header junk
  101 ?
not-a-pid tty
";
        assert_eq!(parse_ps_tty(stdout), vec![(101, false)]);
    }

    /// Read one PID's controlling-terminal state through the same `ps` probe
    /// the module uses, so the e2e assertion tracks the process's real TTY
    /// rather than assuming the test host has (or lacks) one.
    fn probe_has_tty(pid: u32) -> bool {
        !without_controlling_terminal(&[pid]).contains(&pid)
    }

    /// End-to-end against the real `lsof`/`ps`: a child whose cwd is under the
    /// path is discovered by [`processes_under`], correctly kept-or-dropped by
    /// the controlling-terminal guard in [`collect_reapable`], and terminated
    /// by [`reap_pids`].
    ///
    /// The child inherits whatever terminal the test process has, which varies
    /// (a TTY locally, none in CI). Rather than assume either, the guard's
    /// verdict is checked against the child's *actual* TTY state — so the test
    /// exercises both branches without becoming host-dependent.
    #[test]
    fn discovers_filters_and_kills_a_worktree_process() {
        use std::process::Command;
        use std::time::{Duration, Instant};

        let tmp = tempfile::tempdir().unwrap();
        let dir = dunce::canonicalize(tmp.path()).unwrap();

        let mut child = Command::new("sleep")
            .arg("30")
            .current_dir(&dir)
            .spawn()
            .unwrap();
        let pid = child.id();

        // lsof sees the cwd once the child is running; poll briefly.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let under = processes_under(&dir);
            if under.iter().any(|p| p.pid == pid) {
                // Only our child lives under this fresh tempdir.
                assert_eq!(under.iter().filter(|p| p.pid == pid).count(), 1);
                break;
            }
            assert!(
                Instant::now() < deadline,
                "child {pid} never discovered under {}",
                dir.display()
            );
            std::thread::sleep(Duration::from_millis(50));
        }

        // The controlling-terminal guard keeps the child iff it has no TTY.
        let kept = collect_reapable(&dir).iter().any(|p| p.pid == pid);
        assert_eq!(kept, !probe_has_tty(pid));

        // Reap the zombie concurrently: `sleep` is a direct child here, so
        // after SIGTERM it lingers as a zombie (still "alive" to `kill(pid,0)`)
        // until `wait()`. Real reap targets are detached, not `wt`'s children,
        // so `escalate_terminate` sees them vanish. A thread already blocked in
        // `wait()` reaps the zombie the instant it exits, letting the alive
        // check flip within a poll cycle.
        let reaper = std::thread::spawn(move || child.wait().unwrap());
        let gone = reap_pids(&[pid]);
        let status = reaper.join().unwrap();

        assert_eq!(gone, 1, "child {pid} was not confirmed terminated");
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(
            status.signal(),
            Some(nix::sys::signal::Signal::SIGTERM as i32)
        );
    }
}
