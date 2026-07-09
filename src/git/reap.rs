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
///
/// This feeds a data-safety gate (a process *with* a terminal is spared), so
/// the unreadable case fails safe: a line missing the tty column is reported as
/// **having** a terminal, so an unparsable reading never turns into a reap.
/// Real `ps -o tty=` always fills the column, so this only guards the anomaly.
pub fn parse_ps_tty(stdout: &str) -> Vec<(u32, bool)> {
    stdout
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse::<u32>().ok()?;
            let has_tty = match fields.next() {
                Some(tty) => !(tty.starts_with('?') || tty == "-"),
                None => true,
            };
            Some((pid, has_tty))
        })
        .collect()
}

/// Return the subset of `pids` that hold **no** controlling terminal.
///
/// Runs `ps -o pid=,tty=` over the candidate PIDs and keeps those whose
/// terminal is a "none" marker — the detached processes safe to reap. A PID
/// `ps` does not report (it exited between discovery and this probe, or `ps`
/// exited non-zero because every requested PID was gone) simply doesn't appear
/// in the output, so it's dropped. A `ps` spawn failure yields an empty set:
/// without a terminal reading, nothing is reaped (fail-safe).
fn without_controlling_terminal(pids: &[u32]) -> HashSet<u32> {
    if pids.is_empty() {
        return HashSet::new();
    }
    let pid_list = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    // Parse whatever `ps` printed regardless of exit status: it exits non-zero
    // when some requested PIDs are gone but still lists the live ones (the same
    // partial-output shape `lsof` has in `processes_under`).
    let Ok(output) = Cmd::new("ps")
        .args(["-o", "pid=,tty=", "-p", &pid_list])
        .timeout(PROBE_TIMEOUT)
        .run()
    else {
        return HashSet::new();
    };
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

/// Pluralized noun for a process count (`"process"` / `"processes"`).
pub fn process_noun(count: usize) -> &'static str {
    if count == 1 { "process" } else { "processes" }
}

/// Human summary of a reap outcome. `Ok` when every process was terminated (the
/// caller renders it as success); `Err` when some survived both `SIGTERM` and
/// `SIGKILL` (rendered as a warning).
pub fn reap_summary(count: usize, gone: usize) -> Result<String, String> {
    let noun = process_noun(count);
    if gone >= count {
        Ok(format!("Reaped {count} {noun}"))
    } else {
        let survived = count - gone;
        Err(format!(
            "Reaped {gone} of {count} {noun}; {survived} ignored SIGTERM & SIGKILL"
        ))
    }
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
        // A process whose cwd lsof could not read (no `n` line) is dropped; a
        // blank line and unknown field tags (`f`) are ignored.
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
        // A pid-only line (no tty column) fails safe as "has terminal" so an
        // unreadable reading never becomes a reap candidate.
        let stdout = "\
header junk
  101 ?
  707
not-a-pid tty
";
        assert_eq!(parse_ps_tty(stdout), vec![(101, false), (707, true)]);
    }

    #[test]
    fn without_controlling_terminal_empty_input_is_empty() {
        assert!(without_controlling_terminal(&[]).is_empty());
    }

    #[test]
    fn discovery_on_nonexistent_path_is_empty() {
        // A path that can't be canonicalized falls back to itself; no live
        // process has a cwd under it, so both discovery layers return empty.
        let missing = Path::new("/nonexistent/worktrunk-reap-xyz");
        assert!(processes_under(missing).is_empty());
        assert!(collect_reapable(missing).is_empty());
    }

    #[test]
    fn reap_summary_reports_success_and_survivors() {
        assert_eq!(process_noun(1), "process");
        assert_eq!(process_noun(2), "processes");
        assert_eq!(reap_summary(1, 1), Ok("Reaped 1 process".into()));
        assert_eq!(reap_summary(3, 3), Ok("Reaped 3 processes".into()));
        assert_eq!(
            reap_summary(3, 1),
            Err("Reaped 1 of 3 processes; 2 ignored SIGTERM & SIGKILL".into())
        );
    }

    /// `reap_pids` against a real process: `SIGTERM` terminates it and the
    /// count comes back confirmed. Discovery and the controlling-terminal
    /// guard are covered in-process by the `test_remove_reap_kills_process`
    /// integration test (which calls `processes_under` / `collect_reapable`
    /// directly), so this focuses on the signalling half.
    #[test]
    fn reap_pids_terminates_a_process() {
        use std::process::Command;

        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id();

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
