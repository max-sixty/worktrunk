//! Deferred reporting for background hook-pipeline failures.
//!
//! `post-*` hooks run in a detached `wt hook run-pipeline` process whose
//! stdout/stderr are redirected into `.git/wt/logs/`, and the `wt` command that
//! spawned them has normally exited by the time a step fails. So when a
//! pipeline aborts, three things happen together and none of them reaches the
//! terminal: the remaining steps don't run, the originating command already
//! printed them as running, and its exit code was already 0. The only record is
//! the runner's own log, which a user has to already suspect something to go
//! looking for (issue #3858).
//!
//! This module is the channel that closes that gap. The runner appends one JSON
//! line per aborted pipeline to `.git/wt/hook-failures.jsonl`; the next
//! foreground `wt` invocation in that repo drains the file and prints one
//! warning per record, naming the step that failed and the steps its abort
//! skipped, before the command's own output.
//!
//! # Contracts
//!
//! - **Recording is a second channel, never the only one.** A failed pipeline
//!   still exits non-zero with its error in `runner.log`; [`record`] is
//!   best-effort on top of that, so an I/O failure here is swallowed rather
//!   than masking the original error.
//! - **Draining is destructive, so it moves the file aside first.** [`report`]
//!   renames the file to a pid-suffixed sibling before reading it: a record
//!   appended by a concurrently-failing pipeline lands in the fresh file rather
//!   than being deleted unread, and two `wt` processes draining at once can't
//!   claim the same records or clobber each other's snapshot.
//! - **Surfaces that suppress warnings don't drain.** The statusline, the
//!   pickers, and shell completion latch `config::suppress_warnings()`;
//!   draining there would consume the notice into a redraw nobody reads, or
//!   corrupt a TUI. The file is left for the next ordinary command.
//! - **A `wt` inside a background hook doesn't drain either** — its stderr is a
//!   log file, so the notice would land where the record already is.
//! - **Interrupts aren't failures.** A pipeline killed by SIGINT/SIGTERM
//!   records nothing, matching the project's convention that a user-initiated
//!   cancellation exits quietly.

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use color_print::cformat;
use serde::{Deserialize, Serialize};

use worktrunk::git::Repository;
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{eprintln, hint_message, warning_message};

/// File under `Repository::wt_dir()` holding pending failure records, one JSON
/// object per line.
const FAILURES_FILE: &str = "hook-failures.jsonl";

/// Most records reported on a single invocation. A repo whose `post-*` hooks
/// fail on every merge accumulates one record per merge until a foreground
/// `wt` runs; reporting all of them would bury the command the user actually
/// ran, so the oldest collapse into a count.
const MAX_REPORTED: usize = 5;

/// One aborted background pipeline.
///
/// Field names are short because this is written once per failure and read
/// once; the file is internal state, not a documented interface.
#[derive(Debug, Serialize, Deserialize)]
pub struct HookFailure {
    /// Hook type, rendered (`post-merge`).
    pub hook_type: String,
    /// Config source the pipeline came from (`user` / `project`).
    pub source: String,
    /// Branch the pipeline ran for, for the log path and for orientation when
    /// the failure surfaces from a different worktree.
    pub branch: String,
    /// Display name of the step that failed — its hook name, or the expanded
    /// command when the step is unnamed.
    pub failed: String,
    /// Exit code of the failed step. `None` when the pipeline aborted before
    /// running it (template expansion, log creation, spawn).
    pub exit: Option<i32>,
    /// Display names of the steps the abort skipped, in pipeline order.
    pub skipped: Vec<String>,
    /// Absolute path to the failed step's own output log, when it got far
    /// enough to have one.
    pub log: Option<PathBuf>,
}

impl HookFailure {
    /// The warning line: what failed, how, and what didn't run because of it.
    fn headline(&self) -> String {
        let source = &self.source;
        let hook_type = &self.hook_type;
        let branch = &self.branch;
        let failed = &self.failed;
        let outcome = match self.exit {
            Some(code) => format!("exited {code}"),
            None => "did not run".to_string(),
        };
        let skipped = if self.skipped.is_empty() {
            String::new()
        } else {
            let names = self
                .skipped
                .iter()
                .map(|n| cformat!("<bold>{n}</>"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("; skipped {names}")
        };
        cformat!(
            "Background <bold>{hook_type}</> hook for <bold>{branch}</> failed: <bold>{source}:{failed}</> {outcome}{skipped}"
        )
    }
}

/// Append a failure record for the next foreground `wt` to report.
///
/// Best-effort by contract (see module docs): every error is swallowed, since
/// the caller is already on its way to exiting non-zero with the real error.
/// The append is a single `writeln!` of one line, matching `command_log`'s
/// reliance on O_APPEND atomicity for concurrent writers.
pub fn record(wt_dir: &Path, failure: &HookFailure) {
    let Ok(mut line) = serde_json::to_string(failure) else {
        return;
    };
    line.push('\n');
    if fs::create_dir_all(wt_dir).is_err() {
        return;
    }
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(wt_dir.join(FAILURES_FILE))
    {
        let _ = file.write_all(line.as_bytes());
    }
}

/// Drain pending records and warn about each, newest last.
///
/// Called once per `wt` invocation, before the command's own output. Returns
/// without touching the filesystem on the surfaces that must not drain (see
/// module docs), so the notice survives to a command that can show it.
pub fn report(repo: &Repository) {
    if worktrunk::config::warnings_suppressed() || worktrunk::priority::in_background_hook() {
        return;
    }
    let failures = take_pending(&repo.wt_dir());
    let skipped_count = failures.len().saturating_sub(MAX_REPORTED);
    for failure in failures.iter().skip(skipped_count) {
        eprintln!("{}", warning_message(failure.headline()));
        if let Some(log) = &failure.log {
            eprintln!(
                "{}",
                hint_message(cformat!(
                    "Output @ <underline>{}</>",
                    format_path_for_display(log)
                ))
            );
        }
    }
    if skipped_count > 0 {
        eprintln!(
            "{}",
            hint_message(format!(
                "{skipped_count} earlier background hook failures not shown"
            ))
        );
    }
}

/// Claim the pending records, leaving the repo with none.
///
/// The rename is what makes this safe to interleave with a failing pipeline:
/// it's a single atomic step, so a record written after it lands in a fresh
/// file for the *next* invocation rather than in the window between reading and
/// deleting. The pid suffix keeps two concurrent `wt` processes from renaming
/// onto the same sibling — worth more than sweeping the sibling a mid-drain
/// SIGKILL would strand, since reclaiming another process's in-flight claim
/// would double-report. Malformed lines (a truncated write, a record from a
/// future version) are dropped rather than aborting the report.
///
/// No `wt config state` category clears the pending file, and none needs to:
/// draining runs on every invocation, so `wt config state clear` has already
/// reported and removed it by the time it looks.
fn take_pending(wt_dir: &Path) -> Vec<HookFailure> {
    let path = wt_dir.join(FAILURES_FILE);
    let claimed = wt_dir.join(format!("{FAILURES_FILE}.{}", std::process::id()));
    // Nothing pending is the overwhelmingly common case, and lands here as a
    // failed rename — one syscall on the startup path.
    if fs::rename(&path, &claimed).is_err() {
        return Vec::new();
    }
    let contents = fs::read_to_string(&claimed).unwrap_or_default();
    let _ = fs::remove_file(&claimed);
    contents
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure() -> HookFailure {
        HookFailure {
            hook_type: "post-merge".to_string(),
            source: "user".to_string(),
            branch: "main".to_string(),
            failed: "sync".to_string(),
            exit: Some(1),
            skipped: vec!["push".to_string()],
            log: None,
        }
    }

    #[test]
    fn headline_names_the_failed_step_and_the_skipped_ones() {
        let plain = anstream::adapter::strip_str(&failure().headline()).to_string();
        assert_eq!(
            plain,
            "Background post-merge hook for main failed: user:sync exited 1; skipped push"
        );
    }

    #[test]
    fn headline_omits_the_skipped_clause_when_nothing_was_skipped() {
        let mut f = failure();
        f.skipped.clear();
        let plain = anstream::adapter::strip_str(&f.headline()).to_string();
        assert_eq!(
            plain,
            "Background post-merge hook for main failed: user:sync exited 1"
        );
    }

    /// A pipeline that aborts before the step runs (template expansion, log
    /// creation, spawn) has no exit code to report.
    #[test]
    fn headline_reports_a_step_that_never_ran() {
        let mut f = failure();
        f.exit = None;
        let plain = anstream::adapter::strip_str(&f.headline()).to_string();
        assert_eq!(
            plain,
            "Background post-merge hook for main failed: user:sync did not run; skipped push"
        );
    }

    #[test]
    fn take_pending_drains_records_and_leaves_none_behind() {
        let dir = tempfile::tempdir().unwrap();
        record(dir.path(), &failure());
        record(dir.path(), &failure());

        let drained = take_pending(dir.path());
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].failed, "sync");
        assert!(
            take_pending(dir.path()).is_empty(),
            "a second drain must find nothing"
        );
        assert!(!dir.path().join(FAILURES_FILE).exists());
    }

    #[test]
    fn take_pending_is_empty_when_nothing_failed() {
        let dir = tempfile::tempdir().unwrap();
        assert!(take_pending(dir.path()).is_empty());
    }

    /// A truncated or unrecognized line is dropped; the rest still report.
    #[test]
    fn take_pending_skips_malformed_lines() {
        let dir = tempfile::tempdir().unwrap();
        record(dir.path(), &failure());
        let mut file = OpenOptions::new()
            .append(true)
            .open(dir.path().join(FAILURES_FILE))
            .unwrap();
        file.write_all(b"{\"hook_type\":\"post-merge\"\n").unwrap();
        drop(file);

        let drained = take_pending(dir.path());
        assert_eq!(drained.len(), 1);
    }
}
