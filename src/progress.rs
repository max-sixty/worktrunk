//! TTY spinners for long-running work.
//!
//! Two spinner types share one set of render primitives (spinner frames, cursor
//! control, TTY gating, clear-on-drop): [`Progress`] shows file/byte counters
//! for file-walk operations; [`Watchdog`] shows an elapsed-time "still waiting"
//! line for a slow blocking subprocess (e.g. the `commit.generation` LLM) so a
//! long or hung command isn't silent.
//!
//! ## `Progress`
//!
//! Shows a single-line stderr spinner (`⠋ Copying 1,234 files · 312 MiB`,
//! `⠋ Removing 7,272 files · 64.5 MiB`) that updates in place while the work
//! runs. Workers bump atomic counters via [`Progress::record`]; a background
//! thread renders at ~10Hz using crossterm cursor control.
//!
//! `Progress` is the single owner of operation counts: every state (spinner
//! enabled, disabled, and the no-`cli` stub) accumulates files/bytes, and
//! [`Progress::totals`] reads the running totals from `&self` mid-operation.
//! Callers report from `totals()` rather than keeping their own counters.
//! Counts accumulate for the lifetime of the reporter, so each counted
//! operation (or batch reported as one) gets its own `Progress`.
//!
//! `start` is named deliberately (not `new`) because it spawns a ticker thread
//! as a side effect — `Default`-style semantics would be misleading. The verb
//! (`"Copying"`, `"Removing"`) is fixed for the lifetime of the spinner.
//!
//! The progress line is cleared on [`Progress::finish`] or on drop, so the
//! caller can print a summary message immediately afterward without overlap.
//!
//! The spinner machinery (crossterm, the ticker thread, the render loop) is
//! gated on the `cli` feature. Without `cli`, [`Progress`] keeps the counters
//! but never renders. Pure formatting helpers ([`format_bytes`],
//! [`format_stats_paren`]) are always available since callers in both modes
//! want them.

use color_print::cformat;

pub use imp::Progress;
pub use imp::Watchdog;

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
        cursor::{MoveToColumn, MoveUp},
        terminal::{Clear, ClearType},
    };

    use super::{format_bytes, format_count};

    const SPINNER_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    const TICK_INTERVAL: Duration = Duration::from_millis(100);
    /// Delay before the first frame renders, so sub-second operations stay silent.
    const STARTUP_DELAY: Duration = Duration::from_millis(300);
    /// `Watchdog` waits longer before nagging — a configured LLM routinely takes
    /// a couple of seconds, and the caller has already printed a "Generating…"
    /// line, so the watchdog only surfaces once a command is genuinely slow.
    const WATCHDOG_STARTUP_DELAY: Duration = Duration::from_secs(2);
    /// Second tier: once a command has run this long, the bare status line
    /// isn't enough to debug it, so the exact invocation is revealed in a gutter
    /// beneath the status — cleared with the rest of the block when the command
    /// finishes.
    const WATCHDOG_ESCALATE_DELAY: Duration = Duration::from_secs(10);

    struct Shared {
        files: AtomicUsize,
        bytes: AtomicU64,
        done: AtomicBool,
    }

    impl Shared {
        fn new() -> Self {
            Self {
                files: AtomicUsize::new(0),
                bytes: AtomicU64::new(0),
                done: AtomicBool::new(false),
            }
        }
    }

    /// Live spinner displaying file and byte counters for a single operation.
    ///
    /// Counters accumulate in every state; only the rendering is conditional.
    /// See [module docs](super) for the output format and lifecycle.
    pub struct Progress {
        shared: Arc<Shared>,
        /// Render thread; present only when the spinner is enabled (stderr TTY).
        ticker: Option<JoinHandle<()>>,
    }

    impl Progress {
        /// Start a progress reporter, enabling the spinner iff stderr is a TTY.
        ///
        /// `verb` is the present-participle label shown to the user (e.g.
        /// `"Copying"`, `"Removing"`). Spawns a background ticker thread when a
        /// TTY is detected. When stderr is not a TTY, returns a disabled
        /// reporter that still counts but renders nothing.
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

        /// A reporter that renders nothing but still counts — for non-TTY
        /// contexts, benchmarks, tests, and internal moves.
        pub fn disabled() -> Self {
            Self {
                shared: Arc::new(Shared::new()),
                ticker: None,
            }
        }

        /// Constructor for the enabled state, separated so the TTY-gated branch in
        /// [`Self::start`] and the test-only "force enabled" path share one
        /// implementation. Spawns the ticker thread; safe to call from any
        /// context that genuinely wants live output.
        fn enabled(verb: &'static str) -> Self {
            let shared = Arc::new(Shared::new());
            let ticker = {
                let shared = Arc::clone(&shared);
                thread::spawn(move || ticker_loop(&shared, verb))
            };
            Self {
                shared,
                ticker: Some(ticker),
            }
        }

        /// Record that a file (or symlink) was processed. Safe to call from any thread.
        pub fn record(&self, bytes: u64) {
            self.shared.files.fetch_add(1, Ordering::Relaxed);
            self.shared.bytes.fetch_add(bytes, Ordering::Relaxed);
        }

        /// Running `(files, bytes)` totals recorded so far.
        ///
        /// Relaxed loads — exact only once the recording threads have finished
        /// (e.g. after the rayon pool call returns).
        pub fn totals(&self) -> (usize, u64) {
            (
                self.shared.files.load(Ordering::Relaxed),
                self.shared.bytes.load(Ordering::Relaxed),
            )
        }

        /// Stop the spinner and clear the progress line.
        pub fn finish(self) {
            // Drop runs the same shutdown logic — no need to duplicate it here.
            drop(self);
        }
    }

    impl Drop for Progress {
        fn drop(&mut self) {
            if let Some(ticker) = self.ticker.take() {
                self.shared.done.store(true, Ordering::Relaxed);
                ticker.thread().unpark();
                let _ = ticker.join();
                let _ = clear_line(&mut std::io::stderr().lock());
            }
        }
    }

    fn ticker_loop(shared: &Shared, verb: &str) {
        let start = Instant::now();
        // Sub-300ms operations render nothing — the line never gets drawn.
        // park_timeout returns immediately on `unpark` from drop, so short
        // operations don't block shutdown either.
        while start.elapsed() < STARTUP_DELAY {
            if shared.done.load(Ordering::Relaxed) {
                return;
            }
            thread::park_timeout(STARTUP_DELAY.saturating_sub(start.elapsed()));
        }
        while !shared.done.load(Ordering::Relaxed) {
            let frame_idx = (start.elapsed().as_millis() / TICK_INTERVAL.as_millis()) as usize
                % SPINNER_FRAMES.len();
            let files = shared.files.load(Ordering::Relaxed);
            let bytes = shared.bytes.load(Ordering::Relaxed);
            let line = format_line(verb, files, bytes, SPINNER_FRAMES[frame_idx]);
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

    /// Shared state between the watchdog and its ticker thread. `rendered_rows`
    /// is the terminal-row count of the block currently on screen — 0 means
    /// nothing was drawn (a fast command that finished inside
    /// [`WATCHDOG_STARTUP_DELAY`], the common case), so `Drop` leaves the
    /// terminal untouched; >0 tells `Drop` how many rows to clear. `escalated`
    /// records whether the command gutter has been revealed, for tests.
    struct WatchdogShared {
        done: AtomicBool,
        rendered_rows: AtomicUsize,
        escalated: AtomicBool,
    }

    impl WatchdogShared {
        fn new() -> Self {
            Self {
                done: AtomicBool::new(false),
                rendered_rows: AtomicUsize::new(0),
                escalated: AtomicBool::new(false),
            }
        }
    }

    /// Live "still waiting" status for a single slow blocking subprocess.
    ///
    /// Unlike [`Progress`] (which counts work units), `Watchdog` just tracks
    /// elapsed time. After a startup delay it renders a dim one-line status —
    /// `○ Waiting for the commit message (4s)` — redrawn in place each second;
    /// the ticking counter is the "still alive" signal. After a longer delay it
    /// escalates, revealing the exact command in a gutter beneath the status:
    ///
    /// ```text
    /// ○ Waiting for the commit message (12s)
    ///    sh -c 'claude -p --model=haiku'
    /// ```
    ///
    /// The whole block is cleared on drop — nothing persists — so the caller can
    /// print its result immediately afterward. The dim, in-place look mirrors
    /// `wt list`'s stall footer.
    ///
    /// It only reads a clock and writes to stderr; it never touches the child's
    /// stdin/stdout or its `wait`, so it's safe to wrap a capture-mode
    /// [`Cmd::run`](crate::shell_exec::Cmd) whose output is captured rather than
    /// streamed (e.g. the LLM commit command). Don't pair it with a streaming
    /// command — the child's own output would interleave with the status block.
    pub struct Watchdog {
        shared: Arc<WatchdogShared>,
        /// Render thread; present only when enabled (stderr TTY, non-verbose).
        ticker: Option<JoinHandle<()>>,
    }

    impl Watchdog {
        /// Start a watchdog for a slow subprocess.
        ///
        /// `waiting_for` names what is being awaited (e.g. `"the commit
        /// message"`). `command`, if given, is the exact invocation — revealed
        /// in a gutter beneath the status line once the command runs past the
        /// escalation delay, so a slow or stuck command is debuggable.
        ///
        /// Enabled only when stderr is a TTY and verbosity is 0 — under
        /// `-v`/`-vv` the structured diagnostics take over, and a non-TTY (piped)
        /// context renders nothing.
        pub fn start(waiting_for: &str, command: Option<&str>) -> Self {
            let enabled = std::io::stderr().is_terminal() && crate::styling::verbosity() == 0;
            Self::start_with(waiting_for, command, enabled)
        }

        /// Dispatch helper picking enabled/disabled from an explicit flag, so
        /// tests exercise both branches without depending on the ambient stderr
        /// fd (see [`Progress::start_with`] for why).
        fn start_with(waiting_for: &str, command: Option<&str>, enabled: bool) -> Self {
            if enabled {
                Self::enabled(waiting_for, command)
            } else {
                Self::disabled()
            }
        }

        fn disabled() -> Self {
            Self {
                shared: Arc::new(WatchdogShared::new()),
                ticker: None,
            }
        }

        fn enabled(waiting_for: &str, command: Option<&str>) -> Self {
            Self::enabled_with_delays(
                waiting_for,
                command,
                WATCHDOG_STARTUP_DELAY,
                WATCHDOG_ESCALATE_DELAY,
            )
        }

        /// Spawn the ticker with explicit delays. The public path passes the
        /// module constants; tests pass millisecond delays so the escalation
        /// tier is exercisable without a 10-second wait.
        fn enabled_with_delays(
            waiting_for: &str,
            command: Option<&str>,
            startup_delay: Duration,
            escalate_delay: Duration,
        ) -> Self {
            let shared = Arc::new(WatchdogShared::new());
            let ticker = {
                let shared = Arc::clone(&shared);
                let waiting_for = waiting_for.to_owned();
                let command = command.map(str::to_owned);
                thread::spawn(move || {
                    watchdog_loop(
                        &shared,
                        &waiting_for,
                        command.as_deref(),
                        startup_delay,
                        escalate_delay,
                    )
                })
            };
            Self {
                shared,
                ticker: Some(ticker),
            }
        }

        /// Stop the watchdog and clear its block. Equivalent to dropping.
        pub fn finish(self) {
            drop(self);
        }
    }

    impl Drop for Watchdog {
        fn drop(&mut self) {
            if let Some(ticker) = self.ticker.take() {
                self.shared.done.store(true, Ordering::Relaxed);
                ticker.thread().unpark();
                let _ = ticker.join();
                // Clear the block the ticker left on screen. 0 rows means nothing
                // was drawn (a fast command that never tripped the startup delay),
                // so the terminal is left untouched.
                let rows = self.shared.rendered_rows.load(Ordering::Relaxed);
                if rows > 0 {
                    let _ = clear_block(&mut std::io::stderr().lock(), rows);
                }
            }
        }
    }

    fn watchdog_loop(
        shared: &WatchdogShared,
        waiting_for: &str,
        command: Option<&str>,
        startup_delay: Duration,
        escalate_delay: Duration,
    ) {
        let start = Instant::now();
        // park_timeout returns immediately on `unpark` from drop, so a command
        // that finishes before the delay neither renders nor blocks shutdown.
        while start.elapsed() < startup_delay {
            if shared.done.load(Ordering::Relaxed) {
                return;
            }
            thread::park_timeout(startup_delay.saturating_sub(start.elapsed()));
        }
        let mut prev_rows = 0usize;
        let mut last_block: Vec<String> = Vec::new();
        while !shared.done.load(Ordering::Relaxed) {
            let elapsed = start.elapsed();
            // Second tier: once the command has run long enough, reveal it in a
            // gutter beneath the status line.
            let escalated = command.is_some() && elapsed >= escalate_delay;
            if escalated {
                shared.escalated.store(true, Ordering::Relaxed);
            }
            let block = watchdog_block(
                waiting_for,
                elapsed.as_secs(),
                if escalated { command } else { None },
                crate::styling::terminal_width(),
            );
            // Redraw only when the content changes — the status counter ticks
            // once a second, the gutter appears once — so the block doesn't
            // flicker between the (sub-second) wake-ups.
            if block != last_block {
                let mut err = std::io::stderr().lock();
                prev_rows = render_block(&mut err, &block, prev_rows);
                shared.rendered_rows.store(prev_rows, Ordering::Relaxed);
                last_block = block;
            }
            thread::park_timeout(TICK_INTERVAL);
        }
    }

    /// Build the watchdog block: a dim status line, plus — once `command` is
    /// given (escalated) — that command in a bash-highlighted gutter beneath it
    /// (matching how worktrunk shows commands elsewhere, e.g. the LLM-failure
    /// error).
    ///
    /// Every returned string must occupy exactly one terminal row, or the
    /// in-place cursor math in [`render_block`]/[`clear_block`] desyncs from what
    /// the terminal actually wrapped and corrupts the lines above. So both lines
    /// are width-bounded rather than left to soft-wrap: the status is
    /// ANSI-aware-truncated to `width`, and the command gutter is *chopped* (not
    /// wrapped) so an over-wide invocation stays one row. `width` is the terminal
    /// width (`None` → unknown, leave the status unbounded); the loop passes the
    /// live width each tick so a resize is picked up.
    fn watchdog_block(
        waiting_for: &str,
        secs: u64,
        command: Option<&str>,
        width: Option<usize>,
    ) -> Vec<String> {
        let status = cformat!("<dim>○ Waiting for {waiting_for} ({secs}s)</>");
        let status = match width {
            Some(w) => crate::styling::truncate_visible(&status, w),
            None => status,
        };
        let mut lines = vec![status];
        if let Some(cmd) = command {
            lines.extend(
                crate::styling::format_bash_with_gutter_chopped(cmd)
                    .lines()
                    .map(str::to_owned),
            );
        }
        lines
    }

    /// Redraw the in-place block: clear the previous `prev_rows` rows, then write
    /// the new lines (joined, no trailing newline). Returns the new row count.
    /// Relies on each line being ≤ terminal width (one row), so `prev_rows`
    /// equals the visual rows occupied.
    fn render_block<W: Write>(w: &mut W, lines: &[String], prev_rows: usize) -> usize {
        let _ = w.queue(MoveToColumn(0));
        if prev_rows > 1 {
            let _ = w.queue(MoveUp(prev_rows as u16 - 1));
        }
        let _ = w.queue(Clear(ClearType::FromCursorDown));
        let _ = write!(w, "{}", lines.join("\n"));
        let _ = w.flush();
        lines.len()
    }

    /// Clear a `rows`-row block: move to its first row and clear downward.
    fn clear_block<W: Write>(w: &mut W, rows: usize) -> std::io::Result<()> {
        w.queue(MoveToColumn(0))?;
        if rows > 1 {
            w.queue(MoveUp(rows as u16 - 1))?;
        }
        w.queue(Clear(ClearType::FromCursorDown))?;
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
            assert!(Progress::start_with("Copying", false).ticker.is_none());
        }

        #[test]
        fn test_start_with_tty_is_enabled() {
            let p = Progress::start_with("Copying", true);
            assert!(p.ticker.is_some());
            p.finish();
        }

        #[test]
        fn test_enabled_lifecycle_counters_propagate() {
            let p = Progress::enabled("Copying");
            p.record(1024);
            p.record(2048);
            assert_eq!(p.totals(), (2, 3072));
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

        #[test]
        fn test_watchdog_block_status_only() {
            // Before escalation (no command) the block is a single dim status
            // row. A wide width leaves the status untruncated.
            let block = watchdog_block("the commit message", 4, None, Some(200));
            assert_eq!(block.len(), 1);
            assert!(block[0].contains("Waiting for the commit message"));
            assert!(block[0].contains("(4s)"));
            assert!(block[0].contains('○'));
            assert!(!block[0].contains('…'));
        }

        #[test]
        fn test_watchdog_block_escalated_adds_command_gutter() {
            // Escalated, the command is revealed in a gutter row beneath the
            // status. Assert on a bare word ("claude") rather than the whole
            // command, since bash highlighting interleaves ANSI codes between
            // tokens (a word itself is never split mid-token).
            let block = watchdog_block(
                "the commit message",
                12,
                Some("claude --model=haiku"),
                Some(200),
            );
            assert!(block.len() >= 2);
            assert!(block[0].contains("Waiting for the commit message"));
            assert!(block.join("\n").contains("claude"));
        }

        #[test]
        fn test_watchdog_block_status_truncated_to_width() {
            // A width narrower than the status text truncates it to one row
            // (visible width within budget, ellipsis appended) — without this the
            // soft-wrapped second row desyncs the in-place cursor math.
            use ansi_str::AnsiStr;
            use unicode_width::UnicodeWidthStr;
            let block = watchdog_block("the squash commit message", 1234, None, Some(20));
            assert_eq!(block.len(), 1);
            let visible = block[0].ansi_strip();
            assert!(UnicodeWidthStr::width(visible.as_ref()) <= 20);
            assert!(block[0].contains('…'));
        }

        #[test]
        fn test_watchdog_start_with_non_tty_is_disabled() {
            assert!(
                Watchdog::start_with("the commit message", None, false)
                    .ticker
                    .is_none()
            );
        }

        #[test]
        fn test_watchdog_start_with_tty_is_enabled() {
            let w = Watchdog::start_with("the commit message", None, true);
            assert!(w.ticker.is_some());
            w.finish();
        }

        #[test]
        fn test_watchdog_renders_after_startup_delay() {
            // Tiny startup delay, escalation far off: the status renders but the
            // command gutter never appears.
            let w = Watchdog::enabled_with_delays(
                "the commit message",
                Some("sh -c 'claude -p'"),
                Duration::from_millis(10),
                Duration::from_secs(3600),
            );
            std::thread::sleep(Duration::from_millis(120));
            assert!(w.shared.rendered_rows.load(Ordering::Relaxed) > 0);
            assert!(!w.shared.escalated.load(Ordering::Relaxed));
            w.finish();
        }

        #[test]
        fn test_watchdog_fast_command_leaves_no_trace() {
            // A command that finishes before the startup delay must never draw a
            // block — so Drop has nothing to clear and leaves no escape in output.
            let w = Watchdog::enabled("the commit message", None);
            assert_eq!(w.shared.rendered_rows.load(Ordering::Relaxed), 0);
            w.finish();
        }

        #[test]
        fn test_watchdog_escalates_and_grows_the_block() {
            // Short startup + escalation delays so the second tier fires fast; the
            // revealed gutter makes the block more than one row.
            let w = Watchdog::enabled_with_delays(
                "the commit message",
                Some("sh -c 'claude -p'"),
                Duration::from_millis(10),
                Duration::from_millis(30),
            );
            std::thread::sleep(Duration::from_millis(150));
            assert!(w.shared.escalated.load(Ordering::Relaxed));
            assert!(w.shared.rendered_rows.load(Ordering::Relaxed) >= 2);
            w.finish();
        }

        #[test]
        fn test_watchdog_no_escalation_without_command() {
            // No command → the gutter never appears, however long it runs, so the
            // block stays a single status row.
            let w = Watchdog::enabled_with_delays(
                "the commit message",
                None,
                Duration::from_millis(10),
                Duration::from_millis(30),
            );
            std::thread::sleep(Duration::from_millis(150));
            assert_eq!(w.shared.rendered_rows.load(Ordering::Relaxed), 1);
            assert!(!w.shared.escalated.load(Ordering::Relaxed));
            w.finish();
        }
    }
}

#[cfg(not(feature = "cli"))]
mod imp {
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    /// Spinner-less stub when the `cli` feature is off. The spinner depends on
    /// `crossterm`, which is only pulled in by `cli`; library consumers that
    /// disable default features get this thread-free, render-free type. The
    /// counters still accumulate so [`Progress::totals`] reports the same
    /// numbers in both builds.
    pub struct Progress {
        files: AtomicUsize,
        bytes: AtomicU64,
    }

    impl Progress {
        pub fn start(_verb: &'static str) -> Self {
            Self::disabled()
        }

        pub fn disabled() -> Self {
            Self {
                files: AtomicUsize::new(0),
                bytes: AtomicU64::new(0),
            }
        }

        pub fn record(&self, bytes: u64) {
            self.files.fetch_add(1, Ordering::Relaxed);
            self.bytes.fetch_add(bytes, Ordering::Relaxed);
        }

        pub fn totals(&self) -> (usize, u64) {
            (
                self.files.load(Ordering::Relaxed),
                self.bytes.load(Ordering::Relaxed),
            )
        }

        pub fn finish(self) {}
    }

    /// Spinner-less stub when the `cli` feature is off — mirrors [`Progress`]'s
    /// stub. The watchdog has no counters to keep, so it carries no state.
    pub struct Watchdog;

    impl Watchdog {
        pub fn start(_waiting_for: &str, _command: Option<&str>) -> Self {
            Self
        }

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

    // Not cfg-gated: covers the disabled-state counting contract in both the
    // `cli` implementation and the no-`cli` stub.
    #[test]
    fn test_disabled_still_counts() {
        let p = Progress::disabled();
        p.record(1_000_000);
        p.record(2_000_000);
        assert_eq!(p.totals(), (2, 3_000_000));
        p.finish();
    }
}
