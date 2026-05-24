//! On-disk log file sinks and routing for `-vv` debug output.
//!
//! At `-vv`, two files are written in the repo's `.git/wt/logs/` directory:
//!
//!   - [`TRACE`] → `trace.log`: structured records, `$ cmd [context]`
//!     headers, and bounded subprocess previews. High-signal, bounded size —
//!     safe to embed in `diagnostic.md` bug reports.
//!   - [`OUTPUT`] → `output.log`: raw, uncapped subprocess stdout/stderr
//!     bodies captured by `shell_exec::Cmd`. Potentially multi-MB (full
//!     `git log -p` / patch-id output); opt-in for deep dives.
//!
//! Direct user-facing output (`info_message` / `eprintln!` from command
//! code) is unaffected — it goes to stderr at every verbosity level. This
//! module governs only the `log::*` macro pipeline.
//!
//! # Routing
//!
//! [`route`] is the single source of truth for which sink a log record
//! reaches. Invariants:
//!
//!   - `SUBPROCESS_FULL_TARGET` records never reach stderr — raw bodies
//!     don't flood terminals. They go to `output.log` if active, else
//!     drop.
//!   - All other log records go to `trace.log` when it's active (`-vv`),
//!     and to stderr otherwise (e.g. `-v`, or `RUST_LOG=debug` without
//!     `-vv`). At `-vv` the `log::*` pipeline is silent on stderr — the
//!     user-facing `eprintln!` output stays, but the noisy `$ cmd`
//!     headers and subprocess previews land in the file.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use worktrunk::shell_exec::SUBPROCESS_FULL_TARGET;

pub(crate) struct LogSink {
    file: OnceLock<Mutex<OpenFile>>,
    filename: &'static str,
}

struct OpenFile {
    path: PathBuf,
    file: File,
}

impl LogSink {
    fn init(&self) {
        if let Some((path, file)) = try_create(self.filename) {
            let _ = self.file.set(Mutex::new(OpenFile { path, file }));
        }
    }

    /// Whether the file has been successfully created.
    ///
    /// Lock-free (`OnceLock::get`); safe for per-record hot-path checks.
    pub(crate) fn is_active(&self) -> bool {
        self.file.get().is_some()
    }

    /// Append a line to the file (no-op if not initialized).
    ///
    /// The line should be plain text (no ANSI codes) for readability in bug
    /// reports. Write errors are swallowed — logging must not break commands.
    pub(crate) fn write_line(&self, line: &str) {
        if let Some(mutex) = self.file.get()
            && let Ok(mut open) = mutex.lock()
        {
            let _ = writeln!(open.file, "{}", line);
            let _ = open.file.flush();
        }
    }

    /// Path to the file, if it was created.
    pub(crate) fn path(&self) -> Option<PathBuf> {
        self.file
            .get()
            .and_then(|mutex| mutex.lock().ok().map(|open| open.path.clone()))
    }
}

pub(crate) static TRACE: LogSink = LogSink {
    file: OnceLock::new(),
    filename: "trace.log",
};
pub(crate) static OUTPUT: LogSink = LogSink {
    file: OnceLock::new(),
    filename: "output.log",
};

/// Initialize both log sinks.
///
/// Called once early in `main` when `-vv` or finer is active. Outside a git
/// repo both sinks stay inactive and all writes become no-ops.
pub(crate) fn init() {
    TRACE.init();
    OUTPUT.init();
    // Let shell_exec phrase the elision marker to match reality — points at
    // output.log when it exists, else suggests rerunning with -vv. The
    // false case at -vv (open failed asymmetrically) is benign here: the
    // user already got a "(output.log unavailable)" warning from the
    // startup pointer, so the marker's "rerun with -vv" text is just
    // redundant rather than misleading.
    worktrunk::shell_exec::set_output_log_available(OUTPUT.is_active());
}

/// Sink routing decision for one log record.
pub(crate) enum Route {
    /// Append to this sink; skip stderr.
    File(&'static LogSink),
    /// Emit to stderr with normal formatting. Chosen when TRACE is
    /// inactive — the `-vv` path takes `File(&TRACE)` instead.
    Stderr,
    /// Drop the record entirely.
    Drop,
}

/// Decide where a log record goes based on its target.
///
/// See module docs for the invariants each variant upholds.
pub(crate) fn route(target: &str) -> Route {
    if target == SUBPROCESS_FULL_TARGET {
        if OUTPUT.is_active() {
            Route::File(&OUTPUT)
        } else {
            Route::Drop
        }
    } else if TRACE.is_active() {
        // -vv: send the noisy log pipeline to trace.log instead of stderr
        // so the user's terminal stays readable.
        Route::File(&TRACE)
    } else {
        // -v, or RUST_LOG=debug without -vv. `SUBPROCESS_BOUNDED_TARGET`
        // and all other targets share this path.
        Route::Stderr
    }
}

fn try_create(filename: &str) -> Option<(PathBuf, File)> {
    let repo = worktrunk::git::Repository::current().ok()?;
    let log_dir = repo.wt_logs_dir();
    std::fs::create_dir_all(&log_dir).ok()?;
    let path = log_dir.join(filename);
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .ok()?;
    Some((path, file))
}
