//! TTY spinner for long-running copy operations.
//!
//! Shows a single-line stderr spinner (`⠋ Copying 1,234 files · 312 MB`) that
//! updates in place while files are copied. Copy workers bump atomic counters
//! via [`CopyProgress::file_copied`]; a background thread renders at ~10Hz
//! using crossterm cursor control.
//!
//! Constructed via [`CopyProgress::start`], which auto-detects stderr TTY and
//! returns a disabled (no-op) instance when not attached to a terminal. Callers
//! that want to opt out explicitly — benchmarks, tests, and non-interactive
//! internal moves — should pass [`CopyProgress::disabled`]. `start` is named
//! deliberately (not `new`) because it spawns a ticker thread as a side effect
//! — `Default`-style "make me a fresh instance" semantics would be misleading.
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

/// Live spinner displaying files-copied and bytes-copied counters.
///
/// See [module docs](crate::copy_progress) for the output format and lifecycle.
pub struct CopyProgress {
    shared: Arc<Shared>,
    ticker: Option<JoinHandle<()>>,
}

impl CopyProgress {
    /// Start a progress reporter, enabling the spinner iff stderr is a TTY.
    ///
    /// Spawns a background ticker thread when a TTY is detected. When stderr
    /// is not a TTY, returns the same shape as [`CopyProgress::disabled`] and
    /// does no work.
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
        Self {
            shared,
            ticker: Some(ticker),
        }
    }

    /// Create a disabled reporter. All methods are no-ops; no thread is spawned.
    pub fn disabled() -> Self {
        Self {
            shared: Arc::new(Shared {
                files: AtomicUsize::new(0),
                bytes: AtomicU64::new(0),
                done: AtomicBool::new(true),
            }),
            ticker: None,
        }
    }

    /// Record that a file (or symlink) was copied. Safe to call from any thread.
    pub fn file_copied(&self, bytes: u64) {
        if self.ticker.is_none() {
            return;
        }
        self.shared.files.fetch_add(1, Ordering::Relaxed);
        self.shared.bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Stop the spinner and clear the progress line.
    pub fn finish(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        if let Some(ticker) = self.ticker.take() {
            self.shared.done.store(true, Ordering::Relaxed);
            // Wake the ticker if it's mid-sleep so `join` returns promptly.
            ticker.thread().unpark();
            let _ = ticker.join();
            clear_line();
        }
    }
}

impl Drop for CopyProgress {
    fn drop(&mut self) {
        self.stop();
    }
}

fn ticker_loop(shared: &Shared) {
    let start = Instant::now();
    // Startup delay: wait up to STARTUP_DELAY before the first render. If the
    // copy finishes in under that window there's no flicker — the line never
    // gets drawn. park_timeout returns immediately on `unpark` from `stop()`,
    // so short copies also don't block shutdown.
    while start.elapsed() < STARTUP_DELAY {
        if shared.done.load(Ordering::Relaxed) {
            return;
        }
        thread::park_timeout(STARTUP_DELAY - start.elapsed());
    }
    while !shared.done.load(Ordering::Relaxed) {
        let frame_idx = (start.elapsed().as_millis() / TICK_INTERVAL.as_millis()) as usize
            % SPINNER_FRAMES.len();
        render(shared, SPINNER_FRAMES[frame_idx]);
        thread::park_timeout(TICK_INTERVAL);
    }
}

fn render(shared: &Shared, spinner: char) {
    let files = shared.files.load(Ordering::Relaxed);
    let bytes = shared.bytes.load(Ordering::Relaxed);
    let line = if files == 0 {
        cformat!("<cyan>{spinner}</> Copying...")
    } else {
        let word = if files == 1 { "file" } else { "files" };
        cformat!(
            "<cyan>{spinner}</> Copying {} {} · {}",
            format_count(files),
            word,
            format_bytes(bytes),
        )
    };

    let mut err = std::io::stderr().lock();
    let _ = err.queue(MoveToColumn(0));
    let _ = err.queue(Clear(ClearType::CurrentLine));
    let _ = write!(err, "{line}");
    let _ = err.flush();
}

fn clear_line() {
    let mut err = std::io::stderr().lock();
    let _ = err.queue(MoveToColumn(0));
    let _ = err.queue(Clear(ClearType::CurrentLine));
    let _ = err.flush();
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

fn format_bytes(n: u64) -> String {
    // IEC binary prefixes — these match the 1024 divisor. SI-prefix "MB" would
    // imply 10^6, which doesn't match what we compute.
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
    fn test_disabled_is_noop() {
        let p = CopyProgress::disabled();
        p.file_copied(1_000_000);
        p.file_copied(2_000_000);
        // No thread spawned; counters stay at 0 for disabled instances.
        assert_eq!(p.shared.files.load(Ordering::Relaxed), 0);
        assert_eq!(p.shared.bytes.load(Ordering::Relaxed), 0);
        p.finish();
    }

    #[test]
    fn test_start_in_non_tty_is_disabled() {
        // cargo test runs without a stderr TTY, so `start()` should fall back
        // to the disabled path and not spawn a ticker thread.
        let p = CopyProgress::start();
        assert!(p.ticker.is_none());
    }
}
