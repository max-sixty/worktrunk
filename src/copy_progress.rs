//! TTY spinner for long-running copy operations.
//!
//! Shows a single-line stderr spinner (`⠋ Copying 1,234 files · 312 MiB`) that
//! updates in place while files are copied. Copy workers bump atomic counters
//! via [`CopyProgress::file_copied`]; a background thread renders at ~10Hz
//! using crossterm cursor control.
//!
//! `start` is named deliberately (not `new`) because it spawns a ticker thread
//! as a side effect — `Default`-style semantics would be misleading.
//!
//! The progress line is cleared on [`CopyProgress::finish`] or on drop, so the
//! caller can print a summary message immediately afterward without overlap.

use std::io::{IsTerminal, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use color_print::cformat;
use crossterm::{
    QueueableCommand,
    cursor::MoveToColumn,
    terminal::{Clear, ClearType},
};

const SPINNER_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const TICK_INTERVAL: Duration = Duration::from_millis(100);
/// Delay before the first frame renders, so sub-second copies stay silent.
const STARTUP_DELAY: Duration = Duration::from_millis(300);

struct Shared {
    files: AtomicUsize,
    bytes: AtomicU64,
    done: AtomicBool,
}

struct Inner {
    shared: Arc<Shared>,
    ticker: JoinHandle<()>,
}

/// Live spinner displaying files-copied and bytes-copied counters.
///
/// See [module docs](crate::copy_progress) for the output format and lifecycle.
pub struct CopyProgress(Option<Inner>);

impl CopyProgress {
    /// Start a progress reporter, enabling the spinner iff stderr is a TTY.
    ///
    /// Spawns a background ticker thread when a TTY is detected. When stderr
    /// is not a TTY, returns a disabled reporter and does no work.
    pub fn start() -> Self {
        if !std::io::stderr().is_terminal() {
            return Self::disabled();
        }
        let shared = Arc::new(Shared {
            files: AtomicUsize::new(0),
            bytes: AtomicU64::new(0),
            done: AtomicBool::new(false),
        });
        let ticker = {
            let shared = Arc::clone(&shared);
            thread::spawn(move || ticker_loop(&shared))
        };
        Self(Some(Inner { shared, ticker }))
    }

    /// A reporter that does nothing — for benchmarks, tests, and internal moves.
    pub fn disabled() -> Self {
        Self(None)
    }

    /// Record that a file (or symlink) was copied. Safe to call from any thread.
    pub fn file_copied(&self, bytes: u64) {
        if let Some(inner) = &self.0 {
            inner.shared.files.fetch_add(1, Ordering::Relaxed);
            inner.shared.bytes.fetch_add(bytes, Ordering::Relaxed);
        }
    }

    /// Stop the spinner and clear the progress line.
    pub fn finish(self) {
        // Drop runs the same shutdown logic — no need to duplicate it here.
        drop(self);
    }
}

impl Drop for CopyProgress {
    fn drop(&mut self) {
        // `Inner` is Drop-free, so we can take ownership of its fields and
        // run shutdown without partial-move conflicts.
        if let Some(inner) = self.0.take() {
            inner.shared.done.store(true, Ordering::Relaxed);
            inner.ticker.thread().unpark();
            let _ = inner.ticker.join();
            let _ = clear_line(&mut std::io::stderr().lock());
        }
    }
}

fn ticker_loop(shared: &Shared) {
    let start = Instant::now();
    // Sub-300ms copies render nothing — the line never gets drawn. park_timeout
    // returns immediately on `unpark` from drop, so short copies don't block
    // shutdown either.
    while start.elapsed() < STARTUP_DELAY {
        if shared.done.load(Ordering::Relaxed) {
            return;
        }
        thread::park_timeout(STARTUP_DELAY - start.elapsed());
    }
    while !shared.done.load(Ordering::Relaxed) {
        let frame_idx = (start.elapsed().as_millis() / TICK_INTERVAL.as_millis()) as usize
            % SPINNER_FRAMES.len();
        let files = shared.files.load(Ordering::Relaxed);
        let bytes = shared.bytes.load(Ordering::Relaxed);
        let line = format_line(files, bytes, SPINNER_FRAMES[frame_idx]);
        let _ = render_line(&mut std::io::stderr().lock(), &line);
        thread::park_timeout(TICK_INTERVAL);
    }
}

fn format_line(files: usize, bytes: u64, spinner: char) -> String {
    if files == 0 {
        cformat!("<cyan>{spinner}</> Copying...")
    } else {
        let word = if files == 1 { "file" } else { "files" };
        cformat!(
            "<cyan>{spinner}</> Copying {} {} · {}",
            format_count(files),
            word,
            format_bytes(bytes),
        )
    }
}

fn render_line<W: Write>(w: &mut W, line: &str) -> std::io::Result<()> {
    w.queue(MoveToColumn(0))?;
    w.queue(Clear(ClearType::CurrentLine))?;
    write!(w, "{line}")?;
    w.flush()
}

fn clear_line<W: Write>(w: &mut W) -> std::io::Result<()> {
    w.queue(MoveToColumn(0))?;
    w.queue(Clear(ClearType::CurrentLine))?;
    w.flush()
}

fn format_count(n: usize) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

/// Format a byte count using IEC binary prefixes (KiB, MiB, GiB, TiB).
///
/// The divisor is 1024; SI-prefix "MB" would imply 10^6 and doesn't match what
/// we compute. Used by both the spinner line and the post-copy summary.
pub fn format_bytes(n: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut size = n as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} {}", UNITS[unit])
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Construct an enabled instance directly, bypassing the TTY check that
    /// would otherwise force a disabled reporter under cargo test. Lives in
    /// the test module per the CLAUDE.md rule against test-only library APIs.
    fn enabled_for_test() -> CopyProgress {
        let shared = Arc::new(Shared {
            files: AtomicUsize::new(0),
            bytes: AtomicU64::new(0),
            done: AtomicBool::new(false),
        });
        let ticker = {
            let shared = Arc::clone(&shared);
            thread::spawn(move || ticker_loop(&shared))
        };
        CopyProgress(Some(Inner { shared, ticker }))
    }

    #[test]
    fn test_format_count() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(42), "42");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1_000), "1,000");
        assert_eq!(format_count(12_345), "12,345");
        assert_eq!(format_count(1_234_567), "1,234,567");
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(1_536), "1.5 KiB");
        assert_eq!(format_bytes(1_048_576), "1.0 MiB");
        assert_eq!(format_bytes(1_610_612_736), "1.5 GiB");
    }

    #[test]
    fn test_format_line_empty() {
        let line = format_line(0, 0, '⠋');
        assert!(line.contains("Copying..."));
        assert!(line.contains('⠋'));
    }

    #[test]
    fn test_format_line_singular() {
        let line = format_line(1, 42, '⠙');
        assert!(line.contains("1 file "));
        assert!(line.contains("42 B"));
    }

    #[test]
    fn test_format_line_plural() {
        let line = format_line(2_500, 5 * 1024 * 1024, '⠹');
        assert!(line.contains("2,500 files"));
        assert!(line.contains("5.0 MiB"));
    }

    #[test]
    fn test_render_line_writes_text_with_prefix_control_bytes() {
        let mut buf = Vec::new();
        render_line(&mut buf, "hello").unwrap();
        assert!(buf.ends_with(b"hello"));
        assert!(buf.len() > b"hello".len());
    }

    #[test]
    fn test_clear_line_writes_control_bytes() {
        let mut buf = Vec::new();
        clear_line(&mut buf).unwrap();
        assert!(!buf.is_empty());
    }

    #[test]
    fn test_disabled_file_copied_is_noop() {
        let p = CopyProgress::disabled();
        p.file_copied(1_000_000);
        p.file_copied(2_000_000);
        // No counters to inspect — Disabled has no fields. The assertion is
        // simply that the call doesn't panic and finish() returns cleanly.
        p.finish();
    }

    #[test]
    fn test_start_in_non_tty_is_disabled() {
        assert!(CopyProgress::start().0.is_none());
    }

    #[test]
    fn test_enabled_lifecycle_counters_propagate() {
        let p = enabled_for_test();
        p.file_copied(1024);
        p.file_copied(2048);
        let inner = p.0.as_ref().expect("expected enabled");
        assert_eq!(inner.shared.files.load(Ordering::Relaxed), 2);
        assert_eq!(inner.shared.bytes.load(Ordering::Relaxed), 3072);
        p.finish();
    }

    #[test]
    fn test_enabled_renders_after_startup_delay() {
        let p = enabled_for_test();
        p.file_copied(100);
        // Wait past the startup delay + one tick so ticker_loop reaches the
        // render branch — the part that's hardest to cover otherwise.
        std::thread::sleep(STARTUP_DELAY + TICK_INTERVAL + Duration::from_millis(50));
        p.finish();
    }
}
