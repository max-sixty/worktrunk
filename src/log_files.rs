//! On-disk log file sinks for `-vv` debug output.
//!
//! At `-vv`, two files are written in the repo's `.git/wt/logs/` directory:
//!
//!   - [`TRACE`] → `trace.log`: structured records, `$ cmd [context]`
//!     headers, and bounded subprocess previews. High-signal, bounded size —
//!     safe to embed in `diagnostic.md` bug reports.
//!   - [`SUBPROCESS`] → `subprocess.log`: raw, uncapped subprocess
//!     stdout/stderr bodies captured by `shell_exec::Cmd`. Potentially
//!     multi-MB (full `git log -p` / patch-id output); opt-in for deep
//!     dives.
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
//!   - The `subprocess.log` layer filters to `SUBPROCESS_FULL_TARGET` only,
//!     so raw bodies never reach stderr or `trace.log`.
//!   - The `trace.log` layer accepts every record *except*
//!     `SUBPROCESS_FULL_TARGET` and writes to this file when `-vv` opened it.
//!   - The stderr layer honors `RUST_LOG` plus the flag baseline (`Off` at
//!     no `-v`, `Info` at `-v` *and* `-vv`). Debug records (the noisy
//!     ones) route to the file layers only — `-vv` is a strict superset
//!     of `-v` on stderr.

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

    /// Append a whole formatted event (which may contain internal `\n`s) to
    /// the file under a single lock acquisition. Trailing `\n` from the
    /// fmt layer is stripped — `writeln!` adds its own — so multi-line
    /// events stay grouped together rather than interleaving with another
    /// thread's lines between intermediates. The line should be plain text
    /// (no ANSI codes) for readability in bug reports. Write errors are
    /// swallowed — logging must not break commands.
    pub(crate) fn write_event(&self, text: &str) {
        if let Some(mutex) = self.file.get()
            && let Ok(mut open) = mutex.lock()
        {
            let body = text.trim_end_matches('\n');
            let _ = writeln!(open.file, "{}", body);
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
    /// `MakeWriter`. Buffers one event in memory, then forwards it as a
    /// single locked append to the sink on drop — see [`Self::write_event`]
    /// for why a multi-line event lands together.
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
pub(crate) static SUBPROCESS: LogSink = LogSink {
    file: OnceLock::new(),
    filename: "subprocess.log",
};

/// Initialize both log sinks.
///
/// Called once early in `main` when `-vv` or finer is active. Outside a git
/// repo both sinks stay inactive and all writes become no-ops. Run *before*
/// the tracing subscriber is installed so the `Repository::current()` call
/// here doesn't emit records to a half-built pipeline.
pub(crate) fn init() {
    TRACE.init();
    SUBPROCESS.init();
}

/// Per-event writer: collects formatted bytes, then forwards them to the
/// sink as one locked `write_event` call on drop.
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
        //
        // Single-locked write: a multi-line event (the body has
        // intermediate `\n`s) lands together in the file instead of
        // interleaving with another thread's lines between
        // intermediates. Nothing in this codebase emits multi-line
        // tracing events today, but the contract holds if anything
        // ever does.
        let text = String::from_utf8_lossy(&self.buf);
        self.sink.write_event(&text);
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

/// `MakeWriter` for the subprocess.log layer: always writes to `SUBPROCESS`.
pub(crate) struct SubprocessMakeWriter;

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SubprocessMakeWriter {
    type Writer = SinkWriter;
    fn make_writer(&'a self) -> SinkWriter {
        SUBPROCESS.writer()
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

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::{LogSink, SinkWriter};

    /// `tracing_subscriber::fmt` only ever uses `write` + drop, but
    /// `io::Write` requires us to implement `flush`. The body is a
    /// no-op; this test exists so codecov sees the line covered without
    /// us having to add a `#[cfg(not(coverage))]`-style allow.
    #[test]
    fn sink_writer_flush_is_a_no_op() {
        static SINK: LogSink = LogSink {
            file: std::sync::OnceLock::new(),
            filename: "test-flush.log",
        };
        let mut w = SinkWriter {
            sink: &SINK,
            buf: Vec::new(),
        };
        assert!(w.flush().is_ok());
    }
}
