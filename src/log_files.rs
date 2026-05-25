//! On-disk log file sinks for `-vv` debug output.
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
//! module governs only the `log::*` / `tracing::*` macro pipeline.
//!
//! # Routing
//!
//! Routing is performed structurally by the `tracing-subscriber` layers
//! registered in `init_logging`:
//!
//!   - The `output.log` layer filters to `SUBPROCESS_FULL_TARGET` only,
//!     so raw bodies never reach stderr or `trace.log`.
//!   - The `trace.log` layer accepts every record *except*
//!     `SUBPROCESS_FULL_TARGET` and writes to this file when `-vv` opened it.
//!   - The stderr layer is disabled at `-vv` (the file layers replace it)
//!     and otherwise honors `RUST_LOG` plus the `-v` baseline.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

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

    /// Per-event `io::Write` adapter for use as a `tracing_subscriber`
    /// `MakeWriter`. Buffers one event in memory, then forwards a single
    /// line per intermediate `\n` to the sink on drop.
    fn writer(&'static self) -> SinkWriter {
        SinkWriter {
            sink: self,
            buf: Vec::new(),
        }
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
/// repo both sinks stay inactive and all writes become no-ops. Run *before*
/// the tracing subscriber is installed so the `Repository::current()` call
/// here doesn't emit records to a half-built pipeline.
pub(crate) fn init() {
    TRACE.init();
    OUTPUT.init();
    // Let shell_exec phrase the elision marker to match reality — points at
    // output.log when it exists, else suggests rerunning with -vv.
    worktrunk::shell_exec::set_output_log_active(OUTPUT.is_active());
}

/// Per-event writer: collects formatted bytes, then forwards a single line
/// to the sink on drop. Splits internal `\n` so multi-line events still
/// produce one `write_line` per visual line.
pub(crate) struct SinkWriter {
    sink: &'static LogSink,
    buf: Vec<u8>,
}

impl io::Write for SinkWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for SinkWriter {
    fn drop(&mut self) {
        // `tracing_subscriber::fmt` always invokes the writer for an
        // event (the formatter writes at least the trailing `\n`), so
        // we don't need an empty-buffer guard here.
        let text = String::from_utf8_lossy(&self.buf);
        // The fmt layer emits a trailing `\n` per event; strip it so
        // `write_line` doesn't double it. Intermediate newlines (from
        // multi-line messages) become separate lines, each prefixed by
        // whatever the format wrote.
        for line in text.trim_end_matches('\n').split('\n') {
            self.sink.write_line(line);
        }
    }
}

/// `MakeWriter` for the trace.log layer: always writes to `TRACE`.
pub(crate) struct TraceMakeWriter;

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for TraceMakeWriter {
    type Writer = SinkWriter;
    fn make_writer(&'a self) -> SinkWriter {
        TRACE.writer()
    }
}

/// `MakeWriter` for the output.log layer: always writes to `OUTPUT`.
pub(crate) struct OutputMakeWriter;

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for OutputMakeWriter {
    type Writer = SinkWriter;
    fn make_writer(&'a self) -> SinkWriter {
        OUTPUT.writer()
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
