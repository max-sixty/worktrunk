//! TTY spinner for long-running file-walk operations.
//!
//! Shows a single-line stderr spinner (`⠋ Copying 1,234 files · 312 MiB`,
//! `⠋ Removing 7,272 files · 64.5 MiB`) that updates in place while the work
//! runs. Workers bump atomic counters via [`Progress::record`]; a background
//! thread renders at ~10Hz using crossterm cursor control.
//!
//! `start` is named deliberately (not `new`) because it spawns a ticker thread
//! as a side effect — `Default`-style semantics would be misleading. The verb
//! (`"Copying"`, `"Removing"`) is fixed for the lifetime of the spinner.
//!
//! The progress line is cleared on [`Progress::finish`] or on drop, so the
//! caller can print a summary message immediately afterward without overlap.
//!
//! The full spinner machinery (crossterm, the ticker thread, the render loop)
//! is gated on the `cli` feature. Without `cli`, [`Progress`] is a zero-cost
//! stub: `start` always returns a no-op reporter and `record` does nothing.
//! Pure formatting helpers ([`format_bytes`], [`format_stats_paren`]) are
//! always available since callers in both modes want them.

use color_print::cformat;

pub use imp::Progress;

#[cfg(feature = "cli")]
mod imp {
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

    use super::{format_bytes, format_count};

    const SPINNER_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    const TICK_INTERVAL: Duration = Duration::from_millis(100);
    /// Delay before the first frame renders, so sub-second operations stay silent.
    const STARTUP_DELAY: Duration = Duration::from_millis(300);

    struct Shared {
        files: AtomicUsize,
        bytes: AtomicU64,
        done: AtomicBool,
        verb: &'static str,
    }

    struct Inner {
        shared: Arc<Shared>,
        ticker: JoinHandle<()>,
    }

    /// Live spinner displaying file and byte counters for a single operation.
    ///
    /// See [module docs](super) for the output format and lifecycle.
    pub struct Progress(Option<Inner>);

    impl Progress {
        /// Start a progress reporter, enabling the spinner iff stderr is a TTY.
        ///
        /// `verb` is the present-participle label shown to the user (e.g.
        /// `"Copying"`, `"Removing"`). Spawns a background ticker thread when a
        /// TTY is detected. When stderr is not a TTY, returns a disabled reporter
        /// and does no work.
        pub fn start(verb: &'static str) -> Self {
            Self::start_with(verb, std::io::stderr().is_terminal())
        }

        /// Dispatch helper that picks the enabled or disabled branch from an
        /// explicit `is_tty` flag. Extracted so tests can exercise both branches
        /// without depending on the ambient stderr fd — sandboxes (Nix builds,
        /// some CI runners) hand the test process a PTY-backed stderr, which
        /// would flip the gate in `start` and break a test that hard-coded the
        /// disabled outcome. See #2615.
        fn start_with(verb: &'static str, is_tty: bool) -> Self {
            if is_tty {
                Self::enabled(verb)
            } else {
                Self::disabled()
            }
        }

        /// A reporter that does nothing — for benchmarks, tests, and internal moves.
        pub fn disabled() -> Self {
            Self(None)
        }

        /// Constructor for the enabled state, separated so the TTY-gated branch in
        /// [`Self::start`] and the test-only "force enabled" path share one
        /// implementation. Spawns the ticker thread; safe to call from any
        /// context that genuinely wants live output.
        fn enabled(verb: &'static str) -> Self {
            let shared = Arc::new(Shared {
                files: AtomicUsize::new(0),
                bytes: AtomicU64::new(0),
                done: AtomicBool::new(false),
                verb,
            });
            let ticker = {
                let shared = Arc::clone(&shared);
                thread::spawn(move || ticker_loop(&shared))
            };
            Self(Some(Inner { shared, ticker }))
        }

        /// Record that a file (or symlink) was processed. Safe to call from any thread.
        pub fn record(&self, bytes: u64) {
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

    impl Drop for Progress {
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
        // Sub-300ms operations render nothing — the line never gets drawn.
        // park_timeout returns immediately on `unpark` from drop, so short
        // operations don't block shutdown either.
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
            let line = format_line(shared.verb, files, bytes, SPINNER_FRAMES[frame_idx]);
            let _ = render_line(&mut std::io::stderr().lock(), &line);
            thread::park_timeout(TICK_INTERVAL);
        }
    }

    fn format_line(verb: &str, files: usize, bytes: u64, spinner: char) -> String {
        if files == 0 {
            cformat!("<cyan>{spinner}</> {verb}...")
        } else {
            let word = if files == 1 { "file" } else { "files" };
            cformat!(
                "<cyan>{spinner}</> {verb} {} {} · {}",
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

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_format_line_empty() {
            let line = format_line("Copying", 0, 0, '⠋');
            assert!(line.contains("Copying..."));
            assert!(line.contains('⠋'));
        }

        #[test]
        fn test_format_line_singular() {
            let line = format_line("Copying", 1, 42, '⠙');
            assert!(line.contains("1 file "));
            assert!(line.contains("42 B"));
        }

        #[test]
        fn test_format_line_plural() {
            let line = format_line("Removing", 2_500, 5 * 1024 * 1024, '⠹');
            assert!(line.contains("Removing"));
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
        fn test_start_with_non_tty_is_disabled() {
            assert!(Progress::start_with("Copying", false).0.is_none());
        }

        #[test]
        fn test_start_with_tty_is_enabled() {
            let p = Progress::start_with("Copying", true);
            assert!(p.0.is_some());
            p.finish();
        }

        #[test]
        fn test_enabled_lifecycle_counters_propagate() {
            let p = Progress::enabled("Copying");
            p.record(1024);
            p.record(2048);
            let inner = p.0.as_ref().expect("expected enabled");
            assert_eq!(inner.shared.files.load(Ordering::Relaxed), 2);
            assert_eq!(inner.shared.bytes.load(Ordering::Relaxed), 3072);
            p.finish();
        }

        #[test]
        fn test_enabled_renders_after_startup_delay() {
            let p = Progress::enabled("Removing");
            p.record(100);
            // Wait past the startup delay + one tick so ticker_loop reaches the
            // render branch — the part that's hardest to cover otherwise.
            std::thread::sleep(STARTUP_DELAY + TICK_INTERVAL + Duration::from_millis(50));
            p.finish();
        }
    }
}

#[cfg(not(feature = "cli"))]
mod imp {
    /// Zero-cost stub when the `cli` feature is off. The spinner depends on
    /// `crossterm`, which is only pulled in by `cli`; library consumers that
    /// disable default features get this no-op type instead. Counters are
    /// dropped on the floor — no allocation, no thread, no rendering.
    pub struct Progress;

    impl Progress {
        pub fn start(_verb: &'static str) -> Self {
            Self
        }

        pub fn disabled() -> Self {
            Self
        }

        pub fn record(&self, _bytes: u64) {}

        pub fn finish(self) {}
    }
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
/// we compute. Used by both the spinner line and the post-operation summary.
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

/// Format `(N files · X MiB)` as a gray stats parenthetical, matching the
/// spinner's units.
///
/// Returns an empty string when `files == 0` so callers can unconditionally
/// concatenate it to a success message without producing `(0 files · 0 B)`
/// when nothing was processed.
pub fn format_stats_paren(files: usize, bytes: u64) -> String {
    if files == 0 {
        return String::new();
    }
    let word = if files == 1 { "file" } else { "files" };
    // Split the closing paren into a separate cformat so the optimizer doesn't
    // collapse the two color-print spans (matches the squash-progress pattern
    // in commands/step/squash.rs).
    let close = cformat!("<bright-black>)</>");
    cformat!(
        " <bright-black>({} {word} · {}</>{close}",
        format_count(files),
        format_bytes(bytes),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_format_stats_paren_empty_is_blank() {
        assert_eq!(format_stats_paren(0, 0), "");
    }

    #[test]
    fn test_format_stats_paren_singular() {
        let s = format_stats_paren(1, 42);
        assert!(s.contains("1 file"));
        assert!(s.contains("42 B"));
    }

    #[test]
    fn test_format_stats_paren_plural() {
        let s = format_stats_paren(2_500, 5 * 1024 * 1024);
        assert!(s.contains("2,500 files"));
        assert!(s.contains("5.0 MiB"));
    }

    #[test]
    fn test_disabled_record_is_noop() {
        let p = Progress::disabled();
        p.record(1_000_000);
        p.record(2_000_000);
        // No counters to inspect — Disabled has no fields. The assertion is
        // simply that the call doesn't panic and finish() returns cleanly.
        p.finish();
    }
}
