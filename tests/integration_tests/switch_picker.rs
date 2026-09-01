#![cfg(feature = "shell-integration-tests")]
//! TUI snapshot tests for `wt switch` interactive picker
//!
//! These tests use PTY execution combined with vt100 terminal emulation to capture
//! what the user actually sees on screen, enabling meaningful snapshot testing of
//! the skim-based TUI interface. They run on every platform — `portable_pty` uses a
//! ConPTY on Windows (see `tests/common/pty.rs`), and capturing the vt100-emulated
//! grid (not raw escape sequences) keeps the snapshots backend-agnostic.
//!
//! ## Capture-Before-Abort Pattern
//!
//! Abort tests snapshot the screen BEFORE sending Escape, not after. Skim's teardown
//! is asynchronous — sending Escape races with rendering, producing non-deterministic
//! output (variable border painting, incomplete rows). By capturing the stable pre-abort
//! state, we eliminate this entire class of flakiness. After capture, Escape is sent and
//! only the exit code is checked.
//!
//! ## Timing Strategy
//!
//! Instead of fixed delays (which are either too short on slow CI or wastefully
//! long on fast machines), we poll for screen stabilization:
//!
//! - **Long timeouts** (30s) ensure reliability on slow CI
//! - **Fast polling** (10ms) means tests complete quickly when things work
//! - **Content-based readiness** detects when skim has rendered ("> " prompt)
//! - **Stabilization detection** waits for screen to stop changing
//! - **Content expectations** wait for async preview content to load (e.g., "diff --git")

use crate::common::mock_commands::{MockConfig, MockResponse, mock_calls};
use crate::common::{TEST_EPOCH, TestRepo, wt_bin};
use insta::assert_snapshot;
use portable_pty::CommandBuilder;
use rstest::rstest;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Terminal dimensions for TUI tests
const TERM_ROWS: u16 = 30;
const TERM_COLS: u16 = 120;

/// Maximum time to wait for skim to become ready (show "> " prompt).
/// Long timeout ensures reliability on slow CI.
const READY_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum time to wait for screen to stabilize after input.
/// Long timeout ensures reliability on slow CI where skim's async item loading
/// and preview commands can be very slow under heavy load. Fast polling (10ms)
/// means tests complete quickly when things work — the long timeout only matters
/// in worst-case scenarios.
const STABILIZE_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum time to wait for the picker child to exit after the terminating
/// keystroke (Enter to switch, Escape to abort). A clean switch or abort exits in
/// well under a second, but under heavy CI parallelism the final git work (or
/// skim's Windows terminal teardown) can lag. Generous like
/// `READY_TIMEOUT`/`STABILIZE_TIMEOUT` for the same reason — fast polling means
/// the common case still returns at once. Reaching it is a hang, and
/// [`wait_for_exit`] panics rather than reporting an exit code for it.
const CHILD_EXIT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the PTY must stay silent after the child exits before its final
/// frame counts as complete. Long enough that a loaded runner's last flush lands
/// in the parser; short enough to add no meaningful cost per test.
const POST_EXIT_QUIET: Duration = Duration::from_millis(250);

/// Ceiling on the post-exit drain, in case something else on the PTY keeps
/// writing (a detached background hook shares the terminal).
const POST_EXIT_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// How long screen must be unchanged to consider it "stable".
/// Must be long enough for preview content to load (preview commands run async).
/// 500ms balances reliability (allows preview to complete) with speed.
/// Panel switches trigger async git commands that may take time.
const STABLE_DURATION: Duration = Duration::from_millis(500);

/// Polling interval when waiting for output.
/// Fast polling ensures tests complete quickly when ready.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// How often a cursor-arrow wait re-issues its (idempotent) arrow while the `>`
/// pointer has not yet settled on the target row. Long enough not to thrash the
/// picker; short enough to retry many times within [`STABILIZE_TIMEOUT`] after
/// an async item-list refresh resets the cursor to the top.
const CURSOR_REISSUE_INTERVAL: Duration = Duration::from_secs(1);

/// Picker tests shape their own linked-worktree topology. Starting from the
/// cached main-only variant avoids constructing and immediately removing the
/// standard fixture's three unrelated linked worktrees in every PTY test.
#[rstest::fixture]
fn repo() -> TestRepo {
    TestRepo::standard_main_only()
}

/// Columns that split the list and preview panels in the 120-col test terminal.
/// skim 4.x draws the │ separator at col 59, with the list to its left (cols
/// 0..59) and the preview interior to its right (cols 60..120). Slicing around
/// col 59 drops the separator from both panels — the border glyph renders
/// inconsistently across platforms.
const LIST_WIDTH: u16 = 59;
const PREVIEW_START_COL: u16 = 60;

/// Full screen content as rows of text.
///
/// Trailing whitespace is trimmed from each row because `vt100::rows()` pads
/// rows to the full column width with spaces. This padding is terminal buffer
/// fill, not meaningful content, and varies across platforms. Trailing empty
/// lines are also removed (unwritten terminal rows become empty after trim).
fn screen_text(parser: &vt100::Parser) -> String {
    parser
        .screen()
        .rows(0, TERM_COLS)
        .map(|row| row.trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string()
}

/// Result of executing a command in a PTY, holding the parsed terminal state.
struct PtyResult {
    parser: vt100::Parser,
    exit_code: i32,
}

impl PtyResult {
    /// Full screen content as rows of text — see [`screen_text`].
    fn screen(&self) -> String {
        screen_text(&self.parser)
    }

    /// List and preview panel content, split at the skim border column.
    /// Avoids the │ border character that causes cross-platform rendering issues.
    fn panels(&self) -> (String, String) {
        let screen = self.parser.screen();
        let list = list_pane_text(screen);
        let preview = screen
            .rows(PREVIEW_START_COL, TERM_COLS - PREVIEW_START_COL)
            .map(|row| row.trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
            .trim_end()
            .to_string();
        (list, preview)
    }
}

/// The list pane: screen columns left of the skim border, trailing whitespace
/// trimmed (vt100 pads rows to the full width; that padding is buffer fill, not
/// content, and varies across platforms).
fn list_pane_text(screen: &vt100::Screen) -> String {
    screen
        .rows(0, LIST_WIDTH)
        .map(|row| row.trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string()
}

/// The preview pane: screen columns right of the skim border (the panel interior),
/// trailing whitespace trimmed. Mirrors the split [`PtyResult::panels`] uses.
fn preview_pane_text(screen: &vt100::Screen) -> String {
    screen
        .rows(PREVIEW_START_COL, TERM_COLS - PREVIEW_START_COL)
        .map(|row| row.trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string()
}

/// Assert that exit code is valid for skim abort (0, 1, or 130)
fn assert_valid_abort_exit_code(exit_code: i32) {
    // Skim exits with:
    // - 0: successful selection or no items
    // - 1: normal abort (escape key)
    // - 130: abort via SIGINT (128 + signal 2)
    assert!(
        exit_code == 0 || exit_code == 1 || exit_code == 130,
        "Unexpected exit code: {} (expected 0, 1, or 130 for skim abort)",
        exit_code
    );
}

/// Assert the exit code of a *successful* picker-create (alt-c).
///
/// A create that succeeds exits 0 — `run_picker` returns `Ok(())` after the
/// `SwitchPipeline` runs, and the "cannot cd — shell integration not installed"
/// line is a warning, not an error. Callers still prove the create succeeded the
/// deterministic way: the new branch and worktree exist in git afterward.
///
/// On Windows the picker process has been observed to *self-exit* with code 1
/// after a fully-correct create under the advisory `affected tests (windows)`
/// leg's load, while the required `test (windows)` leg passed 0 on the same SHA
/// (PR #3424 CI: branch + worktree created, pre-start hook ran, only the exit
/// code diverged; the test finished in ~5.6s, well under `CHILD_EXIT_TIMEOUT`,
/// so it was a genuine self-exit and not a harness kill). This is the same
/// "slow-but-successful exit reports 1 on Windows" class the abort helpers
/// already tolerate via [`assert_valid_abort_exit_code`]. Tolerate it here so a
/// correct create doesn't false-fail the advisory leg, while keeping the exit
/// code strict everywhere it is reliable — a create that genuinely *fails*
/// leaves no branch, so the git-state assertions remain the real guard.
fn assert_valid_create_exit_code(exit_code: i32) {
    let valid = if cfg!(windows) {
        exit_code == 0 || exit_code == 1
    } else {
        exit_code == 0
    };
    assert!(
        valid,
        "Unexpected create exit code: {} (expected 0{})",
        exit_code,
        if cfg!(windows) {
            ", or 1 for the Windows slow-exit quirk"
        } else {
            ""
        }
    );
}

/// Check if skim is ready (shows "> " prompt indicating it's accepting input)
fn is_skim_ready(screen_content: &str) -> bool {
    // Skim shows "> " at the start of the prompt line when accepting input.
    screen_content.starts_with("> ") || screen_content.contains("\n> ")
}

/// Live handles for a booted picker PTY session.
///
/// `_master` is held only to keep the pseudo-terminal open for the session's
/// lifetime; it is never read. Dropping it tears down the Windows ConPTY, after
/// which every write to `writer` fails with `BrokenPipe`. On Unix `take_writer()`
/// hands back an independent fd, so the master's lifetime is irrelevant — which
/// is exactly why dropping it early passes locally and on Linux/macOS CI yet
/// wipes out every picker test on Windows.
struct PickerSession {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
    writer: crate::common::pty::SharedPtyWriter,
    rx: mpsc::Receiver<Vec<u8>>,
    parser: vt100::Parser,
}

/// Spawn `command` in an isolated PTY, wait until skim is ready and the initial
/// render has stabilized, and return the live session handles. Every picker PTY
/// helper shares this boot sequence; they differ only in how they drive the
/// session and capture its frames.
fn boot_picker_pty(
    command: &str,
    args: &[&str],
    working_dir: &Path,
    env_vars: &[(String, String)],
) -> PickerSession {
    let pair = crate::common::open_pty_with_size(TERM_ROWS, TERM_COLS);

    let mut cmd = CommandBuilder::new(command);
    for arg in args {
        cmd.arg(arg);
    }
    cmd.cwd(working_dir);

    // Isolated environment with coverage passthrough
    crate::common::configure_pty_command(&mut cmd);
    cmd.env("TERM", "xterm-256color");

    // Test-specific environment variables
    for (key, value) in env_vars {
        cmd.env(key, value);
    }

    let child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);

    let reader = pair.master.try_clone_reader().unwrap();
    let writer: crate::common::pty::SharedPtyWriter =
        Arc::new(Mutex::new(pair.master.take_writer().unwrap()));

    // Drain PTY output into a channel; the reader thread also answers skim's
    // startup cursor-position query (see `spawn_pty_reader_answering_queries`).
    let rx = crate::common::pty::spawn_pty_reader_answering_queries(reader, Arc::clone(&writer));

    let mut parser = vt100::Parser::new(TERM_ROWS, TERM_COLS, 0);

    // Wait for skim to be ready (show "> " prompt)
    let start = Instant::now();
    loop {
        while let Ok(chunk) = rx.try_recv() {
            parser.process(&chunk);
        }

        let screen_content = parser.screen().contents();
        if is_skim_ready(&screen_content) {
            break;
        }

        if start.elapsed() > READY_TIMEOUT {
            eprintln!(
                "Warning: Timed out waiting for skim ready state. Screen content:\n{}",
                screen_content
            );
            break;
        }

        std::thread::sleep(POLL_INTERVAL);
    }

    // Wait for initial render to stabilize
    wait_for_stable(&rx, &mut parser);

    PickerSession {
        child,
        _master: pair.master,
        writer,
        rx,
        parser,
    }
}

/// Send Escape to abort the picker, then drain and discard remaining output —
/// the caller has already captured the frame it wants, so teardown bytes must
/// not reach its parser. Consumes the handles and returns the exit code.
fn abort_and_exit_code(
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: crate::common::pty::SharedPtyWriter,
    rx: mpsc::Receiver<Vec<u8>>,
) -> i32 {
    {
        let mut w = writer.lock().unwrap();
        w.write_all(b"\x1b").unwrap();
        w.flush().unwrap();
    }
    drop(writer);

    // Teardown bytes are discarded rather than parsed, so this waits with a
    // sink parser — the caller's frame was captured before the Escape.
    let mut sink = vt100::Parser::new(TERM_ROWS, TERM_COLS, 0);
    wait_for_exit(&mut child, &rx, &mut sink, "Escape")
}

/// Wait for the picker child to exit, then drain its final output into `parser`
/// and return its exit code.
///
/// A child still running after [`CHILD_EXIT_TIMEOUT`] is hung, and the harness
/// must not turn that into an exit code: `Child::kill` on Windows is
/// `TerminateProcess(proc, 1)`, so a killed hang and a wt that failed on its own
/// both report 1 — the ambiguity that had a *correct* picker-create reported as a
/// mysterious Windows "self-exit 1" (#3427). Panic on the hang instead, so every
/// exit code a caller asserts on is one the child chose.
///
/// `terminator` names the keystroke that should have ended the session, for the
/// panic message.
fn wait_for_exit(
    child: &mut Box<dyn portable_pty::Child + Send + Sync>,
    rx: &mpsc::Receiver<Vec<u8>>,
    parser: &mut vt100::Parser,
    terminator: &str,
) -> i32 {
    let start = Instant::now();
    let mut exited = false;
    while start.elapsed() < CHILD_EXIT_TIMEOUT {
        if child.try_wait().unwrap().is_some() {
            exited = true;
            break;
        }
        std::thread::sleep(POLL_INTERVAL);
    }

    if !exited {
        let _ = child.kill();
        let _ = child.wait();
        drain_until_quiet(rx, parser);
        panic!(
            "Picker child did not exit within {CHILD_EXIT_TIMEOUT:?} of {terminator}.\n\
             Screen content:\n{}",
            screen_text(parser)
        );
    }

    // Drain to quiet, not once: `try_wait` reports the child reaped, but its
    // final writes can still be in the PTY (and in the reader thread) at that
    // moment, so a single non-blocking sweep drops the tail — which is exactly
    // where a failing run's explanation lives (wt's error line, the last
    // warning). Without this the captured frame reads as a silent exit.
    drain_until_quiet(rx, parser);

    child.wait().unwrap().exit_code() as i32
}

/// Execute a command in a PTY with a sequence of inputs and optional content expectations.
///
/// Each input is `(input_bytes, expected_content)`:
/// - `expected_content`: a substring that must appear on screen before the input is considered
///   processed. Required for async preview content that lands later than the prompt update.
///
/// Example: `[("\x1b[B", None), ("\x1b3", Some("diff --git"))]`
/// - After Down (move cursor to the next worktree): just wait for the screen to settle.
/// - After Alt-3 (switch to the committed diff panel): wait until "diff --git" appears.
fn exec_in_pty_with_input_expectations(
    command: &str,
    args: &[&str],
    working_dir: &Path,
    env_vars: &[(String, String)],
    inputs: &[(&str, Option<&str>)],
) -> PtyResult {
    let PickerSession {
        mut child,
        _master,
        writer,
        rx,
        mut parser,
    } = boot_picker_pty(command, args, working_dir, env_vars);

    // Send each input and wait for screen to stabilize after each
    for (input, expected_content) in inputs {
        send_input_awaiting_content(&writer, &rx, &mut parser, input, *expected_content);
    }

    // Release the main thread's writer handle. The reader thread holds the
    // other Arc clone until the PTY drains, so this no longer drives stdin EOF.
    // The picker exits on Accept/Escape.
    drop(writer);

    let exit_code = wait_for_exit(&mut child, &rx, &mut parser, "the last input");

    PtyResult { parser, exit_code }
}

/// Drain the PTY into `parser` until the child's output goes quiet — nothing new
/// for [`POST_EXIT_QUIET`], or [`POST_EXIT_DRAIN_TIMEOUT`] elapsed.
///
/// Used after the child exits, where the goal is a complete final frame rather
/// than a stable one. The bound keeps a still-chatty PTY (a detached background
/// hook writing to the same terminal) from holding the test.
fn drain_until_quiet(rx: &mpsc::Receiver<Vec<u8>>, parser: &mut vt100::Parser) {
    let start = Instant::now();
    let mut last_chunk = Instant::now();
    while start.elapsed() < POST_EXIT_DRAIN_TIMEOUT && last_chunk.elapsed() < POST_EXIT_QUIET {
        match rx.recv_timeout(POLL_INTERVAL) {
            Ok(chunk) => {
                parser.process(&chunk);
                last_chunk = Instant::now();
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            // Reader thread hit PTY EOF and dropped its sender: nothing more is
            // coming, and the channel is already empty.
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// Execute a command in a PTY, capture screen state, then abort with Escape.
///
/// This is the key fix for flaky abort snapshot tests. The problem: snapshotting
/// screen state AFTER sending Escape races with skim's teardown, producing
/// non-deterministic output (variable border painting, incomplete rows, trailing
/// whitespace). The fix: capture the stable screen BEFORE aborting, then only
/// check exit code after abort.
///
/// `pre_abort_inputs` are sent before capturing (e.g., typing a filter or switching
/// preview panels). Each input can optionally specify content that must appear before
/// the screen is considered stable.
fn exec_in_pty_capture_before_abort(
    command: &str,
    args: &[&str],
    working_dir: &Path,
    env_vars: &[(String, String)],
    pre_abort_inputs: &[(&str, Option<&str>)],
) -> PtyResult {
    let PickerSession {
        child,
        _master,
        writer,
        rx,
        mut parser,
    } = boot_picker_pty(command, args, working_dir, env_vars);

    // Send pre-abort inputs (filter text, panel switches, etc.)
    for (input, expected_content) in pre_abort_inputs {
        send_input_awaiting_content(&writer, &rx, &mut parser, input, *expected_content);
    }

    // === CAPTURE: screen state is now stable — snapshot BEFORE aborting ===
    // The parser retains this state because we stop feeding output to it.
    let exit_code = abort_and_exit_code(child, writer, rx);

    PtyResult { parser, exit_code }
}

/// Drive the picker to a settled baseline, capture the list pane, send a
/// sequence of keys, capture the list pane again, then abort. Returns
/// `(baseline, after, exit_code)` so the caller can assert the keys left the
/// list byte-for-byte unchanged.
///
/// This is the invariant form of a "this key is a visual no-op" test. It
/// commits no frame, so picker column-layout changes never touch it, and there
/// is no frozen baseline that can capture a different async-render frame than a
/// sibling snapshot test — the failure mode the old committed snapshot hit.
/// Both captures bracket only the probe keys and are each taken once the screen
/// has settled, so a genuine no-op yields byte-identical frames.
fn exec_in_pty_capture_noop_probe(
    command: &str,
    args: &[&str],
    working_dir: &Path,
    env_vars: &[(String, String)],
    baseline_inputs: &[(&str, Option<&str>)],
    probe_inputs: &[(&str, Option<&str>)],
) -> (String, String, i32) {
    let PickerSession {
        child,
        _master,
        writer,
        rx,
        mut parser,
    } = boot_picker_pty(command, args, working_dir, env_vars);

    // Settle to the baseline, then capture it.
    for (input, expected_content) in baseline_inputs {
        send_input_awaiting_content(&writer, &rx, &mut parser, input, *expected_content);
    }
    let baseline = list_pane_text(parser.screen());

    // Send the probe keys, then capture again.
    for (input, expected_content) in probe_inputs {
        send_input_awaiting_content(&writer, &rx, &mut parser, input, *expected_content);
    }
    let after = list_pane_text(parser.screen());

    let exit_code = abort_and_exit_code(child, writer, rx);
    (baseline, after, exit_code)
}

/// Wait for screen content to stabilize (no changes for STABLE_DURATION)
fn wait_for_stable(rx: &mpsc::Receiver<Vec<u8>>, parser: &mut vt100::Parser) {
    wait_for_stable_with_content(rx, parser, None);
}

/// Wait for screen content to stabilize, optionally requiring specific content.
///
/// If `expected_content` is provided, waits until the screen contains that string
/// AND has stabilized. This is essential for async preview panels where the initial
/// render may show placeholder content before the actual data loads.
///
/// Tip: avoid including the panel border character (`│`) in `expected_content` —
/// its rendering varies by platform and terminal, causing flaky assertions.
fn wait_for_stable_with_content(
    rx: &mpsc::Receiver<Vec<u8>>,
    parser: &mut vt100::Parser,
    expected_content: Option<&str>,
) {
    let describe = expected_content.map(|c| format!("expected content {c:?}"));
    wait_for_stable_until(
        rx,
        parser,
        |screen| expected_content.is_none_or(|c| screen.contains(c)),
        describe.as_deref(),
        None,
    );
}

/// Wait until the list-pane cursor pointer lands on the row for `name`, then
/// settles.
///
/// skim draws its `> ` pointer on the selected row on every render of the item
/// list, so the pointer is a race-free signal of cursor position. The preview
/// pane is not: skim only repaints it on a selection-*change* event
/// (`on_selection_changed` → `Event::RunPreview`), so a cursor move driven by a
/// `Custom` action — the alt-x sticky reposition — leaves the preview showing
/// the previous row until something else repaints it. Gating cursor-position
/// assertions on the preview text therefore races the picker's async render;
/// gating on the pointer does not.
///
/// The query line also starts with `> `, but these helpers navigate by cursor
/// and never type, so the query stays empty — only the selected row both starts
/// with `>` and carries a worktree `name`, which uniquely picks it out.
fn wait_for_cursor_on_row(rx: &mpsc::Receiver<Vec<u8>>, parser: &mut vt100::Parser, name: &str) {
    let describe = format!("the cursor (> pointer) on row {name:?}");
    wait_for_stable_until(
        rx,
        parser,
        |screen| cursor_points_at(screen, name),
        Some(&describe),
        None,
    );
}

/// True when the list-pane `>` pointer is on the row for `name`.
///
/// skim draws its pointer at the start of the selected row's line on every
/// item-list render. The query line also starts with `> `, but the helpers that
/// rely on this navigate by cursor and never type, so the query stays empty —
/// only the selected row both starts with `>` and carries a worktree `name`,
/// which uniquely picks it out.
///
/// The match is scoped to the list pane (cols `0..LIST_WIDTH`). The preview pane
/// shares each physical row to the right of the border, so `name` is sought only
/// in the row's own list text — otherwise a token that also renders in the
/// preview (e.g. a PR title carrying a branch word) could satisfy the check from
/// the wrong row.
fn cursor_points_at(screen: &str, name: &str) -> bool {
    screen.lines().any(|line| {
        let list: String = line.chars().take(LIST_WIDTH as usize).collect();
        list.starts_with('>') && list.contains(name)
    })
}

/// Drive the PTY reader until the screen satisfies `ready` and then settles, or
/// the stabilization timeout elapses.
///
/// `ready` is evaluated against the full screen contents. When `describe` is
/// `Some`, a timeout that never saw `ready` panics with diagnostics (naming the
/// awaited condition); when it is `None` the caller has no readiness condition
/// (stability only) and `ready` is ignored.
///
/// Handles a subtle race: skim may keep redrawing cosmetically (cursor
/// repositioning, border repaints) even after the meaningful content is on
/// screen, which keeps resetting the "no changes for STABLE_DURATION" timer. So
/// once `ready` holds, we track how long it has held continuously and accept
/// stability after STABLE_DURATION even if the screen keeps churning. With no
/// readiness condition there is nothing to find, so the screen must settle the
/// hard way (the cosmetic-redraw fallback never engages).
///
/// `nudge`, when `Some`, is invoked every [`CURSOR_REISSUE_INTERVAL`] while
/// `ready` is still unmet. It exists for the cursor-arrow caller: an idempotent
/// Up/Down arrow re-issued to drive the `>` pointer back onto its target row
/// after an async item-list refresh (CI status / PR markers landing) reset the
/// cursor to the top. Late *preview* content needs no nudge — the picker
/// repaints a preview on its own once its background compute lands (see
/// `PreviewNotifier`), so preview-content callers pass `None` and the poll just
/// waits for `ready`.
fn wait_for_stable_until(
    rx: &mpsc::Receiver<Vec<u8>>,
    parser: &mut vt100::Parser,
    ready: impl Fn(&str) -> bool,
    describe: Option<&str>,
    nudge: Option<&dyn Fn()>,
) {
    let start = Instant::now();
    let mut last_change = Instant::now();
    let mut last_content = parser.screen().contents();
    // Tracks when `ready` first held continuously on screen. Used as a fallback
    // stability signal when skim keeps redrawing cosmetically.
    let mut ready_since: Option<Instant> = None;
    let mut last_nudge = Instant::now();
    let has_condition = describe.is_some();

    while start.elapsed() < STABILIZE_TIMEOUT {
        // Drain available output
        while let Ok(chunk) = rx.try_recv() {
            parser.process(&chunk);
        }

        let current_content = parser.screen().contents();
        if current_content != last_content {
            last_content = current_content.clone();
            last_change = Instant::now();
        }

        let content_ready = if has_condition {
            let found = ready(&current_content);
            if found {
                ready_since.get_or_insert(Instant::now());
            } else {
                // Condition lost (e.g., skim full redraw) — reset
                ready_since = None;
            }
            found
        } else {
            true
        };

        // Primary: screen hasn't changed for STABLE_DURATION and content is ready
        if last_change.elapsed() >= STABLE_DURATION && content_ready {
            return;
        }

        // Fallback (only with a readiness condition): if it has held continuously
        // for STABLE_DURATION, consider the screen stable even while skim keeps
        // doing cosmetic redraws (cursor repositioning, border repaints).
        if let Some(found_time) = ready_since
            && found_time.elapsed() >= STABLE_DURATION
        {
            return;
        }

        // While the readiness condition is still unmet, periodically re-issue the
        // nudge (the cursor-arrow caller's idempotent arrow). An async item-list
        // refresh can reset skim's cursor to the top after the first arrow, so a
        // single keystroke would strand the pointer; re-issuing drives it back
        // onto the target row until the list stops refreshing.
        if !content_ready
            && let Some(nudge) = nudge
            && last_nudge.elapsed() >= CURSOR_REISSUE_INTERVAL
        {
            nudge();
            last_nudge = Instant::now();
        }

        std::thread::sleep(POLL_INTERVAL);
    }

    // Timeout: if a condition was specified but never held, fail with diagnostics
    // instead of proceeding to a guaranteed assertion mismatch.
    if let Some(desc) = describe
        && !ready(&last_content)
    {
        panic!(
            "Timed out after {:?} waiting for {desc} to appear on screen.\n\
             Screen content:\n{}",
            STABILIZE_TIMEOUT, last_content
        );
    }

    // Stability-only timeout (no condition, or condition present but unstable) —
    // warn but proceed (test may still pass with current screen state)
    eprintln!(
        "Warning: Screen did not fully stabilize within {:?}",
        STABILIZE_TIMEOUT
    );
}

/// True for a Up/Down cursor arrow (`ESC [ A` / `ESC [ B`). Arrow navigation
/// clamps at the list ends, so re-issuing one is idempotent there — safe to
/// repeat while waiting for the cursor to reach a target row.
fn is_cursor_arrow(input: &str) -> bool {
    matches!(input.as_bytes(), [0x1b, b'[', b'A' | b'B'])
}

/// Send `input`, then wait for the screen to satisfy the per-input expectation
/// and settle.
///
/// For a Up/Down cursor arrow carrying `expected_content`, the content names the
/// target row and the wait re-issues the arrow every [`CURSOR_REISSUE_INTERVAL`]
/// until the list `>` pointer lands on it. A single arrow is unreliable on rows
/// that decorate asynchronously (CI status / PR markers): when the background
/// resolution lands it refreshes skim's item list, which resets the cursor to the
/// top, stranding the pointer on the primary worktree. That is a Windows-CI flake
/// observed with the cursor stuck on `main`, where the complete-diff tab showed the
/// primary's empty diff and the awaited `diff --git` never appeared. Re-issuing
/// the idempotent arrow drives the cursor back down after any reset; the wait
/// returns only once the pointer holds on the target through [`STABLE_DURATION`],
/// by which point the list has stopped refreshing.
///
/// Every other input — including an Alt-<digit> preview-tab switch — falls back
/// to a plain [`wait_for_stable_with_content`]. Late preview content needs no
/// re-issue: the picker repaints a preview on its own once its background compute
/// lands (see `PreviewNotifier`), so a diff or forge fetch that finishes after
/// the keystroke surfaces without one — the poll just waits for it. The
/// non-idempotent inputs (Tab, filter text, Enter) must not be repeated anyway.
fn send_input_awaiting_content(
    writer: &crate::common::pty::SharedPtyWriter,
    rx: &mpsc::Receiver<Vec<u8>>,
    parser: &mut vt100::Parser,
    input: &str,
    expected_content: Option<&str>,
) {
    let send = || {
        let mut w = writer.lock().unwrap();
        w.write_all(input.as_bytes()).unwrap();
        w.flush().unwrap();
    };
    send();

    match expected_content {
        Some(name) if is_cursor_arrow(input) => {
            let describe = format!("the cursor (> pointer) on row {name:?}");
            wait_for_stable_until(
                rx,
                parser,
                |screen| cursor_points_at(screen, name),
                Some(&describe),
                Some(&send),
            );
        }
        _ => wait_for_stable_with_content(rx, parser, expected_content),
    }
}

/// Create insta settings with filters for switch picker snapshot stability.
///
/// Replaces the manual `normalize_output()` approach with declarative insta filters.
/// Since `rows()` returns plain text (no ANSI codes, no OSC 8 hyperlinks),
/// `add_pty_filters()` and `strip_osc8_hyperlinks()` are not needed.
fn switch_picker_settings(repo: &TestRepo) -> insta::Settings {
    let mut settings = crate::common::setup_snapshot_settings(repo);

    // Query line has timing variations (shows typed chars at different rates).
    // \A anchors to absolute start of string, matching only the first line.
    settings.add_filter(r"\A> [^\n]*", "> [QUERY]");

    // Commit hashes (7-8 hex chars)
    settings.add_filter(r"\b[0-9a-f]{7,8}\b", "[HASH]");

    // Truncated commit hashes (6+ hex chars followed by ..) in narrow columns
    settings.add_filter(r"\b[0-9a-f]{6,8}\.\.", "[HASH]..");

    // Relative timestamps (1d, 16h, etc.)
    settings.add_filter(r"\b\d+[dhms]\b", "[TIME]");

    settings
}

#[rstest]
fn test_switch_picker_abort_with_escape(repo: TestRepo) {
    // Remove origin so snapshots don't show origin/main
    repo.run_git(&["remote", "remove", "origin"]);
    let env_vars = repo.test_env_vars();
    let result = exec_in_pty_capture_before_abort(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
        &[], // No inputs before abort
    );

    assert_valid_abort_exit_code(result.exit_code);

    let (list, preview) = result.panels();
    let settings = switch_picker_settings(&repo);
    settings.bind(|| {
        assert_snapshot!("switch_picker_abort_escape_list", list);
        assert_snapshot!("switch_picker_abort_escape_preview", preview);
    });
}

/// A branch name containing `/` (`feature/auth`) must keep its place in collect
/// order on the empty-query view — it must not sink below plainer names. skim's
/// empty-query engine scores every row `(score=0, begin=0)`, so a `PathName`
/// tiebreak would collapse to `path_name_offset` there and demote every
/// slash-bearing row (a `feature/…` branch, a `/`-gutter row). The picker uses
/// the default `[Score, Begin, End]` tiebreak precisely so the empty query keeps
/// the order `collect` produced (current, main, newest-first). Created
/// `feature/auth` first so collect ranks it ahead of `plain-branch` (equal test
/// timestamps fall back to worktree-creation order). Asserts the relative order
/// directly rather than freezing a frame, so column-layout and async-render
/// timing can't drift it.
#[rstest]
fn test_switch_picker_slashed_branch_keeps_collect_order(mut repo: TestRepo) {
    // Remove origin so the list doesn't show origin/main
    repo.run_git(&["remote", "remove", "origin"]);
    repo.add_worktree("feature/auth");
    repo.add_worktree("plain-branch");

    let env_vars = repo.test_env_vars();
    let result = exec_in_pty_capture_before_abort(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
        // Settle: wait for the last row to render before capturing.
        &[("", Some("plain-branch"))],
    );

    assert_valid_abort_exit_code(result.exit_code);

    let (list, _preview) = result.panels();
    let auth = list
        .find("feature/auth")
        .unwrap_or_else(|| panic!("feature/auth row missing:\n{list}"));
    let plain = list
        .find("plain-branch")
        .unwrap_or_else(|| panic!("plain-branch row missing:\n{list}"));
    assert!(
        auth < plain,
        "slashed branch must keep collect order (feature/auth before plain-branch):\n{list}"
    );
}

/// Alt-l / alt-h are skim's built-in horizontal-scroll keys (ScrollRight /
/// ScrollLeft). The picker binds both to `ignore` because each row's `display()`
/// owns its layout with a leading worktree-status sigil; an unbound alt-l slides
/// every row left, clipping that sigil gutter (`no_hscroll(true)` only gates the
/// automatic match-following shift, not the manual offset these keys push).
///
/// The belief is narrow — "alt-l/alt-h change nothing" — so the test asserts it
/// directly: capture the settled list, press the keys, capture again, require
/// the two byte-for-byte equal. No committed frame, so picker column-layout
/// changes never touch this test and the equality can't drift on async-column
/// render timing the way a frozen snapshot can.
#[rstest]
fn test_switch_picker_alt_l_does_not_hscroll(mut repo: TestRepo) {
    // Remove origin so the list doesn't show origin/main
    repo.run_git(&["remote", "remove", "origin"]);
    repo.add_worktree("feature-one");
    repo.add_worktree("feature-two");

    let env_vars = repo.test_env_vars();
    let (baseline, after, exit_code) = exec_in_pty_capture_noop_probe(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
        &[("", Some("feature-two"))], // settle: wait for items to render
        &[
            ("\x1bl", None), // Alt-l: ignored, must not scroll
            ("\x1bl", None), // a second press, still ignored
            ("\x1bh", None), // Alt-h: ignored too
        ],
    );

    assert_valid_abort_exit_code(exit_code);
    assert_eq!(
        baseline, after,
        "alt-l/alt-h must leave the list unscrolled (left = before keys, right = after)"
    );
}

/// A list taller than the viewport renders skim's scrollbar thumb (`▐`) down
/// the right edge of the item pane. The thumb only appears because the picker
/// sets `.scrollbar("▐")` explicitly: skim's `▐` default lives in its clap
/// `default_value`, gated on the `cli` feature we disable, so the library
/// `Default` for the field is the empty string ("no scrollbar"). Without the
/// explicit setting a long worktree/`--prs` list scrolls with no position cue.
/// `--branches` overflows the 30-row test terminal cheaply (one `git branch`
/// per row, no `git worktree add`).
#[rstest]
fn test_switch_picker_scrollbar_on_overflow(repo: TestRepo) {
    repo.run_git(&["remote", "remove", "origin"]);

    // Far more branches than the ~24 item rows the 30-row terminal can show, so
    // the list is guaranteed to overflow and skim paints the scrollbar.
    repo.create_branches(
        &(0..50)
            .map(|i| format!("scroll-{i:02}"))
            .collect::<Vec<_>>(),
    );

    let env_vars = repo.test_env_vars();
    let result = exec_in_pty_capture_before_abort(
        wt_bin().to_str().unwrap(),
        &["switch", "--branches"],
        repo.root_path(),
        &env_vars,
        // `@ main` is the current worktree, always the top row of the list:
        // gating on it confirms the item rows rendered before capture, and a
        // regression fails fast with the list shown rather than via a 30s
        // stabilize timeout (the role `orphan-branch` plays above).
        &[("", Some("@ main"))],
    );

    assert_valid_abort_exit_code(result.exit_code);

    let (list, _preview) = result.panels();
    assert!(
        list.contains('▐'),
        "scrollbar thumb (▐) should render when the list overflows the viewport:\n{list}"
    );
}

#[rstest]
fn test_switch_picker_preview_navigation_and_log_panel(mut repo: TestRepo) {
    repo.run_git(&["remote", "remove", "origin"]);
    let feature_path = repo.add_worktree("feature");

    // One clean commit is enough to distinguish the complete diff and log panes
    // from the clean working-tree pane.
    std::fs::write(feature_path.join("file.txt"), "content\n").unwrap();
    repo.run_git_in(&feature_path, &["add", "file.txt"]);
    repo.run_git_in(
        &feature_path,
        &["commit", "-m", "Commit for preview navigation"],
    );
    // Make the wrapped comments tab deterministic and local.
    seed_ci_status(&repo, "feature", "null");

    let env_vars = repo.test_env_vars();
    let PickerSession {
        child,
        _master,
        writer,
        rx,
        mut parser,
    } = boot_picker_pty(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
    );

    // One real session covers both cyclic directions, skipping empty tabs, and
    // direct Alt-N selection. This row has content only in complete diff,
    // committed diff, and log.
    send_input_awaiting_content(&writer, &rx, &mut parser, "\x1b[B", Some("feature"));
    wait_for_stable_with_content(&rx, &mut parser, Some("diff --git"));

    send_input_awaiting_content(&writer, &rx, &mut parser, "\x1b[Z", Some("* "));

    send_input_awaiting_content(&writer, &rx, &mut parser, "\t", Some("diff --git"));

    send_input_awaiting_content(&writer, &rx, &mut parser, "\t", Some("diff --git"));
    assert!(
        !preview_pane_text(parser.screen()).contains("no working-tree changes"),
        "Tab skips the empty working-tree pane"
    );

    // Direct accelerators still allow inspecting an empty tab.
    send_input_awaiting_content(
        &writer,
        &rx,
        &mut parser,
        "\x1b2",
        Some("has no working-tree changes"),
    );

    send_input_awaiting_content(&writer, &rx, &mut parser, "\x1b1", Some("diff --git"));

    send_input_awaiting_content(&writer, &rx, &mut parser, "\x1b4", Some("* "));

    let list = list_pane_text(parser.screen());
    let preview = preview_pane_text(parser.screen());
    let exit_code = abort_and_exit_code(child, writer, rx);
    assert_valid_abort_exit_code(exit_code);

    let settings = switch_picker_settings(&repo);
    settings.bind(|| {
        assert_snapshot!("switch_picker_preview_log_list", list);
        assert_snapshot!("switch_picker_preview_log_preview", preview);
    });
}

/// Seed a fresh CI-status cache entry for `branch` so the picker primes the
/// row's `pr_status` at skeleton time (`populate_from_cache`) — making the `pr`
/// and `comments` tabs resolve deterministically with no dependence on the live
/// forge fetch's timing. `status_json` is the cached `status` value: `"null"`
/// for "CI checked, no PR", or a PR object (e.g.
/// `{"ci_status":"passed","source":"pr","is_stale":false,"number":{"number":42,"sigil":"#"}}`).
/// `branch` must be a checked-out worktree — its current HEAD is the cache key.
fn seed_ci_status(repo: &TestRepo, branch: &str, status_json: &str) {
    let head = repo.git_output(&["rev-parse", branch]);
    let cache_dir = repo.path().join(".git/wt/cache/ci-status");
    std::fs::create_dir_all(&cache_dir).unwrap();
    let entry = format!(
        r#"{{"status":{status_json},"checked_at":{TEST_EPOCH},"head":"{head}","branch":"{branch}"}}"#,
        head = head.trim(),
    );
    std::fs::write(cache_dir.join(format!("{branch}.json")), entry).unwrap();
}

/// Env vars (mock-bin on PATH + `WORKTRUNK_TEST_MOCK_CONFIG_DIR`) for a PTY
/// `wt` run that should resolve `gh`/`glab` to a mock written into `mock_bin`.
/// Shared by tests that build their own strict list, CI, and preview responses.
fn forge_mock_env_vars(repo: &TestRepo, mock_bin: &Path) -> Vec<(String, String)> {
    let mut env_vars = repo.test_env_vars();
    env_vars.push((
        "WORKTRUNK_TEST_MOCK_CONFIG_DIR".to_string(),
        mock_bin.display().to_string(),
    ));
    // Prepend mock-bin to PATH using the OS separator (`;` on Windows, `:` on
    // Unix) — a hardcoded `:` corrupts the PATH on Windows, so the mock
    // `gh.exe`/`glab.exe` is never found and the `--prs` fetch silently no-ops.
    // `configure_pty_command` sets `PATH` (uppercase), which this entry overrides.
    let base_path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![mock_bin.to_path_buf()];
    paths.extend(std::env::split_paths(&base_path));
    let joined = std::env::join_paths(paths).expect("mock-bin joins into PATH");
    env_vars.push(("PATH".to_string(), joined.to_string_lossy().into_owned()));
    env_vars
}

/// A file-backed mock response gate that also releases on panic, so a failed
/// PTY assertion cannot leave the mock subprocess blocked until its timeout.
struct MockReleaseGate(PathBuf);

impl MockReleaseGate {
    fn new(path: PathBuf) -> Self {
        Self(path)
    }

    fn release(&self) {
        std::fs::write(&self.0, "").expect("release gated mock response");
    }
}

impl Drop for MockReleaseGate {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.0, "");
    }
}

/// Removing a worktree row with alt-x in `--prs` mode must keep the streamed
/// PR/MR rows on screen — only the removed worktree row leaves the list, no
/// alt-r refresh needed.
///
/// alt-x's drop path rebuilds skim's item pool from the picker's shared row list
/// (`resync_pool`). The `--prs` thread streams its PR rows straight to skim's
/// item channel, so unless they're also recorded in that shared list the rebuild
/// drops them. This first proves the complete mocked GitHub
/// fetch lifecycle from its in-flight marker through the loaded state, then
/// drives the drop path and asserts the `#42` PR row survives it.
#[rstest]
fn test_switch_picker_prs_rows_survive_alt_x_removal(mut repo: TestRepo) {
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://github.com/owner/test-repo.git",
    ]);
    // A clean worktree at main's commit: its branch is integrated, so alt-x
    // takes the drop path (row leaves the list → `resync_pool`), not the morph
    // path (which keeps the row in place and never resyncs the pool).
    repo.add_worktree("wt-drop");

    // PR #42's head branch isn't a shown worktree/branch, so it survives the
    // `--prs` dedup and streams in as its own `#42` row. The mock answers only
    // the `--prs` list call (`gh pr list --state`); the per-worktree CI fetch
    // (`gh pr list --head <branch>`) gets an empty list, so `#42` appears solely
    // as the streamed `--prs` row — never folded into a worktree row's CI cell.
    let pr_json = r#"[{"number":42,"title":"Retry the flaky network test","headRefName":"fix/flaky","author":{"login":"octocat"},"isDraft":false,"url":"https://github.com/owner/test-repo/pull/42","body":"Wraps the request in a retry so the suite stops flaking."}]"#;
    let mock_bin = repo.root_path().join("mock-bin");
    std::fs::create_dir_all(&mock_bin).unwrap();
    std::fs::write(mock_bin.join("list.json"), pr_json).unwrap();
    MockConfig::new("gh")
        .version("gh version 1.0.0 (mock)")
        .command(
            "pr list --state",
            MockResponse::file("list.json").wait_for_file("prs.release"),
        )
        .command("pr list --head", MockResponse::output("[]"))
        .command("_default", MockResponse::exit(1))
        .write(&mock_bin);
    let release = MockReleaseGate::new(mock_bin.join("prs.release"));
    let env_vars = forge_mock_env_vars(&repo, &mock_bin);

    let PickerSession {
        child,
        _master,
        writer,
        rx,
        mut parser,
    } = boot_picker_pty(
        wt_bin().to_str().unwrap(),
        &["switch", "--prs"],
        repo.root_path(),
        &env_vars,
    );

    // The list fetch is causally held, so this is the real in-flight frame and
    // no PR row can have streamed in yet.
    wait_for_stable_with_content(&rx, &mut parser, Some("Loading open PRs"));
    let loading_screen = parser.screen().contents();
    assert!(
        loading_screen.contains("Loading open PRs"),
        "loading line on the header while --prs fetches:\n{loading_screen}"
    );
    assert!(
        !loading_screen.contains("#42"),
        "PR row must not render before the list response is released:\n{loading_screen}"
    );

    release.release();

    // Preview off (alt-p) so the full-width list renders the CI column (`#42`),
    // which the preview-shown pane otherwise clips past skim's split. The wait
    // also blocks until the `--prs` row has streamed in.
    send_input_awaiting_content(&writer, &rx, &mut parser, "\x1bp", Some("#42"));
    // The worktree row is on screen too (one skeleton batch).
    wait_for_stable_with_content(&rx, &mut parser, Some("wt-drop"));
    let loaded_screen = parser.screen().contents();
    assert!(
        loaded_screen.contains("#42"),
        "mocked GitHub PR row reached the picker:\n{loaded_screen}"
    );
    assert!(
        !loaded_screen.contains("Retry the flaky network test"),
        "PR title belongs in the hidden preview, not the list row:\n{loaded_screen}"
    );
    assert!(
        !loaded_screen.contains("Loading open PRs"),
        "loading marker clears when the PR row lands:\n{loaded_screen}"
    );

    // Cursor onto the removable worktree row (the row below the pinned current
    // worktree), then alt-x removes it.
    send_input_awaiting_content(&writer, &rx, &mut parser, "\x1b[B", Some("wt-drop"));
    {
        let mut w = writer.lock().unwrap();
        w.write_all(b"\x1bx").unwrap();
        w.flush().unwrap();
    }
    // The worktree row drops; gating on its disappearance proves the resync ran.
    wait_for_stable_until(
        &rx,
        &mut parser,
        |screen| !screen.contains("wt-drop"),
        Some("the wt-drop row to leave the list"),
        None,
    );

    let screen = parser.screen().contents();
    let exit_code = abort_and_exit_code(child, writer, rx);
    assert_valid_abort_exit_code(exit_code);
    assert!(
        screen.contains("#42"),
        "PR row must survive the alt-x worktree removal (no alt-r needed).\nScreen:\n{screen}"
    );
}

/// alt-x on a row that can't be removed explains the rejection immediately and
/// keeps the cursor on that row. Launching from `wt-a` puts the unremovable main
/// worktree between two real rows; that setup makes a one-row cursor drift
/// observable rather than letting bottom-of-list clamping hide it. One transient
/// frame must contain both the header reason and the pointer on `^ main`.
///
/// The flash clearing after the beat is covered by
/// `test_header_flash_set_then_self_clears`; this asserts the flash paints.
#[rstest]
fn test_switch_picker_alt_x_flashes_unremovable_reason(mut repo: TestRepo) {
    repo.run_git(&["remote", "remove", "origin"]);
    let wt_a = repo.add_worktree("wt-a");
    repo.add_worktree("wt-b");

    let env_vars = repo.test_env_vars();
    let PickerSession {
        child,
        _master,
        writer,
        rx,
        mut parser,
    } = boot_picker_pty(wt_bin().to_str().unwrap(), &["switch"], &wt_a, &env_vars);

    wait_for_stable_with_content(&rx, &mut parser, Some("wt-b"));

    // Prove main is genuinely mid-list before relying on cursor preservation:
    // row-specific gutter/name pairs keep the `main↕` column header from
    // satisfying the main lookup.
    let list = list_pane_text(parser.screen());
    let row = |needle: &str| {
        list.lines()
            .position(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("missing picker row {needle:?}:\n{list}"))
    };
    let wt_a_row = row("@ wt-a");
    let main_row = row("^ main");
    let wt_b_row = row("+ wt-b");
    assert!(
        wt_a_row < main_row && main_row < wt_b_row,
        "expected rendered row order wt-a < main < wt-b, got \
         {wt_a_row} < {main_row} < {wt_b_row}:\n{list}"
    );

    // Down from the pinned current row onto the mid-list main worktree.
    send_input_awaiting_content(&writer, &rx, &mut parser, "\x1b[B", Some("^ main"));

    {
        let mut w = writer.lock().unwrap();
        w.write_all(b"\x1bx").unwrap();
        w.flush().unwrap();
    }

    // Observe one frame that proves both effects of the rejected alt-x. Process
    // queued output byte-by-byte and check after each parser state: even if this
    // test thread is descheduled for the flash's full 2.5-second lifetime, a
    // later clear repaint queued behind it cannot erase the transient frame
    // before the predicate sees it.
    let start = Instant::now();
    let mut saw_rejected_frame = false;
    'wait: while start.elapsed() < STABILIZE_TIMEOUT {
        match rx.recv_timeout(POLL_INTERVAL) {
            Ok(chunk) => {
                for byte in chunk {
                    parser.process(&[byte]);
                    let screen = parser.screen().contents();
                    if screen.contains("main worktree cannot be removed")
                        && cursor_points_at(&screen, "^ main")
                    {
                        saw_rejected_frame = true;
                        break 'wait;
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    assert!(
        saw_rejected_frame,
        "never observed one frame with both the unremovable reason and the cursor \
         on the main row:\n{}",
        parser.screen().contents()
    );

    let exit_code = abort_and_exit_code(child, writer, rx);
    assert_valid_abort_exit_code(exit_code);
}

/// skim debounces preview rendering after navigation, so a Down immediately
/// followed by Tab must let the newly selected row resolve the cycle, rather
/// than use whichever row rendered last. A listed PR starts on PR and advances
/// directly to its next available tab, comments.
#[rstest]
fn test_switch_picker_rapid_pr_navigation_cycles_from_pr(repo: TestRepo) {
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://github.com/owner/test-repo.git",
    ]);

    let mock_bin = repo.root_path().join("mock-bin");
    std::fs::create_dir_all(&mock_bin).unwrap();
    let head = repo.git_output(&["rev-parse", "HEAD"]);
    let pr_json = format!(
        r#"[{{"number":42,"title":"Rapid preview cycle","headRefName":"rapid-pr","headRefOid":"{}","author":{{"login":"octocat"}},"isDraft":false,"url":"https://github.com/owner/test-repo/pull/42","body":"body"}}]"#,
        head.trim()
    );
    std::fs::write(mock_bin.join("pr_list.json"), pr_json).unwrap();
    let comments_json = r#"{"comments":[{"author":{"login":"octocat"},"body":"RAPIDCYCLEMARK","createdAt":"2025-01-01T00:00:00Z"}]}"#;
    MockConfig::new("gh")
        .version("gh version 1.0.0 (mock)")
        .command("pr list --state", MockResponse::file("pr_list.json"))
        .command("pr list --head", MockResponse::output("[]"))
        .command("pr view 42", MockResponse::output(comments_json))
        .command("_default", MockResponse::exit(1))
        .write(&mock_bin);
    let env_vars = forge_mock_env_vars(&repo, &mock_bin);

    let PickerSession {
        child,
        _master,
        writer,
        rx,
        mut parser,
    } = boot_picker_pty(
        wt_bin().to_str().unwrap(),
        &["switch", "--prs"],
        repo.root_path(),
        &env_vars,
    );

    // Wait only for the row inventory, then queue navigation and cycling in
    // one flush so the PR preview cannot render between the two key events.
    wait_for_stable_with_content(&rx, &mut parser, Some("rapid"));
    {
        let mut w = writer.lock().unwrap();
        w.write_all(b"\x1b[B\t").unwrap();
        w.flush().unwrap();
    }
    wait_for_stable_with_content(&rx, &mut parser, Some("RAPIDCYCLEMARK"));

    let preview = preview_pane_text(parser.screen());
    assert!(
        preview.contains("RAPIDCYCLEMARK"),
        "Tab after rapid local→PR navigation must open comments:\n{preview}"
    );
    let exit_code = abort_and_exit_code(child, writer, rx);
    assert_valid_abort_exit_code(exit_code);
}

/// A preview pane fills in on its own once its background compute lands — no
/// keystroke needed. The deterministic vehicle is a `--prs` row's `comments`
/// tab: the comment fetch (`gh pr view <n> --json comments`) is held behind a
/// causal file gate. The test first observes the "Loading comments…"
/// placeholder, then releases the fetch; the comment must surface with no
/// further input once the orchestrator pokes a repaint (`PreviewNotifier`).
/// Before that product-side poke the placeholder would strand until the next
/// keystroke — the gap the picker's test harness used to paper over by
/// re-issuing the tab key.
///
/// The PR row is selected by driving the cursor with a re-issued Down, not by an
/// `!main` filter: a filter applied before the async `--prs` row streams in
/// empties the result set, and skim doesn't reselect the lone row when it arrives
/// — so the gate timed out under contention (#3269). Down is idempotent and
/// clamps on the bottom row, where the streamed PR row sits (below the worktree
/// row), so the cursor-arrow wait re-issues it until the `>` pointer holds on
/// `flaky`, outlasting both the stream-in and the cursor reset that the
/// item-list refresh triggers.
#[rstest]
fn test_switch_picker_preview_auto_refreshes_when_compute_lands(repo: TestRepo) {
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://github.com/owner/test-repo.git",
    ]);

    // `pr list` is instant so the row lands promptly. `pr view` cannot answer
    // until the test creates `comments.release`, so boot speed cannot erase the
    // loading-state precondition.
    let mock_bin = repo.root_path().join("mock-bin");
    std::fs::create_dir_all(&mock_bin).unwrap();
    // A short head branch so the PR row isn't truncated in the narrow
    // (preview-shown) list pane. A locally resolvable head OID keeps the log
    // preview on its local fast path, so the gated `pr view` is exclusively the
    // comments fetch whose repaint this test measures.
    let head = repo.git_output(&["rev-parse", "HEAD"]);
    let pr_json = format!(
        r#"[{{"number":42,"title":"Retry the flaky network test","headRefName":"flaky","headRefOid":"{}","author":{{"login":"octocat"}},"isDraft":false,"url":"https://github.com/owner/test-repo/pull/42","body":"body"}}]"#,
        head.trim()
    );
    std::fs::write(mock_bin.join("pr_list.json"), pr_json).unwrap();
    let comments_json = r#"{"comments":[{"author":{"login":"octocat"},"body":"AUTOREFRESHMARK","createdAt":"2025-01-01T00:00:00Z"}]}"#;
    MockConfig::new("gh")
        .version("gh version 1.0.0 (mock)")
        .command("pr list --state", MockResponse::file("pr_list.json"))
        .command("pr list --head", MockResponse::output("[]"))
        .command(
            "pr view 42",
            MockResponse::output(comments_json).wait_for_file("comments.release"),
        )
        .command("_default", MockResponse::exit(1))
        .write(&mock_bin);
    let release = MockReleaseGate::new(mock_bin.join("comments.release"));
    let call_log = tempfile::tempdir().unwrap();
    let mut env_vars = forge_mock_env_vars(&repo, &mock_bin);
    env_vars.push((
        "WORKTRUNK_TEST_MOCK_CALL_LOG_DIR".to_string(),
        call_log.path().display().to_string(),
    ));

    let PickerSession {
        child,
        _master,
        writer,
        rx,
        mut parser,
    } = boot_picker_pty(
        wt_bin().to_str().unwrap(),
        &["switch", "--prs"],
        repo.root_path(),
        &env_vars,
    );

    // Drive the cursor onto the PR row. Its narrow (preview-shown) list line
    // shows the short head branch `flaky`; the worktree row shows `main`. A
    // re-issued Down re-drives the pointer after a streamed-row refresh.
    send_input_awaiting_content(&writer, &rx, &mut parser, "\x1b[B", Some("flaky"));
    send_input_awaiting_content(&writer, &rx, &mut parser, "\x1b8", Some("Loading comments"));
    let loading = preview_pane_text(parser.screen());
    assert!(
        loading.contains("Loading comments"),
        "comments fetch must be visibly in flight before release:\n{loading}"
    );

    // Release the already-open fetch, then wait without sending another
    // keystroke. Only the product's notifier can repaint the marker.
    release.release();
    wait_for_stable_with_content(&rx, &mut parser, Some("AUTOREFRESHMARK"));
    let preview = preview_pane_text(parser.screen());
    assert!(
        preview.contains("AUTOREFRESHMARK"),
        "comment surfaced on its own once the gated fetch landed:\n{preview}"
    );
    let exit_code = abort_and_exit_code(child, writer, rx);
    assert_valid_abort_exit_code(exit_code);
    let preview_calls: Vec<_> = mock_calls(call_log.path(), "gh")
        .into_iter()
        .filter(|call| call.starts_with("pr view"))
        .collect();
    assert_eq!(
        preview_calls,
        ["pr view 42 --json comments"],
        "only the intended comments fetch may drive this repaint"
    );
}

/// Picker-create validates hook templates *before* `git worktree add`, mirroring
/// the pre-flight that `wt switch --create` already performs.
///
/// Without this, a broken `pre-start` template would let the worktree be
/// created, then fail at expansion time — leaving a half-state that blocks
/// re-running (the branch already exists). The test commits a syntax-broken
/// `pre-start` to the user config, fires picker-create, asserts that no branch
/// or worktree was created, then fixes the template and confirms re-running
/// succeeds — proving the pre-flight aborts cleanly rather than leaving a
/// half-created worktree behind.
#[rstest]
fn test_switch_picker_create_validates_templates_before_worktree(repo: TestRepo) {
    repo.run_git(&["remote", "remove", "origin"]);

    // Broken `pre-start` in user config: unbalanced `{{` is a minijinja parse
    // error, so `validate_template` rejects it without needing approvals.
    // Project config would also trigger the validation path, but it routes
    // through the approval gate first and would prompt for a TTY response —
    // user-config hooks are trusted and exercise validation directly.
    repo.write_test_config(r#"pre-start = "echo {{ unclosed""#);

    let env_vars = repo.test_env_vars();

    let result = exec_in_pty_with_input_expectations(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
        &[
            ("new-feature", None), // Type the branch name
            ("\x1bc", None),       // Alt-C: create
        ],
    );

    assert_ne!(
        result.exit_code,
        0,
        "Expected non-zero exit when pre-start template is broken.\nScreen:\n{}",
        result.screen()
    );

    // Branch must not have been created — pre-flight runs before any
    // `git worktree add` / `git branch`.
    let branch_output = repo
        .git_command()
        .args(["branch", "--list", "new-feature"])
        .run()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&branch_output.stdout)
            .trim()
            .is_empty(),
        "Branch `new-feature` should NOT exist, got:\n{}",
        String::from_utf8_lossy(&branch_output.stdout)
    );

    // Worktree directory must not exist either.
    let repo_name = repo.root_path().file_name().unwrap().to_str().unwrap();
    let worktree_dir = repo
        .root_path()
        .parent()
        .unwrap()
        .join(format!("{repo_name}.new-feature"));
    assert!(
        !worktree_dir.exists(),
        "Worktree dir {worktree_dir:?} should NOT have been created"
    );

    // Fix the template and re-run — proves no half-state was left behind.
    repo.write_test_config(r#"pre-start = "true""#);

    let result = exec_in_pty_with_input_expectations(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
        &[("new-feature", None), ("\x1bc", None)],
    );
    // Re-running with the fixed template should succeed. Success is proven by the
    // branch existing below (no half-state was left behind); the exit code
    // tolerates the Windows self-exit-1 quirk (see assert_valid_create_exit_code)
    // — the flake this test hit on the advisory `affected tests (windows)` leg,
    // where a fully-correct create self-exited 1 (PR #3424 CI).
    assert_valid_create_exit_code(result.exit_code);

    let branch_output = repo
        .git_command()
        .args(["branch", "--list", "new-feature"])
        .run()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&branch_output.stdout).contains("new-feature"),
        "Branch `new-feature` should exist after fix.\nScreen:\n{}",
        result.screen()
    );
}

#[rstest]
fn test_switch_picker_emits_cd_directive_by_default(mut repo: TestRepo) {
    repo.run_git(&["remote", "remove", "origin"]);

    // Create a worktree to switch to
    let target_path = repo.add_worktree("target-branch");

    let (cd_path, _guard) = worktrunk::testing::directive_file();

    let mut env_vars = repo.test_env_vars();
    env_vars.push((
        "WORKTRUNK_DIRECTIVE_CD_FILE".to_string(),
        cd_path.display().to_string(),
    ));

    // Run `wt switch` (without --no-cd), select "target-branch" via picker
    let result = exec_in_pty_with_input_expectations(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
        &[
            // Cursor-navigation select, gated on the list-pane `>` pointer: the
            // picker sorts the current worktree first, so one Down lands on
            // `target-branch`. Typing a query instead would tie the selection to
            // skim's matcher, whose filtered item list is swapped in during a
            // *render* while `Accept` reads the cursor's slot directly — so a
            // query gate can only ever assert what was painted, not what Enter
            // will act on. The pointer comes from the same render state as the
            // accept (see `wait_for_cursor_on_row`), and an async row refresh
            // that resets the cursor is absorbed by the re-issued arrow.
            ("\x1b[B", Some("target-branch")),
            ("\r", None), // Enter to switch
        ],
    );

    assert_eq!(
        result.exit_code,
        0,
        "Expected exit code 0 for successful switch.\nScreen:\n{}",
        result.screen()
    );

    // The directive must name the row the picker actually selected, not merely
    // contain some path (which could hide a stale/default-worktree write).
    let cd_content = std::fs::read_to_string(&cd_path).unwrap_or_default();
    assert_eq!(
        crate::common::canonicalize(Path::new(cd_content.trim())).unwrap(),
        crate::common::canonicalize(&target_path).unwrap(),
        "CD directive must point at the selected target-branch worktree; got {cd_content:?}"
    );
}

#[rstest]
fn test_switch_picker_no_cd_switches_without_cd_directive(mut repo: TestRepo) {
    repo.run_git(&["remote", "remove", "origin"]);

    // Create a worktree to switch to
    repo.add_worktree("target-branch");

    let (cd_path, _guard) = worktrunk::testing::directive_file();

    let mut env_vars = repo.test_env_vars();
    env_vars.push((
        "WORKTRUNK_DIRECTIVE_CD_FILE".to_string(),
        cd_path.display().to_string(),
    ));

    // `wt switch --no-cd` opens the picker and switches identically to
    // `wt switch <branch> --no-cd` — it only suppresses the cd directive.
    // `--format=json` is the observable proof the switch pipeline ran: the
    // structured result reaches stdout only after `execute_switch`.
    let result = exec_in_pty_with_input_expectations(
        wt_bin().to_str().unwrap(),
        &["switch", "--no-cd", "--format=json"],
        repo.root_path(),
        &env_vars,
        &[
            // Cursor-navigation select: see test_switch_picker_emits_cd_directive_by_default
            // for why the `>` pointer is the gate rather than a typed query.
            ("\x1b[B", Some("target-branch")),
            ("\r", None), // Enter to switch
        ],
    );

    let screen = result.screen();
    assert_eq!(
        result.exit_code, 0,
        "Expected exit code 0 for --no-cd switch.\nScreen:\n{screen}"
    );

    // The structured result reaches stdout only after execute_switch — the
    // old print-only path emitted a bare branch name and never reached it.
    assert!(
        screen.contains("\"action\""),
        "Expected --format=json switch result on screen.\nScreen:\n{}",
        screen
    );

    // --no-cd suppresses only the cd directive; the switch still ran.
    let cd_content = std::fs::read_to_string(&cd_path).unwrap_or_default();
    assert!(
        cd_content.trim().is_empty(),
        "CD file should be empty with --no-cd, got: {}",
        cd_content
    );
}

/// `{{ base }}` in a picker `--execute` resolves to the source worktree, just
/// as it does on the argument path (`wt switch <branch> -x …`). The picker now
/// captures pre-switch source identity, so the two paths no longer diverge:
/// before, the picker left `base` unset while pre-flight validation still
/// accepted the template, so `-x 'echo {{ base }}'` passed validation and then
/// errored on the undefined value *after* the switch had already landed.
/// Selecting from the `main` worktree, `{{ base }}` expands to `main`.
#[rstest]
fn test_switch_picker_execute_base_resolves_to_source(mut repo: TestRepo) {
    repo.run_git(&["remote", "remove", "origin"]);
    repo.add_worktree("target-branch");

    let (cd_path, _guard) = worktrunk::testing::directive_file();

    let mut env_vars = repo.test_env_vars();
    env_vars.push((
        "WORKTRUNK_DIRECTIVE_CD_FILE".to_string(),
        cd_path.display().to_string(),
    ));

    // Run from the `main` worktree so the captured source branch is `main`.
    let result = exec_in_pty_with_input_expectations(
        wt_bin().to_str().unwrap(),
        &["switch", "--execute", "echo", "--", "{{ base }}"],
        repo.root_path(),
        &env_vars,
        &[
            // Cursor-navigation select: see test_switch_picker_emits_cd_directive_by_default.
            ("\x1b[B", Some("target-branch")),
            ("\r", None), // Enter to switch
        ],
    );

    assert_eq!(
        result.exit_code,
        0,
        "picker `-x '{{{{ base }}}}'` should succeed, not error on an undefined \
         value after the switch.\nScreen:\n{}",
        result.screen()
    );

    assert!(
        result.screen().contains("main"),
        "Screen:\n{}",
        result.screen()
    );
}

/// A project `pre-switch` hook must pass through the approval gate when the
/// picker switches — the picker has no `--yes`, so an unapproved project
/// command is shown for approval, never auto-run.
///
/// Regression: the picker previously passed `yes = true` to
/// `run_pre_switch_hooks`, silently executing project `pre-switch` commands
/// without a prompt — inconsistent with every other hook the picker gates, and
/// a hole in "Project Commands Run Only After Approval". Here the hook is
/// declined at the prompt; it must not run, and the switch must still succeed.
#[rstest]
fn test_switch_picker_pre_switch_hook_requires_approval(mut repo: TestRepo) {
    repo.run_git(&["remote", "remove", "origin"]);
    repo.add_worktree("target-branch");

    // Project `pre-switch` hook (in `.config/wt.toml`, so it routes through the
    // approval gate) that touches a marker outside the worktree if it runs.
    let marker_dir = tempfile::tempdir().unwrap();
    let marker = marker_dir.path().join("pre-switch-ran");
    repo.write_project_config(&format!(
        "pre-switch = {:?}\n",
        format!("touch {}", marker.display())
    ));

    let env_vars = repo.test_env_vars();
    // Select target-branch, press Enter, then decline the approval prompt.
    let result = exec_in_pty_with_input_expectations(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
        &[
            // Cursor-navigation select: see test_switch_picker_emits_cd_directive_by_default.
            ("\x1b[B", Some("target-branch")),
            ("\r", Some("needs approval")), // Enter; wait for the approval prompt
            // Decline. The line terminator is CR, not LF: once skim releases the
            // terminal, the approval prompt's `read_line` runs in the OS line
            // discipline (cooked mode). Windows' console terminates a line on CR
            // (the Enter key) and never on a bare LF, so "n\n" would leave the
            // read blocked until the harness kills the hung process (exit 1). CR
            // terminates on both platforms — Windows reads it as Enter, and on
            // Unix the PTY's ICRNL maps it to LF.
            ("n\r", None), // decline
        ],
    );

    let screen = result.screen();
    assert_eq!(
        result.exit_code, 0,
        "switch should still succeed after declining the pre-switch hook.\nScreen:\n{screen}"
    );
    assert!(
        screen.contains("needs approval"),
        "picker must prompt before running a project pre-switch hook.\nScreen:\n{screen}"
    );
    assert!(!marker.exists(), "a declined pre-switch hook must not run");
}

/// alt-x lands the cursor on the row that slides up — the *immediate* next row —
/// not one past it, even when the removed row has several rows below it.
///
/// A two-row setup can't distinguish a correct landing from a one-row overshoot:
/// `scroll_by` clamps the cursor to the list's last row, so both land on the same
/// remaining row. This removes a *middle* row with two rows below it, where an
/// overshoot lands one row too far instead of being clamped — the exact "jumps
/// two down" a user sees with a long worktree list.
#[rstest]
fn test_switch_picker_alt_x_lands_on_immediate_next_row(mut repo: TestRepo) {
    repo.run_git(&["remote", "remove", "origin"]);
    // Four worktrees beneath the pinned current (main) row. All sit at main's
    // commit, so each alt-x integrates-and-drops (no morph) — the drop path.
    for branch in ["wt-a", "wt-b", "wt-c", "wt-d"] {
        repo.add_worktree(branch);
    }

    let env_vars = repo.test_env_vars();
    let PickerSession {
        child,
        _master,
        writer,
        rx,
        mut parser,
    } = boot_picker_pty(
        wt_bin().to_str().unwrap(),
        &["switch", "--no-cd", "--format=json"],
        repo.root_path(),
        &env_vars,
    );
    let send = |bytes: &[u8]| {
        let mut w = writer.lock().unwrap();
        w.write_all(bytes).unwrap();
        w.flush().unwrap();
    };

    // One skeleton batch carries every worktree row, so waiting for one implies
    // all are present.
    wait_for_stable_with_content(&rx, &mut parser, Some("wt-a"));

    // Learn the rendered order: the current worktree is pinned to the top, then the
    // four worktrees by commit recency (a tie here, so insertion order). Read the
    // four worktree rows top-to-bottom from the list pane.
    let order: Vec<String> = {
        let list = list_pane_text(parser.screen());
        let mut rows: Vec<(usize, String)> = ["wt-a", "wt-b", "wt-c", "wt-d"]
            .iter()
            .filter_map(|name| {
                list.lines()
                    .position(|l| l.contains(name))
                    .map(|line| (line, (*name).to_string()))
            })
            .collect();
        rows.sort_by_key(|(line, _)| *line);
        rows.into_iter().map(|(_, name)| name).collect()
    };
    assert_eq!(order.len(), 4, "all four worktree rows rendered");
    // Remove the second worktree row (two rows still below it); the row directly
    // below it must catch the cursor.
    let remove_target = order[1].clone();
    let expected_landing = order[2].clone();
    let overshoot_row = order[3].clone();

    // Down onto the second worktree row: one Down per row from the pinned current
    // worktree at the top.
    send(b"\x1b[B");
    wait_for_cursor_on_row(&rx, &mut parser, &order[0]);
    send(b"\x1b[B");
    wait_for_cursor_on_row(&rx, &mut parser, &remove_target);

    // alt-x drops it; the cursor must land on the row that slid up — the one
    // directly below, not the one after it. A one-row overshoot lands on
    // `overshoot_row` and times this out.
    send(b"\x1bx");
    wait_for_cursor_on_row(&rx, &mut parser, &expected_landing);

    // Guard against the cursor having blown past to the next row: the pointer marks
    // exactly one row, so a landing on `expected_landing` already excludes
    // `overshoot_row`, but assert it explicitly for a clear failure message.
    let pointer_line = list_pane_text(parser.screen())
        .lines()
        .find(|l| l.starts_with('>'))
        .map(str::to_string)
        .unwrap_or_default();
    assert!(
        !pointer_line.contains(&overshoot_row),
        "alt-x overshot to `{overshoot_row}` instead of the immediate next row \
         `{expected_landing}`.\nPointer line: {pointer_line:?}"
    );

    let _ = abort_and_exit_code(child, writer, rx);
}

/// Dropping the *last* row with alt-x refreshes the preview pane to the new last
/// row. skim auto-repaints the preview across the matcher's `Replace` only when the
/// selected row's text changes — which a last-row drop doesn't produce (`current`
/// goes briefly out of range, then clamps onto the new last row with no text change
/// to detect). The picker fires its own settled-gated `RunPreview`
/// (`run_preview_when_settled`) to cover that; without it the pane keeps showing the
/// removed row's preview until the next keystroke.
#[rstest]
fn test_switch_picker_alt_x_last_row_refreshes_preview(mut repo: TestRepo) {
    repo.run_git(&["remote", "remove", "origin"]);
    // Two clean worktrees at main's commit, so alt-x integrates-and-drops them.
    for branch in ["wt-a", "wt-b"] {
        repo.add_worktree(branch);
    }

    let env_vars = repo.test_env_vars();
    let PickerSession {
        child,
        _master,
        writer,
        rx,
        mut parser,
    } = boot_picker_pty(
        wt_bin().to_str().unwrap(),
        &["switch", "--no-cd", "--format=json"],
        repo.root_path(),
        &env_vars,
    );
    let send = |bytes: &[u8]| {
        let mut w = writer.lock().unwrap();
        w.write_all(bytes).unwrap();
        w.flush().unwrap();
    };

    wait_for_stable_with_content(&rx, &mut parser, Some("wt-b"));

    // Rendered order: the pinned current (main) on top, then the two worktrees by
    // recency. The bottom worktree row is the drop target; the one above it becomes
    // the new last row the cursor lands on.
    let order: Vec<String> = {
        let list = list_pane_text(parser.screen());
        let mut rows: Vec<(usize, String)> = ["wt-a", "wt-b"]
            .iter()
            .filter_map(|name| {
                list.lines()
                    .position(|l| l.contains(name))
                    .map(|line| (line, (*name).to_string()))
            })
            .collect();
        rows.sort_by_key(|(line, _)| *line);
        rows.into_iter().map(|(_, name)| name).collect()
    };
    assert_eq!(order.len(), 2, "both worktree rows rendered");
    let remove_target = order[1].clone(); // the bottom row
    let new_last = order[0].clone(); // becomes the new last row after the drop

    // Down onto the bottom worktree row, then confirm its preview is showing.
    send(b"\x1b[B");
    wait_for_cursor_on_row(&rx, &mut parser, &order[0]);
    send(b"\x1b[B");
    wait_for_cursor_on_row(&rx, &mut parser, &remove_target);
    wait_for_stable_with_content(
        &rx,
        &mut parser,
        Some(&format!("{remove_target} has no file changes vs")),
    );

    // alt-x drops the last row; the cursor lands on the new last row and its preview
    // must refresh (the removed row's preview must not linger).
    send(b"\x1bx");
    wait_for_cursor_on_row(&rx, &mut parser, &new_last);
    wait_for_stable_with_content(
        &rx,
        &mut parser,
        Some(&format!("{new_last} has no file changes vs")),
    );

    let preview = preview_pane_text(parser.screen());
    assert!(
        preview.contains(&format!("{new_last} has no file changes vs")),
        "the preview refreshed to the new last row `{new_last}`.\nPreview:\n{preview}"
    );
    assert!(
        !preview.contains(&format!("{remove_target} has no file changes vs")),
        "the preview must not keep showing the removed row `{remove_target}`.\nPreview:\n{preview}"
    );

    let _ = abort_and_exit_code(child, writer, rx);
}

/// alt-r refreshes the preview pane, not just the row list. The in-memory preview
/// cache is keyed by `(branch, mode)` with no SHA, so a warm complete diff
/// would otherwise survive an edit and re-serve stale content; the refresh clears
/// it so the pane recomputes against the current tree. Targets the pinned current
/// worktree (the top row), so the cursor sits on it before and after the reload
/// regardless of skim's reload cursor behavior — no navigation needed.
#[rstest]
fn test_switch_picker_alt_r_refreshes_preview(repo: TestRepo) {
    repo.run_git(&["remote", "remove", "origin"]);

    // A committed, tracked file in the current worktree so the complete diff has
    // something to show once it's edited.
    let tracked = repo.root_path().join("tracked.txt");
    std::fs::write(&tracked, "original\n").unwrap();
    repo.run_git(&["add", "tracked.txt"]);
    repo.run_git(&["commit", "-m", "add tracked file"]);

    let env_vars = repo.test_env_vars();
    let PickerSession {
        child,
        _master,
        writer,
        rx,
        mut parser,
    } = boot_picker_pty(
        wt_bin().to_str().unwrap(),
        &["switch", "--no-cd", "--format=json"],
        repo.root_path(),
        &env_vars,
    );
    let send = |bytes: &[u8]| {
        let mut w = writer.lock().unwrap();
        w.write_all(bytes).unwrap();
        w.flush().unwrap();
    };

    // The picker opens on the complete-diff tab with the cursor on the pinned
    // current worktree (`main`), whose tree is clean.
    wait_for_stable_with_content(&rx, &mut parser, Some("has no file changes vs"));

    // Edit the tracked file *while the picker is open*. Without the refresh clearing
    // the warm cache, alt-r would re-serve the cached "no changes" pane.
    std::fs::write(&tracked, "original\nedited\n").unwrap();

    send(b"\x1br"); // alt-r: refresh
    wait_for_stable_with_content(&rx, &mut parser, Some("diff --git"));

    let preview = preview_pane_text(parser.screen());
    assert!(
        preview.contains("diff --git"),
        "alt-r must recompute the working-tree preview to show the new edit.\nPreview:\n{preview}"
    );
    assert!(
        !preview.contains("has no file changes vs"),
        "the stale clean preview must not survive the refresh.\nPreview:\n{preview}"
    );

    let _ = abort_and_exit_code(child, writer, rx);
}

/// Removing the sole row matching an active query leaves the filtered list empty,
/// so the settled-gated preview refresh gives up once the matcher settles empty
/// rather than spinning the event loop. A second alt-x with nothing selected is a
/// no-op (the keybinding callback returns early on an empty selection). The picker
/// stays responsive — its screen stabilizes, then aborts cleanly.
#[rstest]
fn test_switch_picker_alt_x_no_match_stays_responsive(mut repo: TestRepo) {
    repo.run_git(&["remote", "remove", "origin"]);
    let wt_path = repo.add_worktree("solo-wt");

    let env_vars = repo.test_env_vars();
    let PickerSession {
        child,
        _master,
        writer,
        rx,
        mut parser,
    } = boot_picker_pty(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
    );

    send_input_awaiting_content(&writer, &rx, &mut parser, "solo-wt", Some("solo-wt"));
    wait_for_cursor_on_row(&rx, &mut parser, "+ solo-wt");

    // First alt-x must actually remove the selected worktree row. The query
    // line still contains `solo-wt`, so look only for the linked-worktree row.
    {
        let mut w = writer.lock().unwrap();
        w.write_all(b"\x1bx").unwrap();
        w.flush().unwrap();
    }
    wait_for_stable_until(
        &rx,
        &mut parser,
        |screen| {
            !screen.lines().any(|line| {
                let list: String = line.chars().take(LIST_WIDTH as usize).collect();
                list.contains("+ solo-wt")
            })
        },
        Some("the + solo-wt row to leave the filtered list"),
        None,
    );
    worktrunk::testing::wait_for_worktree_removed(&wt_path);
    worktrunk::testing::wait_for("integrated solo-wt branch deletion", || {
        repo.git_output(&["branch", "--list", "solo-wt"]).is_empty()
    });
    worktrunk::testing::assert_worktree_removed(&wt_path);
    assert!(
        repo.git_output(&["branch", "--list", "solo-wt"]).is_empty(),
        "integrated solo-wt branch should be deleted with its worktree"
    );

    // The filtered list is now empty. A second alt-x has no selection and must
    // be a no-op; normal Escape teardown proves the event loop stayed live.
    send_input_awaiting_content(&writer, &rx, &mut parser, "\x1bx", None);
    let exit_code = abort_and_exit_code(child, writer, rx);
    assert_valid_abort_exit_code(exit_code);
}

/// alt-x on a *worktree* row whose branch is unmerged morphs the row to
/// `/ branch` **in place**: the worktree is removed, the local branch stays, and
/// the row keeps its slot with the cursor on it — gutter `+` → `/`, no reload, no
/// teleport. The cursor staying put is the whole point of the morph (the old
/// re-collect re-sorted the row to the bottom and reset the cursor to the top).
/// End-to-end through real skim: after alt-x, the list-pane cursor pointer (`>`)
/// must land on the morphed `/ transform-me` row — proving both the in-place
/// gutter flip and the sticky cursor in one assertion.
#[rstest]
fn test_switch_picker_alt_x_morphs_removed_worktree_in_place(mut repo: TestRepo) {
    repo.run_git(&["remote", "remove", "origin"]);

    // A worktree on a branch with a commit the default branch lacks, so
    // `SafeDelete` keeps the branch when the worktree is removed (→ morph, not drop).
    let wt_path = repo.add_worktree("transform-me");
    std::fs::write(wt_path.join("new.txt"), "unmerged work").unwrap();
    repo.git_command()
        .args(["-C", wt_path.to_str().unwrap(), "add", "new.txt"])
        .run()
        .unwrap();
    repo.git_command()
        .args([
            "-C",
            wt_path.to_str().unwrap(),
            "commit",
            "-m",
            "unmerged work",
        ])
        .run()
        .unwrap();

    let env_vars = repo.test_env_vars();
    let PickerSession {
        child,
        _master,
        writer,
        rx,
        mut parser,
    } = boot_picker_pty(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
    );
    let send = |bytes: &[u8]| {
        let mut w = writer.lock().unwrap();
        w.write_all(bytes).unwrap();
        w.flush().unwrap();
    };

    wait_for_stable_with_content(&rx, &mut parser, Some("transform-me"));

    // Filter to the single worktree row so the selection is deterministic
    // regardless of commit-recency order, then confirm the cursor is on it. The
    // row still reads `> + transform-me` — a linked worktree.
    send(b"transform-me");
    wait_for_cursor_on_row(&rx, &mut parser, "+ transform-me");

    // alt-x morphs the row in place. The cursor must land back on the morphed
    // row — `> / transform-me`, gutter flipped to the branch sigil. The morph
    // leaves `search_text` untouched, so the row still matches the active filter;
    // a drop would empty the list, and the old re-collect would reset the cursor.
    send(b"\x1bx");
    wait_for_cursor_on_row(&rx, &mut parser, "/ transform-me");

    worktrunk::testing::wait_for_worktree_removed(&wt_path);
    let exit_code = abort_and_exit_code(child, writer, rx);
    assert_valid_abort_exit_code(exit_code);

    // The worktree is gone but its branch survives — the morph's premise.
    worktrunk::testing::assert_worktree_removed(&wt_path);
    let branches = repo.git_output(&["branch", "--list", "transform-me"]);
    assert!(
        branches.contains("transform-me"),
        "the unmerged branch is retained after its worktree is removed: {branches:?}"
    );
}

/// alt-x under an active fuzzy query lands the cursor on the row displayed just
/// below the removed one — the *filtered display* order, not the removed row's
/// index in the full (unfiltered) `shared_items` list.
///
/// Typing a query both shrinks and reorders skim's `item_list` relative to
/// `shared_items`. A reposition that scrolled to the removed row's `shared_items`
/// index lands rows past the right one — the "+N down" jump a user sees when
/// removing rows after filtering, where N is the count of filtered-out rows above
/// the cursor. The other alt-x cursor tests type no query, so the two index spaces
/// coincide and this regression hides. Here decoy worktrees the query filters out
/// sit between the matching ones, inflating each keeper's `shared_items` index past
/// its displayed index: an index-based reposition overshoots (and `scroll_by`
/// clamps it to the last filtered row), an identity-based one lands on the neighbor.
#[rstest]
fn test_switch_picker_alt_x_lands_on_neighbor_under_filter(mut repo: TestRepo) {
    repo.run_git(&["remote", "remove", "origin"]);
    // Keepers (match the query `keep`) interleaved with decoys (don't), so each
    // keeper carries decoys ahead of it in `shared_items` order. All sit at main's
    // commit, so alt-x integrates-and-drops (the drop path).
    for branch in [
        "keep-1", "other-1", "keep-2", "other-2", "keep-3", "other-3", "keep-4",
    ] {
        repo.add_worktree(branch);
    }

    let env_vars = repo.test_env_vars();
    let PickerSession {
        child,
        _master,
        writer,
        rx,
        mut parser,
    } = boot_picker_pty(
        wt_bin().to_str().unwrap(),
        &["switch", "--no-cd", "--format=json"],
        repo.root_path(),
        &env_vars,
    );
    let send = |bytes: &[u8]| {
        let mut w = writer.lock().unwrap();
        w.write_all(bytes).unwrap();
        w.flush().unwrap();
    };

    wait_for_stable_with_content(&rx, &mut parser, Some("keep-4"));

    // Type the query: only the four keepers survive (the current/main row and the
    // decoys filter out), so the cursor starts on the top keeper.
    send(b"keep");
    wait_for_stable_with_content(&rx, &mut parser, Some("keep-1"));

    // Learn the filtered display order — skim ranks the equal-scoring keepers, so
    // read the rows top-to-bottom rather than assume one.
    let order: Vec<String> = {
        let list = list_pane_text(parser.screen());
        let mut rows: Vec<(usize, String)> = ["keep-1", "keep-2", "keep-3", "keep-4"]
            .iter()
            .filter_map(|name| {
                list.lines()
                    .position(|l| l.contains(name))
                    .map(|line| (line, (*name).to_string()))
            })
            .collect();
        rows.sort_by_key(|(line, _)| *line);
        rows.into_iter().map(|(_, name)| name).collect()
    };
    assert_eq!(
        order.len(),
        4,
        "all four keepers shown under the `keep` filter"
    );
    // Remove the second displayed keeper (two still below it): the row directly
    // below must catch the cursor, not one further down.
    let remove_target = order[1].clone();
    let expected_landing = order[2].clone();
    let overshoot_row = order[3].clone();

    // Down from the top filtered row onto the second keeper.
    send(b"\x1b[B");
    wait_for_cursor_on_row(&rx, &mut parser, &remove_target);

    // alt-x drops it; the cursor must land on the row that slid up. An index-based
    // reposition overshoots toward `overshoot_row` (clamped to the last filtered
    // row) and times this out.
    send(b"\x1bx");
    wait_for_cursor_on_row(&rx, &mut parser, &expected_landing);

    let pointer_line = list_pane_text(parser.screen())
        .lines()
        .find(|l| l.starts_with('>'))
        .map(str::to_string)
        .unwrap_or_default();
    assert!(
        !pointer_line.contains(&overshoot_row),
        "alt-x under a filter overshot to `{overshoot_row}` instead of the \
         immediate next row `{expected_landing}`.\nPointer line: {pointer_line:?}"
    );

    let _ = abort_and_exit_code(child, writer, rx);
}
